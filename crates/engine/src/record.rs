//! Recording sound from a microphone onto the timeline (a voice-over).
//!
//! [`Recording::start`] opens the microphone on a thread of its own (a
//! sound device can take a while to open, so never on the interface's).
//! The sound card's callback takes the chosen channel of each frame and
//! pushes it into a wait-free ring, nothing allocated, locked or waited on
//! there; the thread empties the ring into a WAV file every 20 ms
//! ([`wav::WavWriter`], 32-bit float, mono).
//!
//! Lining the take up with the timeline: the microphone is opened first,
//! and what it gives is thrown away until the editor says the timeline has
//! started playing ([`Recording::roll`]), which it does once the microphone
//! is giving sound ([`Recording::ready`]). The take then starts within one
//! of the sound card's blocks of the moment the timeline started from.
//! The time the sound takes to reach the speakers and to come back from the
//! microphone is not taken off: a few tens of milliseconds, which a
//! voice-over doesn't need.
//!
//! The input level (the loudest sample since it was last asked) and the
//! length so far are read from the interface through atomics. Stopping
//! closes the microphone, writes what is left and finishes the file;
//! [`Recording::finished`] then hands the take over. A microphone that goes
//! away mid-take ends it: what was recorded is kept.

pub mod wav;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use diag::AppError;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often the ring is emptied into the file.
const EMPTY_EVERY: Duration = Duration::from_millis(20);

/// How much the ring holds (seconds): far more than the writer ever lets
/// build up, so a slow disk only loses sound after this long.
const RING_SECONDS: usize = 4;

/// Which of the microphone's channels make the recording (it is mono).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// The first (left) channel: a microphone on its own, or the first
    /// input of an interface.
    #[default]
    First,
    /// The second (right) channel: a microphone on an interface's second input.
    Second,
    /// Every channel mixed: the average of them.
    Mixed,
}

impl Channel {
    /// The sample of `frame` (one sample per channel) this channel takes.
    fn of(self, frame: &[f32]) -> f32 {
        match self {
            Channel::First => frame.first().copied().unwrap_or(0.0),
            Channel::Second => frame.get(1).or(frame.first()).copied().unwrap_or(0.0),
            Channel::Mixed if frame.is_empty() => 0.0,
            Channel::Mixed => frame.iter().sum::<f32>() / frame.len() as f32,
        }
    }
}

/// What to record.
#[derive(Debug, Clone)]
pub struct Asked {
    /// The microphone by its name ([`microphones`]); `None` for the
    /// system's default one, or when the one named is gone.
    pub microphone: Option<String>,
    pub channel: Channel,
    /// The file to write (made, or replaced).
    pub path: PathBuf,
}

/// A finished recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub path: PathBuf,
    /// Its length in frames, and its rate.
    pub frames: u64,
    pub rate: u32,
    /// The microphone it was made with.
    pub microphone: String,
    /// Samples lost because the file couldn't keep up (0 normally).
    pub lost: u64,
    /// The microphone went away before the take was stopped.
    pub cut_short: bool,
}

impl Take {
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate.max(1))
    }
}

/// What the callback and the interface share.
#[derive(Debug, Default)]
struct Shared {
    /// The microphone has given sound.
    ready: AtomicBool,
    /// The timeline plays: what the microphone gives is kept.
    rolling: AtomicBool,
    /// The loudest sample since the level was last read, as `f32` bits
    /// (non-negative floats order as their bits do).
    peak: AtomicU32,
    /// Frames kept so far, and the rate (for the length).
    frames: AtomicU64,
    rate: AtomicU32,
    /// Samples that didn't fit in the ring.
    lost: AtomicU64,
    /// The microphone reported an error that ended its stream.
    gone: AtomicBool,
}

/// A recording under way.
#[derive(Debug)]
pub struct Recording {
    shared: Arc<Shared>,
    stop: Sender<()>,
    done: Receiver<diag::Result<Take>>,
    thread: Option<JoinHandle<()>>,
}

impl Recording {
    /// Opens the microphone and starts listening (nothing is kept until
    /// [`Recording::roll`]).
    pub fn start(asked: Asked) -> Recording {
        let shared = Arc::new(Shared::default());
        let (stop, stopped) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("recording".into())
            .spawn(move || {
                let take = record(&asked, &theirs, &stopped);
                let _ = finished.send(take);
            })
            .map_err(|e| tracing::warn!("the recording thread won't start: {e}"))
            .ok();
        Recording {
            shared,
            stop,
            done,
            thread,
        }
    }

    /// The microphone is open and giving sound.
    pub fn ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }

    /// From now on what the microphone gives is kept: the timeline has
    /// started playing from where the take goes.
    pub fn roll(&self) {
        self.shared.rolling.store(true, Ordering::Release);
    }

    pub fn rolling(&self) -> bool {
        self.shared.rolling.load(Ordering::Acquire)
    }

    /// The loudest sample since this was last asked (1.0 is full scale).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::AcqRel))
    }

    /// How long the take is so far (seconds).
    pub fn seconds(&self) -> f64 {
        let rate = self.shared.rate.load(Ordering::Acquire).max(1);
        self.shared.frames.load(Ordering::Acquire) as f64 / f64::from(rate)
    }

    /// Stops: the microphone closes and the file is finished on the
    /// recording's thread; [`Recording::finished`] hands the take over.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// The take, once the recording has stopped (by [`Recording::stop`],
    /// or because the microphone couldn't open or went away).
    pub fn finished(&mut self) -> Option<diag::Result<Take>> {
        match self.done.try_recv() {
            Ok(take) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Some(take)
            }
            Err(TryRecvError::Empty) if self.thread.is_some() => None,
            Err(_) => Some(Err(AppError::new("The recording stopped")
                .why("Its thread ended without finishing the file.")
                .fix("Try recording again; the log says more."))),
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The names of the microphones the system offers, the default first.
/// Asking the system can take a while: not on the interface thread.
pub fn microphones() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|d| d.to_string());
    let mut names: Vec<String> = host
        .input_devices()
        .map(|devices| devices.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    names.dedup();
    if let Some(default) = default {
        names.retain(|n| *n != default);
        names.insert(0, default);
    }
    names
}

/// The recording's thread: opens the microphone, writes until stopped.
fn record(asked: &Asked, shared: &Arc<Shared>, stopped: &Receiver<()>) -> diag::Result<Take> {
    let host = cpal::default_host();
    let named = asked.microphone.as_ref().and_then(|name| {
        host.input_devices()
            .ok()
            .and_then(|mut devices| devices.find(|d| d.to_string() == *name))
    });
    let device = named.or_else(|| host.default_input_device()).ok_or_else(|| {
        AppError::new("There is no microphone")
            .why("The system offers no device to record sound from.")
            .fix("Plug in a microphone (or allow the app to use it in the system's privacy settings), then record again.")
    })?;
    let microphone = device.to_string();
    let config = device.default_input_config().map_err(|e| {
        AppError::new("The microphone won't open")
            .why(format!("{microphone}: {e}"))
            .fix("Check that it is plugged in and not in use by another program, then record again.")
    })?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels()).max(1);
    shared.rate.store(rate, Ordering::Release);
    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(rate as usize * RING_SECONDS);
    if let Some(folder) = asked.path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| {
            AppError::new("The recording's folder can't be made")
                .why(format!("{}: {e}", folder.display()))
                .fix("Check that the project's folder can be written to, then record again.")
        })?;
    }
    let mut wav = wav::WavWriter::create(&asked.path, rate, 1)?;
    let stream = open(&device, &config, channels, asked.channel, producer, shared).map_err(|e| {
        AppError::new("The microphone won't open")
            .why(format!("{microphone}: {e}"))
            .fix("Check that it is plugged in and not in use by another program, then record again.")
    })?;
    stream.play().map_err(|e| {
        AppError::new("The microphone won't start")
            .why(format!("{microphone}: {e}"))
            .fix("Check that it is plugged in and not in use by another program, then record again.")
    })?;
    tracing::info!("recording from {microphone} at {rate} Hz, {channels} channels");
    let mut buffer = Vec::with_capacity(rate as usize);
    let mut cut_short = false;
    loop {
        match stopped.recv_timeout(EMPTY_EVERY) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        empty(&mut consumer, &mut buffer, &mut wav, shared)?;
        if shared.gone.load(Ordering::Acquire) {
            cut_short = true;
            break;
        }
    }
    drop(stream);
    empty(&mut consumer, &mut buffer, &mut wav, shared)?;
    let frames = wav.frames();
    let path = wav.finish()?;
    Ok(Take {
        path,
        frames,
        rate,
        microphone,
        lost: shared.lost.load(Ordering::Acquire),
        cut_short,
    })
}

/// Writes what the ring holds.
fn empty(
    consumer: &mut rtrb::Consumer<f32>,
    buffer: &mut Vec<f32>,
    wav: &mut wav::WavWriter,
    shared: &Shared,
) -> diag::Result<()> {
    buffer.clear();
    while let Ok(sample) = consumer.pop() {
        buffer.push(sample);
    }
    if !buffer.is_empty() {
        wav.write(buffer)?;
        shared.frames.store(wav.frames(), Ordering::Release);
    }
    Ok(())
}

/// Opens the microphone's stream at its own format, handing each block to
/// [`Listener::hear`] as floating point.
fn open(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    channels: usize,
    channel: Channel,
    producer: rtrb::Producer<f32>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error> {
    let mut listener = Listener {
        producer,
        shared: Arc::clone(shared),
        channel,
        channels,
        frame: [0.0; MAX_CHANNELS],
    };
    let gone = Arc::clone(shared);
    let on_error = move |e: cpal::Error| {
        tracing::warn!("the microphone stopped: {e}");
        gone.gone.store(true, Ordering::Release);
    };
    let stream_config = config.config();
    match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| listener.hear(data, |s| s),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| listener.hear(data, |s| f32::from(s) / 32_768.0),
            on_error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            stream_config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                listener.hear(data, |s| (f32::from(s) - 32_768.0) / 32_768.0)
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_input_stream(
            stream_config,
            move |data: &[i32], _: &cpal::InputCallbackInfo| {
                listener.hear(data, |s| s as f32 / 2_147_483_648.0)
            },
            on_error,
            None,
        ),
        // Anything else: ask for floating point, which systems convert to.
        _ => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| listener.hear(data, |s| s),
            on_error,
            None,
        ),
    }
}

/// The most channels a microphone frame is read with (more are ignored).
const MAX_CHANNELS: usize = 32;

/// What the sound card's callback does with each block.
struct Listener {
    producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
    channel: Channel,
    channels: usize,
    /// One frame as floating point (no allocation on the sound card's thread).
    frame: [f32; MAX_CHANNELS],
}

impl Listener {
    /// Takes a block: the level always, the samples once rolling.
    fn hear<T: Copy>(&mut self, data: &[T], to_f32: impl Fn(T) -> f32) {
        let rolling = self.shared.rolling.load(Ordering::Acquire);
        let used = self.channels.min(MAX_CHANNELS);
        let mut peak = 0.0f32;
        let mut lost = 0u64;
        for frame in data.chunks_exact(self.channels) {
            for (slot, sample) in self.frame.iter_mut().zip(&frame[..used]) {
                *slot = to_f32(*sample);
            }
            let sample = self.channel.of(&self.frame[..used]);
            peak = peak.max(sample.abs());
            if rolling && self.producer.push(sample).is_err() {
                lost += 1;
            }
        }
        if peak.is_finite() {
            self.shared.peak.fetch_max(peak.to_bits(), Ordering::AcqRel);
        }
        if lost > 0 {
            self.shared.lost.fetch_add(lost, Ordering::Relaxed);
        }
        self.shared.ready.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_chosen_makes_the_recording() {
        let frame = [0.5, -0.25];
        assert_eq!(Channel::First.of(&frame), 0.5);
        assert_eq!(Channel::Second.of(&frame), -0.25);
        assert_eq!(Channel::Mixed.of(&frame), 0.125);
        // A microphone of one channel: every choice takes it.
        assert_eq!(Channel::Second.of(&[0.3]), 0.3);
        assert_eq!(Channel::Mixed.of(&[]), 0.0);
    }

    fn listener(
        channels: usize,
        channel: Channel,
        ring: usize,
    ) -> (Listener, rtrb::Consumer<f32>, Arc<Shared>) {
        let (producer, consumer) = rtrb::RingBuffer::new(ring);
        let shared = Arc::new(Shared::default());
        let listener = Listener {
            producer,
            shared: Arc::clone(&shared),
            channel,
            channels,
            frame: [0.0; MAX_CHANNELS],
        };
        (listener, consumer, shared)
    }

    #[test]
    fn nothing_is_kept_until_the_timeline_rolls_but_the_level_shows() {
        let (mut listener, mut consumer, shared) = listener(2, Channel::First, 100);
        listener.hear(&[0.5f32, 0.0, -0.75, 0.0], |s| s);
        assert!(shared.ready.load(Ordering::Acquire));
        assert!(consumer.pop().is_err(), "not rolling: nothing kept");
        assert_eq!(f32::from_bits(shared.peak.load(Ordering::Acquire)), 0.75);
        shared.rolling.store(true, Ordering::Release);
        listener.hear(&[0.1f32, 9.0, 0.2, 9.0], |s| s);
        assert_eq!((consumer.pop(), consumer.pop()), (Ok(0.1), Ok(0.2)));
    }

    #[test]
    fn whole_numbers_become_floating_point_and_a_full_ring_counts_what_is_lost() {
        let (mut listener, mut consumer, shared) = listener(1, Channel::First, 2);
        shared.rolling.store(true, Ordering::Release);
        listener.hear(&[16_384i16, -32_768, 1], |s| f32::from(s) / 32_768.0);
        assert_eq!((consumer.pop(), consumer.pop()), (Ok(0.5), Ok(-1.0)));
        assert_eq!(shared.lost.load(Ordering::Acquire), 1);
    }

    /// Nothing is allocated on the sound card's thread.
    #[test]
    fn hearing_a_block_allocates_nothing() {
        let (mut listener, _consumer, shared) = listener(2, Channel::Mixed, 4096);
        shared.rolling.store(true, Ordering::Release);
        let block = vec![0.25f32; 512];
        let ((), allocations) = os::alloc_count::counted(|| listener.hear(&block, |s| s));
        assert_eq!(allocations, 0);
    }

    /// With the system's default microphone, where there is one (not on
    /// machines without sound devices, as the build machines are).
    #[test]
    fn a_recording_without_a_microphone_says_so() {
        if !microphones().is_empty() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut recording = Recording::start(Asked {
            microphone: None,
            channel: Channel::First,
            path: dir.path().join("take.wav"),
        });
        let take = loop {
            if let Some(take) = recording.finished() {
                break take;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let err = take.unwrap_err();
        assert!(err.what().contains("microphone"), "{err:?}");
    }
}
