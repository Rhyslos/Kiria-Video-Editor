//! Playing the timeline: sound out of the speakers, and the clock the
//! picture follows.
//!
//! The sound is the master clock: the position is the timeline sample the
//! sound card is being handed now, less the mixer's fixed delay, so the
//! picture can't drift from the sound however long playback runs. Without
//! a sound card (or when it fails to open) the mixer runs all the same, on
//! a thread of its own handed a buffer at real-time pace
//! ([`crate::output::Paced`]): nothing is heard, but the clock, the meters,
//! the loudness count and ducking work as with sound
//! ([`Playback::unheard`]). The same happens when the sound card goes away
//! while playing (unplugged, turned off, its stream no longer valid, or its
//! callback simply no longer called for [`QUIET_LIMIT`]): the mix is taken
//! back from the callback and played on unheard, from where it was. Only
//! when even the mix can't be made does a steady wall clock take over.
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
/// A sound card that hasn't called for sound for this long has gone away
/// (some systems say nothing: the callback simply stops). Far longer than
/// any device's buffer, even a wireless one's.
const QUIET_LIMIT: Duration = Duration::from_secs(1);
/// The longest playback waits for the mix to come back from a sound card
/// that went away before it plays on by the wall clock.
const GIVE_BACK_PATIENCE: Duration = Duration::from_secs(2);

/// Audio threads that have not ended yet, across every playback of this
/// run. Stopping never waits for them; closing the app does, briefly
/// ([`wait_until_quiet`]).
static AUDIO_THREADS: AtomicUsize = AtomicUsize::new(0);

/// Counts an audio thread for as long as it lives, also if it panics: in
/// [`AUDIO_THREADS`] and in its own playback's [`Clock::threads`].
struct Running(Arc<Clock>);

impl Running {
    fn new(clock: Arc<Clock>) -> Running {
        AUDIO_THREADS.fetch_add(1, Ordering::AcqRel);
        clock.threads.fetch_add(1, Ordering::AcqRel);
        Running(clock)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.threads.fetch_sub(1, Ordering::AcqRel);
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
    wait_until_none(&AUDIO_THREADS, timeout)
}

/// Waits until `threads` counts none, at most `timeout`. Returns whether
/// it does.
fn wait_until_none(threads: &AtomicUsize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while threads.load(Ordering::Acquire) > 0 {
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
/// The samples handed out at real-time pace without a sound card.
const PACED: u8 = 3;

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
    /// This playback's audio threads while they live (the audio thread, the
    /// paced thread standing in for the sound card, a sound card being let
    /// go), so a test can wait for its own threads while others' run too.
    threads: AtomicUsize,
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
            threads: AtomicUsize::new(0),
        }
    }

    fn now_us(&self) -> u64 {
        self.created.elapsed().as_micros() as u64
    }

    /// From now on the position follows the sound card.
    fn follow_sound(&self) {
        self.source.store(SOUND, Ordering::Release);
    }

    /// From now on the position follows the samples mixed at real-time
    /// pace without a sound card.
    fn follow_paced(&self) {
        self.source.store(PACED, Ordering::Release);
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
            SOUND | PACED => {
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
    /// What the analyser listens to, if anything.
    Analyse(Option<Listen>),
}

/// What the analyser (the EQ's, and the Audio page's band) listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// A track, after its EQ.
    Track(TrackId),
    /// The mix, as the sound card is handed it.
    Mix,
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
    /// What the analyser was last told to listen to.
    analysed: Option<Listen>,
}

impl Playback {
    /// Starts playing `sequence` of `project` from `from`.
    pub fn start(project: Project, sequence: SequenceId, from: Time) -> Playback {
        Playback::start_on(project, sequence, from, Speakers::Default)
    }

    /// As [`Playback::start`], never opening the sound card: mixed at
    /// real-time pace and measured, but not heard (as with no sound card).
    pub fn start_unheard(project: Project, sequence: SequenceId, from: Time) -> Playback {
        Playback::start_on(project, sequence, from, Speakers::None)
    }

    fn start_on(project: Project, sequence: SequenceId, from: Time, speakers: Speakers) -> Playback {
        let mix = MixShape::of(&project, sequence);
        let (messages, inbox) = channel();
        let mut playback = Playback::with_audio(sequence, from, messages, move |clock| {
            audio_thread(project, sequence, from, clock, &inbox, speakers);
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
        let running = Running::new(Arc::clone(&clock));
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

    /// What the analyser shows (what [`Playback::analyse`] chose: a track
    /// after its EQ, or the mix), its spectrum and stereo picture, once the
    /// mixer is made.
    pub fn spectrum(&self) -> Option<Arc<Spectrum>> {
        self.clock.spectrum.get().cloned()
    }

    /// What the analyser listens to; sent only when it changes.
    pub fn analyse(&mut self, track: Option<Listen>) {
        if self.analysed != track {
            self.analysed = track;
            let _ = self.messages.send(Message::Analyse(track));
        }
    }

    /// Whether sound is coming out (false: silent, or not ready yet).
    pub fn has_sound(&self) -> bool {
        self.clock.source.load(Ordering::Acquire) == SOUND
    }

    /// Whether playback runs without being heard: there is no sound card,
    /// or it would not open. The mix is still made and measured at
    /// real-time pace (or, when even that failed, time simply runs).
    pub fn unheard(&self) -> bool {
        matches!(self.clock.source.load(Ordering::Acquire), PACED | WALL)
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
    /// What it listens to, sent again when tracks change slots.
    analysed: Option<Listen>,
    /// The mix is in a sound card's callback, which may go away: a jump
    /// stops waiting for it when it does.
    card: bool,
    /// A jump the sound card went away during, made again once the mix
    /// plays on unheard.
    pending: Option<(Time, u64)>,
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
            card: false,
            pending: None,
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
        // A newer jump replaces one the sound card went away during: the
        // older one is dropped, never made after it.
        let pending = self.pending.take();
        if let Some((at, number)) = jump.or(pending) {
            self.jump(at, number, clock);
        }
        self.controller.collect();
        self.analyser.step();
        self.feeder.fill(&|| !clock.stopped())
    }

    /// Tells the mixer which slot the analyser listens to.
    fn send_analysed(&mut self) {
        let listen = match self.analysed {
            None => audio::Listen::Nothing,
            Some(Listen::Mix) => audio::Listen::Mix,
            Some(Listen::Track(t)) => self
                .feeder
                .slot_of(t)
                .and_then(|s| u16::try_from(s).ok())
                .map_or(audio::Listen::Nothing, audio::Listen::Track),
        };
        if let Err(err) = self.controller.send(audio::Command::Analyse(listen)) {
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
                let mut quiet = Quiet::new(out, Instant::now());
                while out.heard_after.load(Ordering::Acquire) < located && !clock.stopped() {
                    if self.card && (out.lost.load(Ordering::Acquire) || quiet.too_long(out, Instant::now()))
                    {
                        // Made again once the mix plays on without the card.
                        out.lost.store(true, Ordering::Release);
                        self.pending = Some((at, number));
                        return;
                    }
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

/// Where the sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speakers {
    /// The system's default sound output, when there is one.
    Default,
    /// None: mixed at real-time pace and measured, never heard.
    None,
}

fn audio_thread(
    project: Project,
    sequence: SequenceId,
    from: Time,
    clock: &Arc<Clock>,
    inbox: &Receiver<Message>,
    speakers: Speakers,
) {
    if clock.stopped() {
        return; // stopped before it began, as when the playhead is dragged while playing
    }
    // As in the callback and in export: the feeder's decoding and the
    // nested timelines it mixes treat very small numbers as zero, so they
    // never slow down in silence and make what export makes.
    os::float::denormals_as_zero();
    let device = match speakers {
        Speakers::Default => output_device(),
        Speakers::None => None,
    };
    let shape = device
        .as_ref()
        .map_or(crate::output::PACED_SHAPE, |d| (d.rate, d.channels));
    let (session, output) = match prepare(project, sequence, from, shape, clock) {
        Ok(ready) => ready,
        Err(err) => {
            tracing::warn!("{}", err.report());
            clock.follow_time();
            return;
        }
    };
    // The sound card when it opens; otherwise the same mix at real-time
    // pace, unheard, so the meters still work.
    let mut heard = None;
    let mut output = Some(output);
    if let Some(device) = &device {
        match device.start(output.take(), Arc::clone(&clock.output)) {
            Started::Playing(card) => {
                heard = Some(card);
                clock.follow_sound();
            }
            Started::Refused(back) => output = back.map(|o| *o),
        }
    }
    if let Some(output) = output {
        tracing::info!("no sound output: mixing unheard, so the meters still work");
        play_unheard(output, shape, clock);
    } else if heard.is_none() {
        clock.follow_time();
    }
    play(session, heard, shape, clock, inbox);
}

/// Feeds the mix until playback stops. When the sound card `heard` goes
/// away, the mix is taken back from it and played on unheard.
fn play(
    mut session: Session,
    mut heard: Option<Heard>,
    shape: (u32, usize),
    clock: &Arc<Clock>,
    inbox: &Receiver<Message>,
) {
    let out = &clock.output;
    let mut quiet = Quiet::new(out, Instant::now());
    while !clock.stopped() {
        session.card = heard.is_some();
        if heard.is_some() && (out.lost.load(Ordering::Acquire) || quiet.too_long(out, Instant::now())) {
            if let Some(card) = heard.take() {
                tracing::warn!("the sound output went away: playing on unheard, so the meters still work");
                match card.give_back(clock) {
                    Some(output) => play_unheard(output, shape, clock),
                    None => {
                        tracing::warn!(
                            "the mix didn't come back from the sound output; the clock goes by time"
                        );
                        clock.follow_time();
                        break;
                    }
                }
            }
            continue;
        }
        if session.step(inbox, clock) == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let dry = out.dry.load(Ordering::Relaxed);
    if dry > 0 {
        tracing::info!(
            "{dry} frames of silence were played because a track's sound was late ({} ms over all tracks)",
            dry * 1000 / u64::from(shape.0.max(1))
        );
    }
    drop(heard);
}

/// Runs `output` at real-time pace on a thread of its own, unheard, and
/// has the clock follow it (or time itself, when the thread won't start).
/// That thread counts as an audio thread until it ends ([`Running`]), so
/// closing the app waits for it as for the audio thread.
fn play_unheard(output: Output, shape: (u32, usize), clock: &Arc<Clock>) {
    let running = Running::new(Arc::clone(clock));
    match crate::output::Paced::new(output, shape).run(Arc::clone(&clock.output), running) {
        Ok(_thread) => clock.follow_paced(),
        Err(err) => {
            tracing::warn!("{}", err.report());
            clock.follow_time();
        }
    }
}

/// Watches whether the sound card still calls for sound.
struct Quiet {
    calls: u64,
    since: Instant,
}

impl Quiet {
    fn new(out: &Shared, now: Instant) -> Quiet {
        Quiet {
            calls: out.calls.load(Ordering::Acquire),
            since: now,
        }
    }

    /// Whether the callback hasn't been called for [`QUIET_LIMIT`] by `now`.
    fn too_long(&mut self, out: &Shared, now: Instant) -> bool {
        let calls = out.calls.load(Ordering::Acquire);
        if calls != self.calls {
            self.calls = calls;
            self.since = now;
            return false;
        }
        now.duration_since(self.since) >= QUIET_LIMIT
    }
}

/// Whether a sound card's error means its stream won't play any more.
/// Others (a buffer played late, a route changed and followed, real-time
/// priority refused) are only logged.
fn ends_the_stream(kind: cpal::ErrorKind) -> bool {
    matches!(
        kind,
        cpal::ErrorKind::DeviceNotAvailable
            | cpal::ErrorKind::StreamInvalidated
            | cpal::ErrorKind::HostUnavailable
    )
}

/// The slot the callback takes the mix from, and puts it back in when it
/// is let go ([`Held`]).
type Slot = Arc<Mutex<Option<Output>>>;

/// A sound card playing the mix.
struct Heard {
    /// The stream (a `cpal::Stream`): letting go of it ends its callback.
    stream: Box<dyn Send>,
    slot: Slot,
}

impl Heard {
    /// Lets go of the stream and takes the mix back, at most
    /// [`GIVE_BACK_PATIENCE`] later. The stream is let go on a thread of its
    /// own, so a driver that stopped answering can't hold playback up.
    fn give_back(self, clock: &Arc<Clock>) -> Option<Output> {
        let Heard { stream, slot } = self;
        let running = Running::new(Arc::clone(clock));
        let closing = std::thread::Builder::new()
            .name("sound-closing".into())
            .spawn(move || {
                let _running = running;
                drop(stream);
            });
        if let Err(e) = closing {
            // The stream was let go here, with the thread that wouldn't start.
            tracing::warn!("the sound output was closed on the audio thread: {e}");
        }
        let give_up = Instant::now() + GIVE_BACK_PATIENCE;
        loop {
            if let Some(output) = slot.try_lock().ok().and_then(|mut o| o.take()) {
                return Some(output);
            }
            if Instant::now() >= give_up {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// The mix inside the sound card's callback. It takes the mix from its slot
/// on the first call (a `try_lock`, never waiting; once taken the slot is
/// not touched again from the callback) and puts it back when the callback
/// is let go, which is never during a call: so a stream that won't start,
/// or a sound card that went away, gives the mix back to be run unheard.
struct Held {
    output: Option<Output>,
    slot: Slot,
}

impl Held {
    fn new(slot: Slot) -> Held {
        Held { output: None, slot }
    }

    /// The mix, once the slot has handed it over.
    fn output(&mut self) -> Option<&mut Output> {
        if self.output.is_none() {
            // Never waits: the slot is only locked here and once the
            // callback is let go.
            self.output = self.slot.try_lock().ok().and_then(|mut o| o.take());
        }
        self.output.as_mut()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(output) = self.output.take() {
            let mut slot = self
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *slot = Some(output);
        }
    }
}

/// What became of the mix handed to the sound card.
enum Started {
    /// The sound card plays it; keep the stream alive.
    Playing(Heard),
    /// The sound card would not open or start (logged). The mix comes back,
    /// to be run unheard.
    Refused(Option<Box<Output>>),
}

/// The default sound output, not yet started.
struct Device {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    rate: u32,
    /// The channels the stream is opened with: two (the mix is stereo)
    /// whenever the device takes them, else the device's own number.
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
    let own = usize::from(config.channels()).max(1);
    let channels = stream_channels(own, || takes_stereo(&device, &config));
    if channels != own {
        tracing::info!("the sound device mixes {own} channels; the sound goes to it as stereo");
    }
    Some(Device {
        device,
        config,
        rate,
        channels,
    })
}

/// The channels to open a stream with on a device whose own mix has `own`:
/// stereo when it has more and `takes_stereo` says a stereo stream opens.
///
/// Why: the mix is stereo. Handed to the system as a stream of the
/// device's own channel count (eight when the system's spatial sound or a
/// headset's surround is on), the left and right are two speakers of a
/// surround layout, which the system's virtual surround places and turns
/// down like any surround sound; handed as stereo, they are treated as
/// stereo, as other programs' sound is (`docs/research/sound-quality.md`).
fn stream_channels(own: usize, takes_stereo: impl FnOnce() -> bool) -> usize {
    if own > 2 && takes_stereo() { 2 } else { own }
}

/// Whether a stereo stream opens on `device` at `config`'s rate and sample
/// format (the system converts it to the device's own layout). It is opened
/// and closed without being started.
fn takes_stereo(device: &cpal::Device, config: &cpal::SupportedStreamConfig) -> bool {
    let stereo = cpal::StreamConfig {
        channels: 2,
        ..config.config()
    };
    device
        .build_output_stream_raw(
            stereo,
            config.sample_format(),
            |data: &mut cpal::Data, _: &cpal::OutputCallbackInfo| {
                // Never started; silence if it ever runs.
                for byte in data.bytes_mut() {
                    *byte = 0;
                }
            },
            |e| tracing::info!("a stereo stream won't open: {e}"),
            None,
        )
        .map_err(|e| tracing::info!("a stereo stream won't open: {e}"))
        .is_ok()
}

impl Device {
    /// Opens the stream with `output` in its callback and starts it.
    ///
    /// The callback holds `output` in a [`Held`], so when the stream won't
    /// open or start the mix is back in the slot and comes back in
    /// [`Started::Refused`]. An error that ends the stream sets
    /// `shared.lost`.
    fn start(&self, output: Option<Output>, shared: Arc<Shared>) -> Started {
        let slot: Slot = Arc::new(Mutex::new(output));
        let channels = self.channels;
        let config = cpal::StreamConfig {
            channels: cpal::ChannelCount::try_from(channels).unwrap_or(2),
            ..self.config.config()
        };
        let take = Arc::clone(&slot);
        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 => self.build(config, take, &shared, move |o, out: &mut [f32]| {
                o.render(out, channels)
            }),
            cpal::SampleFormat::I16 => self.build(config, take, &shared, move |o, out: &mut [i16]| {
                o.render_as(out, channels)
            }),
            cpal::SampleFormat::U16 => self.build(config, take, &shared, move |o, out: &mut [u16]| {
                o.render_as(out, channels)
            }),
            cpal::SampleFormat::I32 => self.build(config, take, &shared, move |o, out: &mut [i32]| {
                o.render_as(out, channels)
            }),
            other => {
                tracing::warn!("sound output format {other:?} isn't supported; playing unheard");
                None
            }
        };
        let refused = |stream: Option<cpal::Stream>| {
            drop(stream);
            Started::Refused(slot.lock().ok().and_then(|mut o| o.take()).map(Box::new))
        };
        let Some(stream) = stream else {
            return refused(None);
        };
        if let Err(e) = stream.play() {
            tracing::warn!("sound output won't start: {e}");
            return refused(Some(stream));
        }
        // What the system's mixer takes from the app: when sound is quieter
        // or narrower than elsewhere, the log says which device and format.
        tracing::info!(
            device = %self.device,
            rate = self.rate,
            channels,
            format = ?self.config.sample_format(),
            "sound output open"
        );
        Started::Playing(Heard {
            stream: Box::new(stream),
            slot,
        })
    }

    fn build<T>(
        &self,
        config: cpal::StreamConfig,
        slot: Slot,
        shared: &Arc<Shared>,
        render: impl Fn(&mut Output, &mut [T]) + Send + 'static,
    ) -> Option<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut held = Held::new(slot);
        let shared = Arc::clone(shared);
        self.device
            .build_output_stream(
                config,
                move |out: &mut [T], _| match held.output() {
                    Some(o) => render(o, out),
                    None => out.fill(T::EQUILIBRIUM),
                },
                // Also where the system's refusal of the sound's priority is said.
                move |e| {
                    tracing::warn!("sound output: {e}");
                    if ends_the_stream(e.kind()) {
                        shared.lost.store(true, Ordering::Release);
                    }
                },
                None,
            )
            .map_err(|e| tracing::warn!("sound output can't open: {e}"))
            .ok()
    }
}

#[cfg(test)]
mod tests;
