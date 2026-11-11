//! The waveforms the timeline draws: read once per file and sound stream on
//! a worker, then kept for the session.
//!
//! While a stream is being read the timeline already gets the peaks read so
//! far (the waveform fills in from the left) and how far the reading is.
//! The worker and the interface share the growing list of peaks; the
//! interface only ever tries the lock, and copies the list a few times a
//! second, so it never waits for the worker.

use crate::reading::{self, Pace};
use cache::Peaks;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, MediaRef};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ui_kit::timeline::LoadState;

/// How often the peaks read so far are copied for drawing.
const COPY_EVERY: Duration = Duration::from_millis(200);
/// How often the timeline is redrawn while something is being read.
const REDRAW_EVERY: Duration = Duration::from_millis(100);

struct Reading {
    handle: JobHandle<Peaks>,
    /// The peaks read so far; the worker adds a few seconds of sound at a time.
    so_far: Arc<Mutex<Vec<f32>>>,
    /// A copy of them for drawing.
    copy: Option<Peaks>,
    copied_at: Option<Instant>,
    pace: Pace,
    state: LoadState,
}

enum State {
    Reading(Reading),
    Ready(Peaks),
    /// Couldn't be read (missing file, broken sound): drawn without a waveform.
    Failed,
}

/// What the timeline can show of a stream's waveform right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// All the peaks, or the ones read so far.
    pub peaks: Option<Peaks>,
    /// Seconds of the stream read so far: clips that end before this have
    /// their whole waveform. Infinite once everything is read.
    pub read_secs: f64,
    /// How the reading is going; `None` once the waveform is ready.
    pub state: Option<LoadState>,
}

#[derive(Default)]
pub struct Waveforms {
    streams: HashMap<(MediaId, u32), State>,
    /// `--hold-reading`: workers stop at this share, for screenshots of reading in progress.
    hold_at: Option<f32>,
}

impl Waveforms {
    /// With `--hold-reading`: workers stop where the share read reaches `hold_at`.
    pub fn holding_at(hold_at: Option<f32>) -> Waveforms {
        Waveforms {
            hold_at,
            ..Default::default()
        }
    }

    /// What is known of this stream's waveform; starts reading it if nobody has yet.
    pub fn get(&mut self, jobs: &JobSystem, ctx: &egui::Context, media: &MediaRef, stream: u32) -> Look {
        let key = (media.id, stream);
        let hold_at = self.hold_at;
        let state = self.streams.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let so_far = Arc::new(Mutex::new(Vec::new()));
            let shared = Arc::clone(&so_far);
            let length = media.duration.as_secs_f64().max(0.001);
            let handle = jobs.submit(PoolKind::Analysis, Priority::Background, "waveform", move |job| {
                let mut read = 0usize;
                let result = cache::waveform::peaks_as_read(&path, stream, |more| {
                    read += more.len();
                    if let Ok(mut all) = shared.lock() {
                        all.extend_from_slice(more);
                    }
                    let done = share_read(read, length);
                    job.set_progress(done);
                    reading::hold(hold_at, done, job);
                    !job.is_cancelled()
                });
                ctx.request_repaint();
                result
            });
            State::Reading(Reading {
                handle,
                so_far,
                copy: None,
                copied_at: None,
                pace: Pace::default(),
                state: LoadState::Waiting,
            })
        });
        match state {
            State::Ready(peaks) => Look {
                peaks: Some(peaks.clone()),
                read_secs: f64::INFINITY,
                state: None,
            },
            State::Reading(reading) => Look {
                read_secs: reading
                    .copy
                    .as_ref()
                    .map_or(0.0, |p| p.values.len() as f64 / f64::from(p.per_second)),
                peaks: reading.copy.clone(),
                state: Some(reading.state),
            },
            State::Failed => Look {
                peaks: None,
                read_secs: 0.0,
                state: Some(LoadState::Failed),
            },
        }
    }

    /// All of this stream's peaks, once every one of them is read: for work
    /// that must not judge from half a waveform (finding silence). Starts
    /// reading them if nobody has yet.
    pub fn whole(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> Option<Peaks> {
        let look = self.get(jobs, ctx, media, stream);
        look.state.is_none().then_some(look.peaks).flatten()
    }

    /// Reads this file's streams again next time (it was found somewhere else).
    pub fn forget(&mut self, media: MediaId) {
        self.streams.retain(|(id, _), state| {
            if *id == media
                && let State::Reading(reading) = state
            {
                reading.handle.cancel();
            }
            *id != media
        });
    }

    /// Asks every reading still under way to stop (the app is closing).
    pub fn stop(&self) {
        for state in self.streams.values() {
            if let State::Reading(reading) = state {
                reading.handle.cancel();
            }
        }
    }

    /// Collects finished readings and looks at how far the others are.
    pub fn poll(&mut self, ctx: &egui::Context, now: Instant) {
        let mut reading_any = false;
        for state in self.streams.values_mut() {
            let State::Reading(reading) = state else {
                continue;
            };
            if let Some(result) = reading.handle.try_result() {
                *state = match result {
                    Ok(p) => State::Ready(p),
                    Err(err) => {
                        tracing::info!("no waveform: {}", err.report());
                        State::Failed
                    }
                };
                continue;
            }
            reading_any = true;
            reading.state = reading.pace.of_job(reading.handle.progress(), now);
            let due = reading
                .copied_at
                .is_none_or(|at| now.duration_since(at) >= COPY_EVERY);
            // Only ever tries the lock: the worker holds it for an instant
            // at a time, and the next frame will do as well.
            if due && let Ok(all) = reading.so_far.try_lock() {
                let copied = reading.copy.as_ref().map_or(0, |p| p.values.len());
                if all.len() > copied {
                    reading.copy = Some(Peaks {
                        per_second: cache::waveform::PER_SECOND,
                        values: all.as_slice().into(),
                    });
                    reading.copied_at = Some(now);
                }
            }
        }
        if reading_any {
            ctx.request_repaint_after(REDRAW_EVERY);
        }
    }
}

/// The share of a stream `length` seconds long that `peaks` peaks cover.
/// Never quite 1: the reading is done when its result arrives.
fn share_read(peaks: usize, length: f64) -> f32 {
    let secs = peaks as f64 / f64::from(cache::waveform::PER_SECOND);
    (secs / length.max(0.001)).clamp(0.0, 0.99) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaInfo, MediaKind, Time};
    use std::path::Path;

    fn media(id: u64, path: &Path, secs: i64) -> MediaRef {
        MediaRef {
            id: MediaId(id),
            path: path.to_path_buf(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: Time::seconds(secs),
            info: MediaInfo::default(),
        }
    }

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/voice.mp3")
    }

    fn jobs() -> JobSystem {
        JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()))
    }

    /// Polls until `done` says so, for at most ten seconds.
    fn wait(waveforms: &mut Waveforms, ctx: &egui::Context, done: impl Fn(&mut Waveforms) -> bool) {
        let start = Instant::now();
        while !done(waveforms) {
            assert!(start.elapsed() < Duration::from_secs(10), "took too long");
            std::thread::sleep(Duration::from_millis(5));
            waveforms.poll(ctx, Instant::now());
        }
    }

    #[test]
    fn the_share_read_is_the_sound_covered_and_never_quite_all() {
        assert_eq!(share_read(0, 60.0), 0.0);
        assert_eq!(share_read(3_000, 60.0), 0.5);
        assert_eq!(share_read(6_000, 60.0), 0.99, "done is when the result arrives");
        assert_eq!(share_read(100, 0.0), 0.99, "a file with no known length");
    }

    #[test]
    fn a_stream_is_read_once_and_then_ready() {
        let (jobs, ctx) = (jobs(), egui::Context::default());
        let mut waveforms = Waveforms::default();
        let voice = media(1, &fixture(), 2);
        let first = waveforms.get(&jobs, &ctx, &voice, 0);
        assert_eq!(first.state, Some(LoadState::Waiting), "just asked for");
        assert!(first.peaks.is_none() && first.read_secs == 0.0);
        wait(&mut waveforms, &ctx, |w| {
            w.get(&jobs, &ctx, &voice, 0).state.is_none()
        });
        let ready = waveforms.get(&jobs, &ctx, &voice, 0);
        assert!(ready.peaks.is_some_and(|p| p.values.len() > 100));
        assert_eq!(ready.read_secs, f64::INFINITY);
    }

    #[test]
    fn peaks_read_so_far_can_be_drawn_while_the_rest_is_read() {
        let (jobs, ctx) = (jobs(), egui::Context::default());
        // Held at the first run of peaks, as if the file were very long.
        let mut waveforms = Waveforms::holding_at(Some(0.0));
        let voice = media(1, &fixture(), 60);
        waveforms.get(&jobs, &ctx, &voice, 0);
        wait(&mut waveforms, &ctx, |w| {
            w.get(&jobs, &ctx, &voice, 0).peaks.is_some()
        });
        let partly = waveforms.get(&jobs, &ctx, &voice, 0);
        let read = partly.peaks.as_ref().map_or(0, |p| p.values.len());
        assert!(read > 0);
        assert!(
            waveforms.whole(&jobs, &ctx, &voice, 0).is_none(),
            "half a waveform is not handed out as the whole"
        );
        assert_eq!(partly.read_secs, read as f64 / 100.0);
        let Some(LoadState::Reading { done, .. }) = partly.state else {
            panic!("still reading: {:?}", partly.state);
        };
        assert!(done > 0.0 && done < 0.1, "about 1.5 s of a minute: {done}");
        // Forgetting the file stops the held worker.
        waveforms.forget(MediaId(1));
        assert!(waveforms.streams.is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        let (jobs, ctx) = (jobs(), egui::Context::default());
        let mut waveforms = Waveforms::default();
        let gone = media(2, Path::new("/nowhere/gone.mp3"), 2);
        waveforms.get(&jobs, &ctx, &gone, 0);
        wait(&mut waveforms, &ctx, |w| {
            w.get(&jobs, &ctx, &gone, 0).state == Some(LoadState::Failed)
        });
        assert!(waveforms.get(&jobs, &ctx, &gone, 0).peaks.is_none());
    }
}
