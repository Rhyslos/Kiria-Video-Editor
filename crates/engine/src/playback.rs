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

use crate::meter::{Levels, Meter, Reading};
use crate::mix::Mixer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use media::audio::CHANNELS;
use project::{Project, SequenceId, Time};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Samples per mixing block.
const BLOCK: usize = 1024;
/// Ring buffer length in seconds.
const BUFFERED: f64 = 0.25;

/// Shared between the sound card callback, the audio thread and the interface.
#[derive(Debug)]
struct Clock {
    played: AtomicU64,
    rate: AtomicU64,
    sound: AtomicBool,
    started: Instant,
}

#[derive(Debug)]
pub struct Playback {
    from: Time,
    clock: Arc<Clock>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    reading: Arc<Reading>,
}

impl Playback {
    /// Starts playing `sequence` of `project` from `from`.
    pub fn start(project: Project, sequence: SequenceId, from: Time) -> Playback {
        let clock = Arc::new(Clock {
            played: AtomicU64::new(0),
            rate: AtomicU64::new(0),
            sound: AtomicBool::new(false),
            started: Instant::now(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let reading = Reading::new();
        let (c, s, r) = (Arc::clone(&clock), Arc::clone(&stop), Arc::clone(&reading));
        let thread = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || audio_thread(&project, sequence, from.as_secs_f64(), &c, &s, r))
            .map_err(|e| tracing::error!("the audio thread could not start: {e}"))
            .ok();
        Playback {
            from,
            clock,
            stop,
            thread,
            reading,
        }
    }

    /// Where playback is now on the timeline.
    pub fn position(&self) -> Time {
        let elapsed = if self.clock.sound.load(Ordering::Acquire) {
            let rate = self.clock.rate.load(Ordering::Acquire).max(1);
            Time::from_ratio(self.clock.played.load(Ordering::Acquire) as i64, rate).unwrap_or(Time::ZERO)
        } else {
            let us = self.clock.started.elapsed().as_micros() as i64;
            Time::from_ratio(us, 1_000_000).unwrap_or(Time::ZERO)
        };
        self.from + elapsed
    }

    /// Where playback started.
    pub fn started_at(&self) -> Time {
        self.from
    }

    /// The master output's peaks and loudness, as last measured.
    pub fn levels(&self) -> Levels {
        self.reading.levels()
    }

    /// Whether sound is coming out (false: silent, wall clock).
    pub fn has_sound(&self) -> bool {
        self.clock.sound.load(Ordering::Acquire)
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn audio_thread(
    project: &Project,
    sequence: SequenceId,
    from: f64,
    clock: &Arc<Clock>,
    stop: &AtomicBool,
    reading: Arc<Reading>,
) {
    let Some(seq) = project.sequences.iter().find(|s| s.id == sequence) else {
        return;
    };
    let Some((stream, rate, mut producer)) = open_output(clock) else {
        // No sound card: the wall clock runs; nothing to mix.
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    };
    let mut mixer = Mixer::new(rate);
    let mut meter = Meter::new(rate, reading);
    let mut block = vec![0f32; BLOCK * CHANNELS];
    let mut position = from;
    // The clock starts once the first sound is queued, so the picture waits for it.
    clock.rate.store(u64::from(rate), Ordering::Release);
    while !stop.load(Ordering::Acquire) {
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
        clock.sound.store(true, Ordering::Release);
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

    #[test]
    fn the_clock_moves_from_where_playback_started() {
        let p = Project::new_automatic("T");
        let playback = Playback::start(p.clone(), p.active_sequence, Time::seconds(3));
        assert_eq!(playback.started_at(), Time::seconds(3));
        std::thread::sleep(Duration::from_millis(300));
        let at = playback.position();
        assert!(at >= Time::seconds(3), "{at:?}");
        if !playback.has_sound() {
            // Without a sound card (as on test machines) the wall clock runs.
            assert!(
                at > Time::seconds(3) + Time::from_ratio(1, 10).unwrap_or(Time::ZERO),
                "{at:?}"
            );
        }
    }
}
