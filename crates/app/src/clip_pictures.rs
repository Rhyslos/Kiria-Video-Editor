//! The small pictures drawn inside video clips: read once per file on a
//! worker, then uploaded a few at a time so the interface never stalls.

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

#[derive(Default)]
pub struct ClipPictures {
    files: HashMap<(MediaId, u32), State>,
    /// `--hold-reading`: workers stop at this share, for screenshots of reading in progress.
    hold_at: Option<f32>,
}

impl ClipPictures {
    /// With `--hold-reading`: workers stop where the share read reaches `hold_at`.
    pub fn holding_at(hold_at: Option<f32>) -> ClipPictures {
        ClipPictures {
            hold_at,
            ..Default::default()
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
        let hold_at = self.hold_at;
        let state = self.files.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let rotation = media.info.video.as_ref().map_or(0, |v| v.rotation);
            let duration = media.duration.as_secs_f64();
            let handle = jobs.submit(
                PoolKind::Analysis,
                Priority::Background,
                "clip pictures",
                move |job| {
                    let result = cache::thumbs::strip(
                        &path,
                        stream,
                        rotation,
                        duration,
                        || job.is_cancelled(),
                        |done| {
                            // Never quite 1: done is when the pictures are on show.
                            job.set_progress(done * 0.99);
                            reading::hold(hold_at, done, job);
                        },
                    );
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

    /// Asks every reading still under way to stop (the app is closing).
    pub fn stop(&self) {
        for state in self.files.values() {
            if let State::Reading { handle, .. } = state {
                handle.cancel();
            }
        }
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
    }
}
