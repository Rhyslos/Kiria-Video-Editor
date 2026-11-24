//! The timeline's whole picture put together on the window's device:
//! every layer decoded, titles drawn, a blended layer's next frame.

use super::waiting::Waiting;
use super::{File, Picture, Request, Role, Target, Worker};
use crate::compose::{Composer, NotComposed};
use crate::frame::{self, FrameDescription};
use media::Decoded;
use project::Time;
use render::Area;
use std::sync::Arc;
use std::time::Instant;

impl Worker {
    /// The timeline's whole picture for `r`: every layer that is seen,
    /// decoded and put together on the window's device.
    pub(super) fn composed(
        &mut self,
        r: &Request,
        sequence: &project::Sequence,
        waiting: &mut Waiting<'_>,
        give_up: bool,
        started: Instant,
    ) -> Option<Picture> {
        let size = preview_size(
            (sequence.settings.width, sequence.settings.height),
            (r.max_width, r.max_height),
        );
        let whole = frame::describe(&r.project, sequence, r.at);
        let mut out = Picture::black(r.at);
        if whole.layers.is_empty() {
            return Some(out); // a gap: black
        }
        if let Some(picture) = self.prerendered(r, &whole, size, started) {
            return Some(picture);
        }
        let mut description = whole.at_size(size);
        // One layer is the usual case and behaves as a picture alone does:
        // given up for a newer request, or carried on to it. With more, each
        // is decoded to the end, so they all show the same moment.
        let alone = description.layers.len() == 1;
        let top = description.layers.len() - 1;
        let mut pictures = Vec::with_capacity(description.layers.len());
        for (index, layer) in description.layers.iter().enumerate() {
            if let Some(words) = &layer.title {
                let drawn = self.title_picture(words, title_size(layer));
                pictures.push((index, Decoded::Rgba(drawn), None));
                continue;
            }
            let Some(media) = r.project.media.get(&layer.media) else {
                continue;
            };
            let target = Target {
                key: (layer.media, layer.stream),
                path: &media.path,
                fingerprint: &media.fingerprint,
                rotation: media.info.video.as_ref().map_or(0, |v| v.rotation),
                moment: layer.source_time.as_secs_f64(),
                size: pixels_needed(layer),
            };
            let role = Role {
                leads: index == top,
                follows: alone,
                give_up: alone && give_up,
            };
            let mut got = self.decoded(r.at, &target, waiting, role)?;
            if let Some(decoded) = got.frame.take() {
                out.converted_on_cpu |= matches!(decoded, Decoded::Rgba(_));
                let next = match layer.blend {
                    Some(blend) => self.next_frame(&target, blend, &decoded, waiting)?,
                    None => None,
                };
                pictures.push((index, decoded, next));
            }
            // The corner and Diagnostics describe the top layer; any
            // layer's problem is told.
            if index == top {
                out.at = got.at;
                out.work = got.work;
                out.method = got.method;
            }
            out.error = out.error.or(got.error);
        }
        // Reached a newer request's moment on the way: the layer sits where
        // it does at that moment.
        if out.at != r.at {
            let there = frame::describe(&r.project, sequence, out.at).at_size(size);
            if same_layers(&there, &description) {
                description = there;
            }
        }
        if pictures.is_empty() {
            out.took = started.elapsed();
            return Some(out);
        }
        let composing = self.composing.clone()?;
        let composer = self.composer.get_or_insert_with(|| Composer::new(composing));
        match composer.compose(&description, pictures) {
            Ok(composed) => {
                if r.scope {
                    out.sampled = composer.sample(&composed, SCOPE_WIDTH).map(Arc::new);
                }
                out.composed = Some(composed);
            }
            Err(NotComposed::Refused) => out.refused = true,
            // Told for this frame; the worker carries on with the next.
            Err(NotComposed::Graphics(err)) => out.error = Some(err),
        }
        out.took = started.elapsed();
        Some(out)
    }

    /// The frame for `r` from the render cache, when it was pre-rendered
    /// as `whole` shows it: one picture filling the frame, put together
    /// as any other, so it looks as it would.
    fn prerendered(
        &mut self,
        r: &Request,
        whole: &FrameDescription,
        size: (u32, u32),
        started: Instant,
    ) -> Option<Picture> {
        let store = r.renders.as_ref().filter(|store| !store.is_empty())?;
        let key = crate::export::prerender::frame_key(&r.project, whole)?;
        let kept = store.get(key)?;
        let shown = FrameDescription::filled(size);
        let composing = self.composing.clone()?;
        let composer = self.composer.get_or_insert_with(|| Composer::new(composing));
        let mut out = Picture::black(r.at);
        match composer.compose(&shown, vec![(0, Decoded::Rgba(kept), None)]) {
            Ok(composed) => {
                if r.scope {
                    out.sampled = composer.sample(&composed, SCOPE_WIDTH).map(Arc::new);
                }
                out.composed = Some(composed);
            }
            // Put together the usual way instead.
            Err(_) => return None,
        }
        out.method = Some(PRERENDERED.to_owned());
        out.took = started.elapsed();
        // What was read ahead belongs to the files, not this frame.
        self.playhead = None;
        Some(out)
    }

    /// A title drawn at `size`: as it was last time when it is the same,
    /// else drawn now and kept.
    pub(super) fn title_picture(
        &mut self,
        words: &crate::title::TitleLayer,
        size: (u32, u32),
    ) -> media::Frame {
        if let Some(i) = self.titles.iter().position(|(w, s, _)| w == words && *s == size)
            && let Some(kept) = self.titles.remove(i)
        {
            let frame = kept.2.clone();
            self.titles.push_front(kept);
            return frame;
        }
        let frame = crate::title::draw(words, size);
        self.titles.push_front((words.clone(), size, frame.clone()));
        self.titles.truncate(TITLES_KEPT);
        frame
    }

    /// For frame blending: the recorded frame after `shown` (the one on
    /// screen at `target`'s moment), from the frames kept or decoded on
    /// from it, which is one more frame for the decoder. Nothing when the
    /// file has no later frame. `None` only when the worker is closing.
    pub(super) fn next_frame(
        &mut self,
        target: &Target<'_>,
        blend: frame::Blend,
        shown: &Decoded,
        waiting: &mut Waiting<'_>,
    ) -> Option<Option<Decoded>> {
        let next = Target {
            moment: blend.next.as_secs_f64(),
            ..*target
        };
        let file = File {
            path: target.path,
            stream: target.key.1,
            rotation: target.rotation,
        };
        let got = self.decoded_below(Picture::black(Time::ZERO), &next, &file, waiting)?;
        // The same frame again (the end of the file, or a file whose frames
        // are not where its rate says): nothing to blend.
        Some(got.frame.filter(|frame| frame.time() > shown.time()))
    }
}

/// The size the timeline's picture is put together at for a viewer of
/// `room` pixels: the timeline's shape, as large as fits the room, never
/// larger than the timeline's own size.
pub(super) fn preview_size(timeline: (u32, u32), room: (u32, u32)) -> (u32, u32) {
    let fitted = Area::fitted(timeline, (room.0.max(1), room.1.max(1)));
    let (w, h) = fitted.size();
    let (w, h) = (
        (w.round() as u32).min(timeline.0),
        (h.round() as u32).min(timeline.1),
    );
    (w.max(2), h.max(2))
}

/// The size a layer's whole picture is needed at for the part shown to
/// have a pixel for every pixel of its area.
pub(super) fn pixels_needed(layer: &frame::Layer) -> (u32, u32) {
    let (w, h) = layer.area.size();
    let whole = |shown: f32, part: f32| (shown / part.clamp(0.01, 1.0)).ceil().clamp(2.0, 16_384.0) as u32;
    (whole(w, layer.part.width), whole(h, layer.part.height))
}

/// How a picture from the render cache is described (the viewer's corner).
pub(crate) const PRERENDERED: &str = "pre-rendered";

/// How wide the copy read back for the scopes is.
pub(super) const SCOPE_WIDTH: u32 = 320;

/// How many drawn titles are kept.
pub(super) const TITLES_KEPT: usize = 4;

/// The pixels a title's picture is drawn at: the size its part of the
/// frame takes on screen.
pub(super) fn title_size(layer: &frame::Layer) -> (u32, u32) {
    let (w, h) = layer.area.size();
    (
        w.ceil().clamp(1.0, 8192.0) as u32,
        h.ceil().clamp(1.0, 8192.0) as u32,
    )
}

/// Whether two descriptions show the same clips' files, layer for layer.
pub(super) fn same_layers(a: &FrameDescription, b: &FrameDescription) -> bool {
    a.layers.len() == b.layers.len()
        && a.layers
            .iter()
            .zip(&b.layers)
            .all(|(x, y)| (x.media, x.stream) == (y.media, y.stream))
}
