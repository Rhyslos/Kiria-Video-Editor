//! Playing the timeline: sound out of the speakers, and the clock the
//! picture follows.
//!
//! The sound is the master clock: the position is how many samples the
//! sound card has actually played, so the picture can't drift from the
//! sound however long playback runs. Without a sound card (or when it fails
//! to open) a steady wall clock takes over and playback stays silent.
//!
//! An audio thread owns the sound card's stream and the mixer. It keeps a
//! short ring buffer (about a quarter of a second) filled; the sound card's
//! callback takes from it and never waits, allocates or locks.
//!
//! The clock stands still at the starting point until the audio thread has
//! the first sound ready (or has found that there is no sound card), so the
//! picture never runs ahead of sound that is still being opened. Stopping
//! never waits for the audio thread either: it is told to stop, the sound
//! card goes quiet at once, and the thread ends by itself when whatever it
//! was doing returns.

use crate::meter::{Levels, Meter, Reading};
use crate::mix::Mixer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use media::audio::CHANNELS;
use project::{Project, SequenceId, Time};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Samples per mixing block.
const BLOCK: usize = 1024;
/// Ring buffer length in seconds.
const BUFFERED: f64 = 0.25;

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

/// Shared between the sound card callback, the audio thread and the interface.
#[derive(Debug)]
struct Clock {
    /// One of [`STARTING`] (the clock stands still), [`SOUND`] (samples the
    /// sound card has played) and [`WALL`] (time passed, silently).
    source: AtomicU8,
    played: AtomicU64,
    rate: AtomicU64,
    created: Instant,
    /// When the wall clock took over, in microseconds after `created`.
    wall_from: AtomicU64,
    /// Playback is over: the sound card gives silence, the audio thread ends.
    stop: AtomicBool,
}

impl Clock {
    fn new() -> Clock {
        Clock {
            source: AtomicU8::new(STARTING),
            played: AtomicU64::new(0),
            rate: AtomicU64::new(0),
            created: Instant::now(),
            wall_from: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        }
    }

    /// From now on the position follows the sound card.
    fn follow_sound(&self) {
        self.source.store(SOUND, Ordering::Release);
    }

    /// From now on the position follows time itself: there is no sound.
    fn follow_time(&self) {
        let now = self.created.elapsed().as_micros() as u64;
        self.wall_from.store(now, Ordering::Release);
        self.source.store(WALL, Ordering::Release);
    }

    /// How far playback has come since it started.
    fn elapsed(&self) -> Time {
        match self.source.load(Ordering::Acquire) {
            SOUND => {
                let rate = self.rate.load(Ordering::Acquire).max(1);
                Time::from_ratio(self.played.load(Ordering::Acquire) as i64, rate).unwrap_or(Time::ZERO)
            }
            WALL => {
                let now = self.created.elapsed().as_micros() as u64;
                let us = now.saturating_sub(self.wall_from.load(Ordering::Acquire));
                Time::from_ratio(us as i64, 1_000_000).unwrap_or(Time::ZERO)
            }
            _ => Time::ZERO,
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct Playback {
    from: Time,
    clock: Arc<Clock>,
    reading: Arc<Reading>,
}

impl Playback {
    /// Starts playing `sequence` of `project` from `from`.
    pub fn start(project: Project, sequence: SequenceId, from: Time) -> Playback {
        let reading = Reading::new();
        let r = Arc::clone(&reading);
        Playback::with_audio(from, reading, move |clock| {
            audio_thread(&project, sequence, from.as_secs_f64(), clock, r);
        })
    }

    /// Runs `audio` on the audio thread. It decides what the clock goes by
    /// and returns soon after the clock says playback has stopped.
    fn with_audio(
        from: Time,
        reading: Arc<Reading>,
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
        Playback { from, clock, reading }
    }

    /// Where playback is now on the timeline. It stays where playback
    /// started until the sound is ready.
    pub fn position(&self) -> Time {
        self.from + self.clock.elapsed()
    }

    /// Where playback started.
    pub fn started_at(&self) -> Time {
        self.from
    }

    /// The master output's peaks and loudness, as last measured.
    pub fn levels(&self) -> Levels {
        self.reading.levels()
    }

    /// Starts the integrated loudness and the highest true peak again
    /// (the master column's reset), from the next mixed block.
    pub fn reset_loudness(&self) {
        self.reading.reset();
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
        self.clock.stop.store(true, Ordering::Release);
    }
}

fn audio_thread(
    project: &Project,
    sequence: SequenceId,
    from: f64,
    clock: &Arc<Clock>,
    reading: Arc<Reading>,
) {
    if clock.stopped() {
        return; // stopped before it began, as when the playhead is dragged while playing
    }
    let seq = project.sequences.iter().find(|s| s.id == sequence);
    let (Some(seq), Some((stream, rate, mut producer))) = (seq, open_output(clock)) else {
        // No sound card (or nothing to play): time itself runs; nothing to mix.
        clock.follow_time();
        return;
    };
    let mut mixer = Mixer::new(rate);
    let mut meter = Meter::new(rate, reading);
    let mut block = vec![0f32; BLOCK * CHANNELS];
    let mut position = from;
    clock.rate.store(u64::from(rate), Ordering::Release);
    while !clock.stopped() {
        if producer.slots() < block.len() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        mixer.mix(project, seq, position, &mut block);
        meter.feed(&block);
        position += BLOCK as f64 / f64::from(rate);
        if let Ok(mut chunk) = producer.write_chunk(block.len()) {
            let (a, b) = chunk.as_mut_slices();
            a.copy_from_slice(&block[..a.len()]);
            b.copy_from_slice(&block[a.len()..]);
            chunk.commit_all();
        }
        // The clock starts once the first sound is queued, so the picture waits for it.
        clock.follow_sound();
    }
    drop(stream);
}

/// Opens the default sound output. Returns the stream (keep it alive), its
/// rate and the ring buffer's writing end; `None` when there is no device.
fn open_output(clock: &Arc<Clock>) -> Option<(cpal::Stream, u32, rtrb::Producer<f32>)> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let config = device
        .default_output_config()
        .map_err(|e| tracing::warn!("no sound output: {e}"))
        .ok()?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels()).max(1);
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new((f64::from(rate) * BUFFERED) as usize * CHANNELS);
    let stream_config = config.config();
    let c = Arc::clone(clock);
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, stream_config, channels, consumer, c),
        cpal::SampleFormat::I16 => build::<i16>(&device, stream_config, channels, consumer, c),
        cpal::SampleFormat::U16 => build::<u16>(&device, stream_config, channels, consumer, c),
        cpal::SampleFormat::I32 => build::<i32>(&device, stream_config, channels, consumer, c),
        other => {
            tracing::warn!("sound output format {other:?} isn't supported; playing silently");
            return None;
        }
    }?;
    stream
        .play()
        .map_err(|e| tracing::warn!("sound output won't start: {e}"))
        .ok()?;
    tracing::info!(rate, channels, "sound output open");
    Some((stream, rate, producer))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut consumer: rtrb::Consumer<f32>,
    clock: Arc<Clock>,
) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                if clock.stopped() {
                    // Stopped while the audio thread is still busy: quiet at once.
                    out.fill(T::from_sample(0.0f32));
                    return;
                }
                let mut played = 0u64;
                for frame in out.chunks_mut(channels) {
                    let (l, r) = match consumer.read_chunk(CHANNELS) {
                        Ok(chunk) => {
                            let mut it = chunk.into_iter();
                            played += 1;
                            (it.next().unwrap_or(0.0), it.next().unwrap_or(0.0))
                        }
                        Err(_) => (0.0, 0.0), // ran dry: silence, and the clock waits
                    };
                    for (i, sample) in frame.iter_mut().enumerate() {
                        let v = if i % 2 == 0 { l } else { r };
                        *sample = T::from_sample(v.clamp(-1.0, 1.0));
                    }
                }
                clock.played.fetch_add(played, Ordering::AcqRel);
            },
            |e| tracing::warn!("sound output error: {e}"),
            None,
        )
        .map_err(|e| tracing::warn!("sound output can't open: {e}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenth() -> Time {
        Time::from_ratio(1, 10).unwrap_or(Time::ZERO)
    }

    #[test]
    fn the_clock_moves_from_where_playback_started() {
        let p = Project::new_automatic("T");
        let playback = Playback::start(p.clone(), p.active_sequence, Time::seconds(3));
        assert_eq!(playback.started_at(), Time::seconds(3));
        std::thread::sleep(Duration::from_millis(300));
        let at = playback.position();
        assert!(at >= Time::seconds(3), "{at:?}");
        if !playback.has_sound() {
            // Without a sound card (as on test machines) time itself runs.
            assert!(at > Time::seconds(3) + tenth(), "{at:?}");
        }
    }

    #[test]
    fn the_clock_stands_still_until_the_sound_is_ready() {
        // Sound that takes a while to get ready, as far into a long file on a slow drive.
        let playback = Playback::with_audio(Time::seconds(5), Reading::new(), |clock| {
            std::thread::sleep(Duration::from_millis(400));
            clock.follow_time();
        });
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(playback.position(), Time::seconds(5), "waiting for the sound");
        assert!(!playback.has_sound());
        std::thread::sleep(Duration::from_millis(500));
        let at = playback.position();
        assert!(at > Time::seconds(5) + tenth(), "moving now: {at:?}");
        assert!(
            at < Time::seconds(5) + Time::from_ratio(45, 100).unwrap(),
            "from where it started: {at:?}"
        );
    }

    #[test]
    fn the_position_follows_the_samples_the_sound_card_played() {
        let playback = Playback::with_audio(Time::seconds(2), Reading::new(), |clock| {
            clock.rate.store(48_000, Ordering::Release);
            clock.played.store(24_000, Ordering::Release);
            clock.follow_sound();
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(playback.has_sound());
        assert_eq!(
            playback.position(),
            Time::seconds(2) + Time::from_ratio(1, 2).unwrap()
        );
    }

    #[test]
    fn closing_waits_for_stopped_audio_threads_to_end() {
        let playback = Playback::with_audio(Time::ZERO, Reading::new(), |clock| {
            while !clock.stopped() {
                std::thread::sleep(Duration::from_millis(5));
            }
            // Letting go of the sound card and the files takes a moment.
            std::thread::sleep(Duration::from_millis(300));
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!wait_until_quiet(Duration::from_millis(50)), "still playing");
        drop(playback);
        assert!(
            !wait_until_quiet(Duration::from_millis(50)),
            "stopped, not ended yet"
        );
        // Other tests' audio threads count too; all of them end within seconds.
        assert!(wait_until_quiet(Duration::from_secs(10)));
    }

    #[test]
    fn stopping_does_not_wait_for_the_audio_thread() {
        let (done, finished) = std::sync::mpsc::channel();
        let playback = Playback::with_audio(Time::ZERO, Reading::new(), move |clock| {
            // Busy with something that can't be interrupted, then the usual check.
            std::thread::sleep(Duration::from_millis(600));
            let _ = done.send(clock.stopped());
        });
        std::thread::sleep(Duration::from_millis(50));
        let before = Instant::now();
        drop(playback);
        assert!(
            before.elapsed() < Duration::from_millis(100),
            "{:?}",
            before.elapsed()
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(5)),
            Ok(true),
            "the thread saw the stop and ended"
        );
    }
}
