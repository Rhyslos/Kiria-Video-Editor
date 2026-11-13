//! Playing the timeline: sound out of the speakers, and the clock the
//! picture follows.
//!
//! The sound is the master clock: the position is the timeline sample the
//! sound card is being handed now, less the mixer's fixed delay, so the
//! picture can't drift from the sound however long playback runs. Without
//! a sound card (or when it fails to open) a steady wall clock takes over
//! and playback stays silent.
//!
//! Two threads per playback:
//!
//! - **The sound card's callback** ([`crate::output::Output`]) owns the
//!   real-time mixer (`crates/audio`): every track's effects, volume, pan,
//!   mute and solo, and the master, in 256-frame blocks, at the priority the
//!   system gives sound. It never allocates, locks or waits.
//! - **The audio thread** is the feeder ([`crate::feeder::Feeder`]): it
//!   decodes and mixes each track's clips about half a second ahead into
//!   that track's ring, takes the project as it changes ([`Playback::follow`],
//!   [`Playback::update`]: changes travel to the mixer as commands, heard
//!   within a block) and makes jumps ([`Playback::jump`]): it stops
//!   feeding, the mixer empties its rings and goes on from the new place
//!   (`audio::Controller::locate` / `located`), and the feeder fills them
//!   from there before sound resumes.
//!
//! The clock stands still at the starting point (or the place jumped to)
//! until the sound from there is ready, so the picture never runs ahead
//! of sound that is still being opened. Stopping never waits for the audio
//! thread either: it is told to stop, the sound card goes quiet at once,
//! and the thread ends by itself when whatever it was doing returns.

use crate::feeder::{self, Feeder, MixShape};
use crate::meter::{Levels, MasterReadout};
use crate::output::{Output, Shared};
use audio::settings::{MasterSettings, sample_at};
use audio::{Analyser, Controller, Spectrum, TrackReadout};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use project::{Project, SequenceId, Time, TrackId};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How far ahead the feeder mixes each track, seconds.
const AHEAD: f64 = 0.5;
/// How much of each track is mixed before the sound starts, seconds.
const PRIME: f64 = 0.1;
/// The longest a jump waits for the sound card to take it.
const JUMP_PATIENCE: Duration = Duration::from_secs(2);

/// Audio threads that have not ended yet, across every playback of this
/// run. Stopping never waits for them; closing the app does, briefly
/// ([`wait_until_quiet`]).
static AUDIO_THREADS: AtomicUsize = AtomicUsize::new(0);

/// Counts an audio thread for as long as it lives, also if it panics.
struct Running;

impl Running {
    fn new() -> Running {
        AUDIO_THREADS.fetch_add(1, Ordering::AcqRel);
        Running
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        AUDIO_THREADS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Waits until every audio thread of stopped playbacks has ended, at most
/// `timeout`. Returns whether they all have.
///
/// For closing the app: a thread still inside the sound card's or a
/// decoder's code when the program ends can take the program down with it.
/// Stop playback first; a playback still running keeps its thread.
pub fn wait_until_quiet(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while AUDIO_THREADS.load(Ordering::Acquire) > 0 {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// What the clock goes by.
const STARTING: u8 = 0;
const SOUND: u8 = 1;
const WALL: u8 = 2;

/// Shared between the sound card's callback, the audio thread and the interface.
#[derive(Debug)]
struct Clock {
    /// One of [`STARTING`] (the clock stands still), [`SOUND`] (the sample
    /// the sound card is handed) and [`WALL`] (time passed, silently).
    source: AtomicU8,
    rate: AtomicU64,
    /// What the callback says (and the stop).
    output: Arc<Shared>,
    /// The number of the last jump whose sound is ready (0: the start).
    jumped: AtomicU64,
    created: Instant,
    /// When the wall clock last started from the target, in microseconds
    /// after `created`.
    wall_from: AtomicU64,
    /// The master's meter, once the mixer is made.
    readout: OnceLock<Arc<MasterReadout>>,
    /// Each track's meter, as the feeder last added and removed tracks
    /// (the audio thread swaps the list; the interface clones it).
    meters: Mutex<Vec<(TrackId, Arc<TrackReadout>)>>,
    /// What the EQ's analyser shows, once the mixer is made.
    spectrum: OnceLock<Arc<Spectrum>>,
}

impl Clock {
    fn new() -> Clock {
        Clock {
            source: AtomicU8::new(STARTING),
            rate: AtomicU64::new(0),
            output: Arc::new(Shared::default()),
            jumped: AtomicU64::new(0),
            created: Instant::now(),
            wall_from: AtomicU64::new(0),
            readout: OnceLock::new(),
            meters: Mutex::new(Vec::new()),
            spectrum: OnceLock::new(),
        }
    }

    fn now_us(&self) -> u64 {
        self.created.elapsed().as_micros() as u64
    }

    /// From now on the position follows the sound card.
    fn follow_sound(&self) {
        self.source.store(SOUND, Ordering::Release);
    }

    /// From now on the position follows time itself: there is no sound.
    fn follow_time(&self) {
        self.wall_from.store(self.now_us(), Ordering::Release);
        self.source.store(WALL, Ordering::Release);
    }

    /// Where playback is, given the place it last started or jumped to
    /// (`target`, jump number `jump`).
    fn position(&self, target: Time, jump: u64) -> Time {
        match self.source.load(Ordering::Acquire) {
            SOUND => {
                let out = &self.output;
                // Sound from the place jumped to isn't ready, or the sound
                // card hasn't taken the jump yet: stand at the target.
                if self.jumped.load(Ordering::Acquire) < jump {
                    return target;
                }
                let rate = self.rate.load(Ordering::Acquire).max(1);
                let heard = out.heard.load(Ordering::Acquire);
                let heard =
                    Time::from_ratio(i64::try_from(heard).unwrap_or(i64::MAX), rate).unwrap_or(target);
                heard.max(target)
            }
            WALL => {
                let us = self
                    .now_us()
                    .saturating_sub(self.wall_from.load(Ordering::Acquire));
                target + Time::from_ratio(us as i64, 1_000_000).unwrap_or(Time::ZERO)
            }
            _ => target,
        }
    }

    fn stopped(&self) -> bool {
        self.output.stop.load(Ordering::Acquire)
    }
}

/// What the interface sends the audio thread.
#[derive(Debug)]
enum Message {
    /// The project as it is now.
    Project(Box<Project>),
    /// Go on from `at`; `number` counts jumps.
    Jump { at: Time, number: u64 },
    /// The track the EQ's analyser listens to, if any.
    Analyse(Option<TrackId>),
}

#[derive(Debug)]
pub struct Playback {
    sequence: SequenceId,
    /// Where playback started, or was last sent to by a jump.
    from: Time,
    jumps: u64,
    clock: Arc<Clock>,
    messages: Sender<Message>,
    mix: Option<MixShape>,
    /// The track last sent to the analyser.
    analysed: Option<TrackId>,
}

impl Playback {
    /// Starts playing `sequence` of `project` from `from`.
    pub fn start(project: Project, sequence: SequenceId, from: Time) -> Playback {
        let mix = MixShape::of(&project, sequence);
        let (messages, inbox) = channel();
        let mut playback = Playback::with_audio(sequence, from, messages, move |clock| {
            audio_thread(project, sequence, from, clock, &inbox);
        });
        playback.mix = mix;
        playback
    }

    /// Runs `audio` on the audio thread. It decides what the clock goes by
    /// and returns soon after the clock says playback has stopped.
    fn with_audio(
        sequence: SequenceId,
        from: Time,
        messages: Sender<Message>,
        audio: impl FnOnce(&Arc<Clock>) + Send + 'static,
    ) -> Playback {
        let clock = Arc::new(Clock::new());
        let c = Arc::clone(&clock);
        let running = Running::new();
        // The thread is not kept: stopping never waits for it (see `Drop`).
        let started = std::thread::Builder::new().name("audio".into()).spawn(move || {
            let _running = running;
            audio(&c);
        });
        if let Err(e) = started {
            tracing::error!("the audio thread could not start: {e}");
            clock.follow_time();
        }
        Playback {
            sequence,
            from,
            jumps: 0,
            clock,
            messages,
            mix: None,
            analysed: None,
        }
    }

    /// Where playback is now on the timeline. It stays where playback
    /// started (or jumped to) until the sound from there is ready.
    pub fn position(&self) -> Time {
        self.clock.position(self.from, self.jumps)
    }

    /// The timeline being played.
    pub fn sequence(&self) -> SequenceId {
        self.sequence
    }

    /// Where playback started, or last jumped to.
    pub fn started_at(&self) -> Time {
        self.from
    }

    /// Goes on playing from `at`, without opening the sound card again: the
    /// mixer empties its rings and carries on from there once the feeder
    /// has the sound ready (until then the position stays at `at`).
    pub fn jump(&mut self, at: Time) {
        self.jumps += 1;
        self.from = at;
        if self.clock.source.load(Ordering::Acquire) == WALL {
            self.clock.wall_from.store(self.clock.now_us(), Ordering::Release);
            self.clock.jumped.store(self.jumps, Ordering::Release);
            return;
        }
        let number = self.jumps;
        // The audio thread has ended when nothing is heard: nothing to tell.
        let _ = self.messages.send(Message::Jump { at, number });
    }

    /// Hands the audio thread the project as it is now. Changes to tracks'
    /// volume, mute, solo and sound and to the master are heard within a
    /// block; changes to clips from what is mixed next (about half a second
    /// on; jump to hear them at once).
    pub fn update(&mut self, project: &Project) {
        self.mix = MixShape::of(project, self.sequence);
        let _ = self.messages.send(Message::Project(Box::new(project.clone())));
    }

    /// Called every frame of the interface while playing: when the mix
    /// (tracks' volume, mute, solo and sound, the master, tracks added or
    /// taken out) is not what was last sent, sends the project. Cheap when
    /// nothing changed: a comparison, nothing built.
    pub fn follow(&mut self, project: &Project) {
        let same = self
            .mix
            .as_ref()
            .is_some_and(|m| m.matches(project, self.sequence));
        if !same {
            self.update(project);
        }
    }

    /// The master output's peaks and loudness, as last measured.
    pub fn levels(&self) -> Levels {
        self.clock
            .readout
            .get()
            .map_or(Levels::SILENT, |readout| readout.levels())
    }

    /// The master's meter itself (with its compressor's and limiter's gain
    /// reduction), once the mixer is made.
    pub fn readout(&self) -> Option<Arc<MasterReadout>> {
        self.clock.readout.get().cloned()
    }

    /// Starts the integrated loudness and the highest true peak again
    /// (the master column's reset), from the next mixed block.
    pub fn reset_loudness(&self) {
        if let Some(readout) = self.clock.readout.get() {
            readout.reset();
        }
    }

    /// Each track's meter and how far its effects turn it down, in the
    /// timeline's order (empty until the mixer is made). A clone of a short
    /// list behind a lock the audio thread holds only to swap it.
    pub fn track_meters(&self) -> Vec<(TrackId, Arc<TrackReadout>)> {
        self.clock.meters.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// What the EQ's analyser shows (the track [`Playback::analyse`] chose,
    /// after its EQ), once the mixer is made.
    pub fn spectrum(&self) -> Option<Arc<Spectrum>> {
        self.clock.spectrum.get().cloned()
    }

    /// Which track the EQ's analyser listens to; sent only when it changes.
    pub fn analyse(&mut self, track: Option<TrackId>) {
        if self.analysed != track {
            self.analysed = track;
            let _ = self.messages.send(Message::Analyse(track));
        }
    }

    /// Whether sound is coming out (false: silent, or not ready yet).
    pub fn has_sound(&self) -> bool {
        self.clock.source.load(Ordering::Acquire) == SOUND
    }
}

impl Drop for Playback {
    /// Stops playback without waiting for the audio thread. That thread may
    /// be in the middle of opening a file or jumping in one, which can take
    /// long on a slow drive, and this runs on the interface thread.
    fn drop(&mut self) {
        self.clock.output.stop.store(true, Ordering::Release);
    }
}

/// The audio thread's side of a playback: the mixer's controller, the
/// feeder and what the master was last set to.
struct Session {
    controller: Controller,
    feeder: Feeder,
    sequence: SequenceId,
    master: MasterSettings,
    /// The EQ's analyser, stepped on this thread.
    analyser: Analyser,
    /// The track it listens to, sent again when tracks change slots.
    analysed: Option<TrackId>,
}

/// Makes the mixer for `sequence` at `rate`, adds every track, sends it to
/// `from` and fills the first sound. Returns the audio thread's side and
/// the sound card's.
fn prepare(
    project: Project,
    sequence: SequenceId,
    from: Time,
    (rate, channels): (u32, usize),
    clock: &Clock,
) -> diag::Result<(Session, Output)> {
    let seq = project.sequence(sequence).ok_or_else(|| {
        diag::AppError::new("The timeline can't be played")
            .why("It is not in the project any more.")
            .fix("Open another timeline and play again.")
    })?;
    let master = MasterSettings::from_project(seq);
    let slots = feeder::slots_for(seq);
    let (mut mixer, mut controller, analyser) = audio::mixer(rate, slots, master);
    let _ = clock.readout.set(controller.master());
    let _ = clock.spectrum.set(analyser.spectrum());
    let at = sample_at(from, rate);
    let ring = (f64::from(rate) * AHEAD).ceil() as usize;
    let mut feeder = Feeder::new(
        project,
        sequence,
        &mut controller,
        (slots, ring),
        (at, from.as_secs_f64()),
        false,
    )?;
    if let Ok(mut meters) = clock.meters.lock() {
        *meters = feeder.readouts();
    }
    controller.locate(at)?;
    mixer.idle();
    let prime = (f64::from(rate) * PRIME).ceil() as usize;
    feeder.prime(prime, &|| !clock.stopped());
    clock.rate.store(u64::from(rate), Ordering::Release);
    let output = Output::new(
        mixer,
        controller.located_handle(),
        Arc::clone(&clock.output),
        channels,
    );
    clock.output.primed.store(true, Ordering::Release);
    Ok((
        Session {
            controller,
            feeder,
            sequence,
            master,
            analyser,
            analysed: None,
        },
        output,
    ))
}

impl Session {
    /// Takes what the interface sent, frees what the mixer sent back and
    /// fills the rings. Returns the frames written.
    fn step(&mut self, inbox: &Receiver<Message>, clock: &Clock) -> usize {
        let mut jump = None;
        loop {
            match inbox.try_recv() {
                Ok(Message::Project(project)) => self.take(*project, clock),
                Ok(Message::Jump { at, number }) => jump = Some((at, number)),
                Ok(Message::Analyse(track)) => {
                    self.analysed = track;
                    self.send_analysed();
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if let Some((at, number)) = jump {
            self.jump(at, number, clock);
        }
        self.controller.collect();
        self.analyser.step();
        self.feeder.fill(&|| !clock.stopped())
    }

    /// Tells the mixer which slot the analyser listens to.
    fn send_analysed(&mut self) {
        let slot = self
            .analysed
            .and_then(|t| self.feeder.slot_of(t))
            .and_then(|s| u16::try_from(s).ok());
        if let Err(err) = self.controller.send(audio::Command::Analyse(slot)) {
            tracing::warn!("{}", err.report());
        }
    }

    /// The project as it is now: the tracks' and the master's changes.
    fn take(&mut self, project: Project, clock: &Clock) {
        if let Some(seq) = project.sequence(self.sequence) {
            let master = MasterSettings::from_project(seq);
            for change in master.changes_from(&self.master) {
                if let Err(err) = self.controller.change_master(change) {
                    tracing::warn!("{}", err.report());
                }
            }
            self.master = master;
        }
        if let Err(err) = self.feeder.update(project, &mut self.controller) {
            tracing::warn!("{}", err.report());
        }
        // Tracks added or taken out: the meters and the analysed slot follow.
        if let Ok(mut meters) = clock.meters.lock() {
            *meters = self.feeder.readouts();
        }
        self.send_analysed();
    }

    /// Goes on from `at`: silence while the mixer jumps and the rings fill
    /// from there, then sound, and the clock moves from `at`.
    fn jump(&mut self, at: Time, number: u64, clock: &Clock) {
        let out = &clock.output;
        out.primed.store(false, Ordering::Release);
        let rate = self.controller.rate();
        let sample = sample_at(at, rate);
        match self.controller.locate(sample) {
            Ok(located) => {
                // The callback takes the jump at its next call, and says so.
                let give_up = Instant::now() + JUMP_PATIENCE;
                while out.heard_after.load(Ordering::Acquire) < located && !clock.stopped() {
                    if Instant::now() >= give_up {
                        tracing::warn!("the sound card didn't take a jump in time");
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            Err(err) => tracing::warn!("{}", err.report()),
        }
        self.feeder.jump(sample, at.as_secs_f64());
        let prime = (f64::from(rate) * PRIME).ceil() as usize;
        self.feeder.prime(prime, &|| !clock.stopped());
        clock.jumped.store(number, Ordering::Release);
        out.primed.store(true, Ordering::Release);
    }
}

fn audio_thread(
    project: Project,
    sequence: SequenceId,
    from: Time,
    clock: &Arc<Clock>,
    inbox: &Receiver<Message>,
) {
    if clock.stopped() {
        return; // stopped before it began, as when the playhead is dragged while playing
    }
    // As in the callback and in export: the feeder's decoding and the
    // nested timelines it mixes treat very small numbers as zero, so they
    // never slow down in silence and make what export makes.
    os::float::denormals_as_zero();
    let Some(device) = output_device() else {
        // No sound card: time itself runs; nothing to mix.
        clock.follow_time();
        return;
    };
    let (mut session, output) = match prepare(project, sequence, from, (device.rate, device.channels), clock)
    {
        Ok(ready) => ready,
        Err(err) => {
            tracing::warn!("{}", err.report());
            clock.follow_time();
            return;
        }
    };
    let Some(stream) = device.start(output) else {
        clock.follow_time();
        return;
    };
    clock.follow_sound();
    while !clock.stopped() {
        if session.step(inbox, clock) == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let dry = clock.output.dry.load(Ordering::Relaxed);
    if dry > 0 {
        tracing::info!(
            "{dry} frames of silence were played because a track's sound was late ({} ms over all tracks)",
            dry * 1000 / u64::from(device.rate.max(1))
        );
    }
    drop(stream);
}

/// The default sound output, not yet started.
struct Device {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    rate: u32,
    channels: usize,
}

/// The default sound output; `None` when there is none.
fn output_device() -> Option<Device> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let config = device
        .default_output_config()
        .map_err(|e| tracing::warn!("no sound output: {e}"))
        .ok()?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels()).max(1);
    Some(Device {
        device,
        config,
        rate,
        channels,
    })
}

impl Device {
    /// Opens the stream with `output` in its callback and starts it; `None`
    /// (logged) when it won't open. Keep the stream alive.
    fn start(&self, output: Output) -> Option<cpal::Stream> {
        let channels = self.channels;
        let config = self.config.config();
        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 => {
                self.build(config, output, move |o, out: &mut [f32]| o.render(out, channels))
            }
            cpal::SampleFormat::I16 => self.build(config, output, move |o, out: &mut [i16]| {
                o.render_as(out, channels)
            }),
            cpal::SampleFormat::U16 => self.build(config, output, move |o, out: &mut [u16]| {
                o.render_as(out, channels)
            }),
            cpal::SampleFormat::I32 => self.build(config, output, move |o, out: &mut [i32]| {
                o.render_as(out, channels)
            }),
            other => {
                tracing::warn!("sound output format {other:?} isn't supported; playing silently");
                return None;
            }
        }?;
        stream
            .play()
            .map_err(|e| tracing::warn!("sound output won't start: {e}"))
            .ok()?;
        tracing::info!(rate = self.rate, channels, "sound output open");
        Some(stream)
    }

    fn build<T>(
        &self,
        config: cpal::StreamConfig,
        mut output: Output,
        render: impl Fn(&mut Output, &mut [T]) + Send + 'static,
    ) -> Option<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        self.device
            .build_output_stream(
                config,
                move |out: &mut [T], _| render(&mut output, out),
                // Also where the system's refusal of the sound's priority is said.
                |e| tracing::warn!("sound output: {e}"),
                None,
            )
            .map_err(|e| tracing::warn!("sound output can't open: {e}"))
            .ok()
    }
}

#[cfg(test)]
mod tests;
