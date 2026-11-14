//! The small pictures drawn inside video clips: read once per file on a
//! worker (or read back from the cache folder's `pictures/`, where they are
//! kept by the recording's fingerprint), then uploaded a few at a time so
//! the interface never stalls. A Library card's one resting picture (its
//! cover) is read on its own, ahead of the strip, so cards fill quickly.

use crate::reading::{self, Pace};
use cache::thumbs::Strip;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, MediaRef};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui_kit::timeline::LoadState;

/// Pictures uploaded per frame while a file's strip arrives.
const UPLOADS_PER_FRAME: usize = 24;

/// What the kept pictures may take on disk; those used longest ago go first.
const STORE_BUDGET: u64 = 512 * 1024 * 1024;

/// How often the timeline is redrawn while pictures are being read.
const REDRAW_EVERY: Duration = Duration::from_millis(100);

enum State {
    Reading {
        handle: JobHandle<Strip>,
        pace: Pace,
        /// How far the reading was when last looked at.
        state: LoadState,
    },
    Uploading {
        strip: Strip,
        done: Vec<egui::TextureHandle>,
    },
    Ready {
        pictures: ui_kit::ClipPictures,
        /// Keeps the textures alive while the ids are in use.
        _textures: Vec<egui::TextureHandle>,
    },
    /// Couldn't be read: the clip is drawn without pictures.
    Failed,
}

/// A file's cover: the one picture its Library card rests on.
enum Cover {
    Reading(JobHandle<media::Frame>),
    Ready(egui::TextureHandle, f32),
    Failed,
}

#[derive(Default)]
pub struct ClipPictures {
    files: HashMap<(MediaId, u32), State>,
    covers: HashMap<(MediaId, u32), Cover>,
    /// `--hold-reading`: workers stop at this share, for screenshots of reading in progress.
    hold_at: Option<f32>,
    /// Exact frames, not the nearest key frames (Settings › Editing).
    exact: bool,
    /// Where strips are kept between sessions; none in tests.
    store: Option<cache::thumbs::Store>,
}

impl ClipPictures {
    /// With `--hold-reading`: workers stop where the share read reaches `hold_at`.
    pub fn holding_at(hold_at: Option<f32>) -> ClipPictures {
        ClipPictures {
            hold_at,
            ..Default::default()
        }
    }

    /// Keeps strips in `folder` between sessions, and trims those kept to
    /// their budget on a worker.
    pub fn keep_in(&mut self, folder: std::path::PathBuf, jobs: &JobSystem) {
        let store = cache::thumbs::Store::new(folder);
        let trimmed = store.clone();
        // Nobody waits for it: the handle is let go.
        drop(jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "clip pictures trim",
            move |_| {
                let removed = trimmed.trim(STORE_BUDGET);
                if removed > 0 {
                    tracing::info!("{removed} kept clip pictures used longest ago removed");
                }
                Ok(())
            },
        ));
        self.store = Some(store);
    }

    /// Removes every strip kept on disk, on a worker (Settings › Cache), and
    /// forgets those in memory: they are read again as needed.
    pub fn clear_kept(&mut self, jobs: &JobSystem) {
        self.stop();
        self.files.clear();
        self.covers.clear();
        if let Some(store) = self.store.clone() {
            drop(jobs.submit_write(
                PoolKind::Analysis,
                Priority::Interactive,
                "clear clip pictures",
                move |_| {
                    let removed = store.clear();
                    tracing::info!("{removed} kept clip pictures cleared");
                    Ok(())
                },
            ));
        }
    }

    /// The pictures of this file's video stream, if ready (with `source_in`
    /// left at zero for the caller to set), and otherwise how the reading
    /// is going; starts reading them if nobody has yet.
    pub fn get(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> (Option<ui_kit::ClipPictures>, Option<LoadState>) {
        let key = (media.id, stream);
        let (hold_at, exact) = (self.hold_at, self.exact);
        let state = self.files.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let rotation = media.info.video.as_ref().map_or(0, |v| v.rotation);
            let duration = media.duration.as_secs_f64();
            let (store, fingerprint) = (self.store.clone(), media.fingerprint.clone());
            let handle = jobs.submit(
                PoolKind::Analysis,
                Priority::Background,
                "clip pictures",
                move |job| {
                    if let Some(kept) = store.as_ref().and_then(|s| s.load(&fingerprint, stream, exact)) {
                        ctx.request_repaint();
                        return Ok(kept);
                    }
                    let result = cache::thumbs::strip(
                        &path,
                        stream,
                        rotation,
                        duration,
                        exact,
                        || job.is_cancelled(),
                        |done| {
                            // Never quite 1: done is when the pictures are on show.
                            job.set_progress(done * 0.99);
                            reading::hold(hold_at, done, job);
                        },
                    );
                    // A strip cut short by a cancel is not kept.
                    if let (Ok(strip), Some(store)) = (&result, &store)
                        && !job.is_cancelled()
                        && !strip.frames.is_empty()
                    {
                        store.save(&fingerprint, stream, exact, strip);
                    }
                    ctx.request_repaint();
                    result
                },
            );
            State::Reading {
                handle,
                pace: Pace::default(),
                state: LoadState::Waiting,
            }
        });
        match state {
            State::Ready { pictures, .. } => (Some(pictures.clone()), None),
            State::Reading { state, .. } => (None, Some(*state)),
            // Read; a few frames more and they are on show.
            State::Uploading { .. } => (
                None,
                Some(LoadState::Reading {
                    done: 1.0,
                    seconds_left: None,
                }),
            ),
            State::Failed => (None, Some(LoadState::Failed)),
        }
    }

    /// The picture a Library card of this file rests on, with its width
    /// over its height, once read; starts reading it if nobody has yet,
    /// ahead of the file's strip (or reads it back from the cache folder).
    pub fn cover(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> Option<(egui::TextureId, f32)> {
        let key = (media.id, stream);
        let cover = self.covers.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let rotation = media.info.video.as_ref().map_or(0, |v| v.rotation);
            let duration = media.duration.as_secs_f64();
            let (store, fingerprint) = (self.store.clone(), media.fingerprint.clone());
            Cover::Reading(jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "card picture",
                move |_| {
                    let kept = store.as_ref().and_then(|s| s.load_cover(&fingerprint, stream));
                    let result =
                        kept.map_or_else(|| cache::thumbs::cover(&path, stream, rotation, duration), Ok);
                    if let (Ok(frame), Some(store)) = (&result, &store) {
                        store.save_cover(&fingerprint, stream, frame);
                    }
                    ctx.request_repaint();
                    result
                },
            ))
        });
        if let Cover::Reading(handle) = cover
            && let Some(result) = handle.try_result()
        {
            *cover = match result {
                Ok(frame) => {
                    let size = [frame.width as usize, frame.height as usize];
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
                    let name = format!("card-picture-{}-{stream}", media.id.0);
                    let aspect = frame.width as f32 / frame.height.max(1) as f32;
                    Cover::Ready(
                        ctx.load_texture(name, image, egui::TextureOptions::LINEAR),
                        aspect,
                    )
                }
                Err(err) => {
                    tracing::info!("no card picture: {}", err.report());
                    Cover::Failed
                }
            };
        }
        match cover {
            Cover::Ready(texture, aspect) => Some((texture.id(), *aspect)),
            Cover::Reading(_) | Cover::Failed => None,
        }
    }

    /// Asks every reading still under way to stop (the app is closing).
    pub fn stop(&self) {
        for state in self.files.values() {
            if let State::Reading { handle, .. } = state {
                handle.cancel();
            }
        }
        for cover in self.covers.values() {
            if let Cover::Reading(handle) = cover {
                handle.cancel();
            }
        }
    }

    /// Every clip's pictures asked for are ready (or couldn't be read), or
    /// readings are held on purpose (`--hold-reading`): what a screenshot
    /// waits for.
    pub fn settled(&self) -> bool {
        self.hold_at.is_some()
            || !self
                .files
                .values()
                .any(|s| matches!(s, State::Reading { .. } | State::Uploading { .. }))
                && !self.covers.values().any(|c| matches!(c, Cover::Reading(_)))
    }

    /// Collects finished readings, looks at how far the others are, and
    /// uploads a few pictures.
    pub fn poll(&mut self, ctx: &egui::Context, now: Instant) {
        for ((media, _), state) in self.files.iter_mut() {
            if let State::Reading { handle, pace, state } = state {
                *state = pace.of_job(handle.progress(), now);
                ctx.request_repaint_after(REDRAW_EVERY);
            }
            if let State::Reading { handle, .. } = state
                && let Some(result) = handle.try_result()
            {
                *state = match result {
                    Ok(strip) if !strip.frames.is_empty() => State::Uploading {
                        strip,
                        done: Vec::new(),
                    },
                    Ok(_) => State::Failed,
                    Err(err) => {
                        tracing::info!("no clip pictures: {}", err.report());
                        State::Failed
                    }
                };
            }
            if let State::Uploading { strip, done } = state {
                for frame in strip.frames.iter().skip(done.len()).take(UPLOADS_PER_FRAME) {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    );
                    let name = format!("clip-picture-{}-{}", media.0, done.len());
                    done.push(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
                }
                if done.len() < strip.frames.len() {
                    ctx.request_repaint();
                } else {
                    let first = &strip.frames[0];
                    let pictures = ui_kit::ClipPictures {
                        every: strip.every,
                        textures: done.iter().map(egui::TextureHandle::id).collect::<Arc<[_]>>(),
                        aspect: first.width as f32 / first.height.max(1) as f32,
                        source_in: 0.0,
                        speed: 1.0,
                        map: None,
                    };
                    *state = State::Ready {
                        pictures,
                        _textures: std::mem::take(done),
                    };
                }
            }
        }
    }

    /// Takes exact frames, or the nearest key frames in long recordings;
    /// a change reads every file's pictures again the new way.
    pub fn set_exact(&mut self, exact: bool) {
        if exact != self.exact {
            self.exact = exact;
            for state in self.files.values() {
                if let State::Reading { handle, .. } = state {
                    handle.cancel();
                }
            }
            self.files.clear();
        }
    }

    /// Reads this file's pictures again next time (it was found somewhere else).
    pub fn forget(&mut self, media: MediaId) {
        self.files.retain(|(id, _), state| {
            if *id == media
                && let State::Reading { handle, .. } = state
            {
                handle.cancel();
            }
            *id != media
        });
        self.covers.retain(|(id, _), cover| {
            if *id == media
                && let Cover::Reading(handle) = cover
            {
                handle.cancel();
            }
            *id != media
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asks for `media`'s pictures until they are ready.
    fn ready(pictures: &mut ClipPictures, jobs: &JobSystem, media: &MediaRef) -> ui_kit::ClipPictures {
        let ctx = egui::Context::default();
        let index = media.info.video.as_ref().unwrap().index;
        let give_up = Instant::now() + Duration::from_secs(60);
        loop {
            pictures.poll(&ctx, Instant::now());
            if let (Some(ready), _) = pictures.get(jobs, &ctx, media, index) {
                return ready;
            }
            assert!(Instant::now() < give_up, "ready in time");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_files_pictures_are_kept_for_the_next_session() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let media = media::describe(&fixture).unwrap();
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("pictures");
        let mut first = ClipPictures::default();
        first.keep_in(folder.clone(), &jobs);
        let read = ready(&mut first, &jobs, &media);
        // The next session: the recording is gone, its pictures are not.
        let mut gone = media.clone();
        gone.path = dir.path().join("moved away.mkv");
        let mut next = ClipPictures::default();
        next.keep_in(folder.clone(), &jobs);
        let kept = ready(&mut next, &jobs, &gone);
        assert_eq!(
            (kept.every, kept.textures.len()),
            (read.every, read.textures.len())
        );
        // Cleared, they are read again (and here can't be).
        next.clear_kept(&jobs);
        let ctx = egui::Context::default();
        let index = media.info.video.as_ref().unwrap().index;
        let give_up = Instant::now() + Duration::from_secs(60);
        loop {
            next.poll(&ctx, Instant::now());
            if let (_, Some(LoadState::Failed)) = next.get(&jobs, &ctx, &gone, index) {
                break;
            }
            assert!(Instant::now() < give_up, "read again, and failed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_card_picture_is_read_on_its_own_and_kept() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let media = media::describe(&fixture).unwrap();
        let index = media.info.video.as_ref().unwrap().index;
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("pictures");
        let ctx = egui::Context::default();
        let cover = |pictures: &mut ClipPictures, media: &MediaRef| {
            let give_up = Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(cover) = pictures.cover(&jobs, &ctx, media, index) {
                    return cover;
                }
                assert!(Instant::now() < give_up, "ready in time");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let mut first = ClipPictures::default();
        first.keep_in(folder.clone(), &jobs);
        let (_, aspect) = cover(&mut first, &media);
        assert!(
            !first.files.contains_key(&(media.id, index)),
            "the strip is not read for it"
        );
        assert!(first.settled());
        // The next session reads it back, the recording gone.
        let mut gone = media.clone();
        gone.path = dir.path().join("moved away.mkv");
        let mut next = ClipPictures::default();
        next.keep_in(folder, &jobs);
        assert_eq!(cover(&mut next, &gone).1, aspect);
    }

    #[test]
    fn switching_between_quick_and_exact_reads_the_pictures_again() {
        let mut pictures = ClipPictures::default();
        pictures.files.insert((MediaId(1), 0), State::Failed);
        pictures.set_exact(false);
        assert_eq!(pictures.files.len(), 1, "no change, nothing read again");
        pictures.set_exact(true);
        assert!(pictures.files.is_empty(), "read again the exact way");
        assert!(pictures.exact);
    }
}
