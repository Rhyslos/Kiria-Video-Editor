//! The sound card's end of playback: what runs in its callback.
//!
//! [`Output`] owns the real-time mixer (`crates/audio`) and the adapter
//! that cuts the device's buffers into the mixer's blocks. Each call it
//! asks the processor to treat very small numbers as zero, mixes, and says
//! which sample of the timeline is being heard: the mixer's position less
//! what the adapter still holds and less the mixer's fixed delay, so the
//! picture stays in step with the sound. Before the feeder has the first
//! sound ready, and during a jump, it hands out silence and only lets the
//! mixer take its commands (`audio::Mixer::idle`), so nothing is owed.
//!
//! Nothing here allocates, frees, locks or waits (a test counts).
//!
//! Without a sound card, [`Paced`] stands in for its callback: a thread of
//! its own hands the same [`Output`] a buffer at real-time pace and throws
//! the sound away, so the meters, the loudness count and ducking work as
//! when the sound is heard.

use audio::{BlockAdapter, Mixer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Frames of the device's buffer converted at a time for devices that
/// don't take `f32`.
const CONVERTED: usize = 4_096;

/// What the callback and the rest of playback share.
#[derive(Debug, Default)]
pub struct Shared {
    /// Playback is over: silence at once.
    pub stop: AtomicBool,
    /// The feeder has the sound ready: mix. Otherwise silence, and the
    /// mixer only takes its commands.
    pub primed: AtomicBool,
    /// The timeline sample being heard now.
    pub heard: AtomicU64,
    /// The number of the last jump the mixer had made when `heard` was
    /// worked out (`audio::Controller::locate`).
    pub heard_after: AtomicU64,
    /// Frames of silence played because a track's sound was late.
    pub dry: AtomicU64,
}

/// The sound card's end.
#[derive(Debug)]
pub struct Output {
    mixer: Mixer,
    adapter: BlockAdapter,
    shared: Arc<Shared>,
    /// The mixer's jump numbers (`audio::Controller::located_handle`).
    located: Arc<AtomicU64>,
    latency: u64,
    /// For devices that take another sample format: made once.
    converted: Vec<f32>,
}

impl Output {
    /// Made off the real-time thread: it allocates.
    pub fn new(mixer: Mixer, located: Arc<AtomicU64>, shared: Arc<Shared>, channels: usize) -> Output {
        let latency = mixer.latency() as u64;
        Output {
            mixer,
            adapter: BlockAdapter::default(),
            shared,
            located,
            latency,
            converted: vec![0.0; CONVERTED * channels.max(1)],
        }
    }

    /// How late the mix comes out, in frames (the mixer's fixed delay).
    pub fn latency(&self) -> usize {
        self.latency as usize
    }

    /// Frames of silence played because a track's sound was late, summed
    /// over tracks.
    pub fn dry_frames(&self) -> u64 {
        self.mixer.dry_frames()
    }

    /// Fills `out` (interleaved, `channels` per frame) with the next sound.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        os::float::denormals_as_zero();
        if self.shared.stop.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }
        if self.shared.primed.load(Ordering::Acquire) {
            self.adapter.render(&mut self.mixer, out, channels);
            for sample in out.iter_mut() {
                *sample = sample.clamp(-1.0, 1.0);
            }
        } else {
            self.adapter.clear();
            self.mixer.idle();
            out.fill(0.0);
        }
        let behind = self.adapter.pending() as u64 + self.latency;
        let heard = self.mixer.position().saturating_sub(behind);
        self.shared.heard.store(heard, Ordering::Release);
        self.shared
            .heard_after
            .store(self.located.load(Ordering::Acquire), Ordering::Release);
        self.shared.dry.store(self.mixer.dry_frames(), Ordering::Relaxed);
    }

    /// As [`Output::render`] for a device that takes samples of another
    /// format, converted from a buffer made once.
    pub fn render_as<T>(&mut self, out: &mut [T], channels: usize)
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let channels = channels.max(1);
        let mut converted = std::mem::take(&mut self.converted);
        let step = (converted.len() / channels).max(1) * channels;
        for chunk in out.chunks_mut(step) {
            let part = &mut converted[..chunk.len()];
            self.render(part, channels);
            for (o, s) in chunk.iter_mut().zip(part.iter()) {
                *o = T::from_sample(*s);
            }
        }
        self.converted = converted;
    }
}

/// Frames handed out at a time without a sound card: about 10 ms at 48 kHz,
/// as a sound card's buffer.
pub const PACED_FRAMES: usize = 512;

/// The rate and channels mixed at without a sound card.
pub const PACED_SHAPE: (u32, usize) = (48_000, 2);

/// How far behind the paced thread may fall (a busy machine, a debugger)
/// before it stops catching up and goes on from now.
const GIVE_UP_CATCHING_UP: Duration = Duration::from_millis(100);

/// Playing without a sound card: [`Output`] handed a buffer every
/// [`PACED_FRAMES`] frames' worth of time by a thread of its own, the sound
/// thrown away. The clock then follows the samples handed out, as it
/// follows a sound card's.
#[derive(Debug)]
pub struct Paced {
    output: Output,
    channels: usize,
    buffer: Vec<f32>,
    block: Duration,
    next: Instant,
}

impl Paced {
    /// Made off the paced thread: it allocates its buffer here, once.
    pub fn new(output: Output, (rate, channels): (u32, usize)) -> Paced {
        let channels = channels.max(1);
        Paced {
            output,
            channels,
            buffer: vec![0.0; PACED_FRAMES * channels],
            block: Duration::from_secs_f64(PACED_FRAMES as f64 / f64::from(rate.max(1))),
            next: Instant::now(),
        }
    }

    /// Mixes one buffer and says how long to wait before the next, given
    /// that it is `now`. Never allocates.
    pub fn tick(&mut self, now: Instant) -> Duration {
        self.output.render(&mut self.buffer, self.channels);
        self.next += self.block;
        if now > self.next + GIVE_UP_CATCHING_UP {
            self.next = now;
        }
        self.next.saturating_duration_since(now)
    }

    /// Runs on a thread named `paced-sound` until playback stops. Returns
    /// the thread, or why it couldn't start.
    pub fn run(mut self, shared: Arc<Shared>) -> diag::Result<std::thread::JoinHandle<()>> {
        use diag::Explain;
        std::thread::Builder::new()
            .name("paced-sound".into())
            .spawn(move || {
                os::float::denormals_as_zero();
                self.next = Instant::now();
                while !shared.stop.load(Ordering::Acquire) {
                    let wait = self.tick(Instant::now());
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                }
            })
            .or_explain(|| {
                diag::AppError::new("Playback can't measure the sound")
                    .why("The thread that mixes without a sound card could not start.")
                    .fix("Close a few programs and play again.")
            })
    }
}
