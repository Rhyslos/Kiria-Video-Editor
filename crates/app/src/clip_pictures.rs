//! The small pictures drawn inside video clips: read once per file on a
//! worker, then uploaded a few at a time so the interface never stalls.

use cache::thumbs::Strip;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, MediaRef};
use std::collections::HashMap;
use std::sync::Arc;

/// Pictures uploaded per frame while a file's strip arrives.
const UPLOADS_PER_FRAME: usize = 24;

enum State {
    Reading(JobHandle<Strip>),
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
}

impl ClipPictures {
    /// The pictures of this file's video stream, if ready (with `source_in`
    /// left at zero for the caller to set); starts reading them otherwise.
    pub fn get(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        media: &MediaRef,
        stream: u32,
    ) -> Option<ui_kit::ClipPictures> {
        let key = (media.id, stream);
        let state = self.files.entry(key).or_insert_with(|| {
            let (path, ctx) = (media.path.clone(), ctx.clone());
            let rotation = media.info.video.as_ref().map_or(0, |v| v.rotation);
            let duration = media.duration.as_secs_f64();
            State::Reading(jobs.submit(
                PoolKind::Analysis,
                Priority::Background,
                "clip pictures",
                move |job| {
                    let result =
                        cache::thumbs::strip(&path, stream, rotation, duration, || job.is_cancelled());
                    ctx.request_repaint();
                    result
                },
            ))
        });
        match state {
            State::Ready { pictures, .. } => Some(pictures.clone()),
            _ => None,
        }
    }

    /// Collects finished readings and uploads a few pictures.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for ((media, _), state) in self.files.iter_mut() {
            if let State::Reading(handle) = state
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
        self.files.retain(|(id, _), _| *id != media);
    }
}
