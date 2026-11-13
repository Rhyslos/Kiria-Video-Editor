//! The waveforms the timeline draws: read on a worker, then kept for the
//! session, and on disk by the recording's fingerprint
//! (`cache::waveform::Store`), so a recording is read once, not once per
//! session.
//!
//! Every sound stream of a file is read in one pass over it (a long
//! recording with five streams is read once, not five times): asking for
//! one stream's waveform starts the reading of all of them.
//!
//! While a stream is being read the timeline already gets the peaks read so
//! far (the waveform fills in from the left) and how far the reading is.
//! The worker and the interface share the growing list of peaks; the
//! interface only ever tries the lock, and copies the list a few times a
//! second, so it never waits for the worker.

use crate::reading::{self, Pace};
use cache::Peaks;
use cache::waveform::Store;
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
/// The disk kept waveforms may take; the ones used longest ago go first.
const STORE_BUDGET: u64 = 512 * 1024 * 1024;

struct Reading {
    /// The peaks read so far; the worker adds a second of sound or so at a time.
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

/// The reading of one file's streams: one job for all of them.
struct FileReading {
    handle: JobHandle<Vec<Peaks>>,
    /// The streams it reads, in the order of its result.
    streams: Vec<u32>,
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
    /// Readings under way, each of one file's streams.
    files: Vec<(MediaId, FileReading)>,
    /// Where waveforms are kept on disk (`None`: only in memory, in tests).
    store: Option<Store>,
    /// `--hold-reading`: workers stop at this share, for screenshots of reading in progress.
    hold_at: Option<f32>,
}

impl Waveforms {
    /// Waveforms kept on disk in `folder` too (`None`: in memory only).
    /// With `--hold-reading`: workers stop where the share read reaches `hold_at`.
    pub fn new(folder: Option<std::path::PathBuf>, hold_at: Option<f32>) -> Waveforms {
        Waveforms {
            store: folder.map(Store::new),
            hold_at,
            ..Default::default()
        }
    }

    /// Trims the waveforms kept on disk to their budget, on a worker.
    pub fn trim(&self, jobs: &JobSystem) {
        if let Some(store) = self.store.clone() {
            // Nobody waits for it: the handle is let go.
            drop(jobs.submit(
                PoolKind::Analysis,
                Priority::Background,
                "waveform trim",
                move |_| {
                    let removed = store.trim(STORE_BUDGET);
                    if removed > 0 {
                        tracing::info!("{removed} kept waveforms used longest ago removed");
                    }
                    Ok(())
                },
            ));
        }
    }

    /// Removes every waveform kept on disk, on a worker (Settings › Cache),
    /// and forgets the ones in memory: they are read again as needed.
    pub fn clear_kept(&mut self, jobs: &JobSystem) {
        for (_, reading) in &self.files {
            reading.handle.cancel();
        }
        self.files.clear();
        self.streams.clear();
        if let Some(store) = self.store.clone() {
            // Nobody waits for it: the handle is let go.
            drop(jobs.submit_write(
                PoolKind::Analysis,
                Priority::Interactive,
                "clear waveforms",
                move |_| {
                    let removed = store.clear();
                    tracing::info!("{removed} kept waveforms cleared");
                    Ok(())
                },
            ));
        }
    }

    /// What is known of this stream's waveform; starts reading it (with the
    /// file's other streams) if nobody has yet.
    pub fn get(&mut self, jobs: &JobSystem, ctx: &egui::Context, media: &MediaRef, stream: u32) -> Look {
        if !self.streams.contains_key(&(media.id, stream)) {
            self.read(jobs, ctx, media, stream);
        }
        match self.streams.get(&(media.id, stream)) {
            Some(State::Ready(peaks)) => Look {
                peaks: Some(peaks.clone()),
                read_secs: f64::INFINITY,
                state: None,
            },
            Some(State::Reading(reading)) => Look {
                read_secs: reading
                    .copy
                    .as_ref()
                    .map_or(0.0, |p| p.values.len() as f64 / f64::from(p.per_second)),
                peaks: reading.copy.clone(),
                state: Some(reading.state),
            },
            Some(State::Failed) | None => Look {
                peaks: None,
                read_secs: 0.0,
                state: Some(LoadState::Failed),
            },
        }
    }

    /// Starts one reading of every sound stream of `media` not read or
    /// being read yet (`stream` among them), from disk where kept.
    fn read(&mut self, jobs: &JobSystem, ctx: &egui::Context, media: &MediaRef, stream: u32) {
        let mut streams: Vec<u32> = media.info.audio.iter().map(|a| a.index).collect();
        if !streams.contains(&stream) {
            streams.push(stream);
        }
        streams.retain(|s| !self.streams.contains_key(&(media.id, *s)));
        if streams.is_empty() {
            return;
        }
        let shared: Vec<Arc<Mutex<Vec<f32>>>> = streams.iter().map(|_| Arc::default()).collect();
        for (s, so_far) in streams.iter().zip(&shared) {
            self.streams.insert(
                (media.id, *s),
                State::Reading(Reading {
                    so_far: Arc::clone(so_far),
                    copy: None,
                    copied_at: None,
                    pace: Pace::default(),
                    state: LoadState::Waiting,
                }),
            );
        }
        let (path, ctx, hold_at) = (media.path.clone(), ctx.clone(), self.hold_at);
        let (store, fingerprint) = (self.store.clone(), media.fingerprint.clone());
        let length = media.duration.as_secs_f64().max(0.001);
        let which = streams.clone();
        let handle = jobs.submit(PoolKind::Analysis, Priority::Background, "waveform", move |job| {
            let mut found: Vec<Option<Peaks>> = which
                .iter()
                .map(|s| store.as_ref().and_then(|store| store.load(&fingerprint, *s)))
                .collect();
            for (peaks, so_far) in found.iter().zip(&shared) {
                if let (Some(peaks), Ok(mut all)) = (peaks, so_far.lock()) {
                    all.extend_from_slice(&peaks.values);
                }
            }
            let left: Vec<usize> = (0..which.len()).filter(|i| found[*i].is_none()).collect();
            if !left.is_empty() {
                let wanted: Vec<u32> = left.iter().map(|i| which[*i]).collect();
                let mut read = vec![0usize; wanted.len()];
                let result = cache::waveform::peaks_of_streams(&path, &wanted, |i, more| {
                    read[i] += more.len();
                    if let Ok(mut all) = shared[left[i]].lock() {
                        all.extend_from_slice(more);
                    }
                    let done = read.iter().map(|r| share_read(*r, length)).fold(1.0f32, f32::min);
                    job.set_progress(done);
                    reading::hold(hold_at, done, job);
                    !job.is_cancelled()
                });
                ctx.request_repaint();
                for (i, peaks) in left.iter().zip(result?) {
                    if let Some(store) = &store {
                        store.save(&fingerprint, which[*i], &peaks);
                    }
                    found[*i] = Some(peaks);
                }
            }
            ctx.request_repaint();
            Ok(found.into_iter().flatten().collect())
        });
        self.files.push((media.id, FileReading { handle, streams }));
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
        self.files.retain(|(id, reading)| {
            if *id == media {
                reading.handle.cancel();
            }
            *id != media
        });
        self.streams.retain(|(id, _), _| *id != media);
    }

    /// Asks every reading still under way to stop (the app is closing).
    pub fn stop(&self) {
        for (_, reading) in &self.files {
            reading.handle.cancel();
        }
    }

    /// Every waveform asked for is read (or couldn't be), or readings are
    /// held on purpose (`--hold-reading`): what a screenshot waits for.
    pub fn settled(&self) -> bool {
        self.hold_at.is_some() || !self.streams.values().any(|s| matches!(s, State::Reading(_)))
    }

    /// Collects finished readings and looks at how far the others are.
    pub fn poll(&mut self, ctx: &egui::Context, now: Instant) {
        let mut still = Vec::with_capacity(self.files.len());
        for (media, reading) in std::mem::take(&mut self.files) {
            let Some(result) = reading.handle.try_result() else {
                still.push((media, reading));
                continue;
            };
            if let Err(err) = &result {
                tracing::info!("no waveform: {}", err.report());
            }
            for (i, stream) in reading.streams.iter().enumerate() {
                let state = match &result {
                    Ok(all) => all.get(i).cloned().map_or(State::Failed, State::Ready),
                    Err(_) => State::Failed,
                };
                self.streams.insert((media, *stream), state);
            }
        }
        self.files = still;
        let mut reading_any = false;
        for ((media, stream), state) in self.streams.iter_mut() {
            let State::Reading(reading) = state else {
                continue;
            };
            reading_any = true;
            if let Some((_, file)) = self
                .files
                .iter()
                .find(|(id, f)| id == media && f.streams.contains(stream))
            {
                reading.state = reading.pace.of_job(file.handle.progress(), now);
            }
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
        let mut waveforms = Waveforms::new(None, Some(0.0));
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
    fn every_stream_of_a_file_comes_from_one_reading_and_then_from_disk() {
        let (jobs, ctx) = (jobs(), egui::Context::default());
        let dir = tempfile::tempdir().unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let mut game = media(3, &path, 2);
        game.info = media::probe(&path).unwrap().info;
        let mut waveforms = Waveforms::new(Some(dir.path().join("waveforms")), None);
        waveforms.get(&jobs, &ctx, &game, 1);
        assert_eq!(waveforms.files.len(), 1, "one reading");
        assert_eq!(waveforms.streams.len(), 3, "of all three streams");
        wait(&mut waveforms, &ctx, |w| w.files.is_empty());
        let peaks: Vec<Peaks> = [1, 2, 3]
            .into_iter()
            .map(|s| waveforms.get(&jobs, &ctx, &game, s).peaks.unwrap())
            .collect();
        assert!(waveforms.files.is_empty(), "nothing read again");
        // Another session: from disk, the same within 16-bit steps.
        let mut again = Waveforms::new(Some(dir.path().join("waveforms")), None);
        again.get(&jobs, &ctx, &game, 2);
        wait(&mut again, &ctx, |w| w.files.is_empty());
        for (s, before) in [1, 2, 3].into_iter().zip(&peaks) {
            let kept = again.get(&jobs, &ctx, &game, s).peaks.unwrap();
            assert_eq!(kept.values.len(), before.values.len());
            assert!(
                kept.values
                    .iter()
                    .zip(before.values.iter())
                    .all(|(a, b)| (a - b).abs() < 1e-4)
            );
        }
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
