//! A first picture before the exact one: a kept frame, a proxy frame or
//! the key frame before the moment.

use super::composed::{pixels_needed, preview_size};
use super::{Convert, KEPT, Picture, Request, Target, Worker};
use crate::compose::Composer;
use crate::frame;
use crate::timeline::video_at;
use cache::proxy::ProxyMode;
use media::{Decoded, Decoding, Level, VideoDecoder};
use std::time::Instant;

/// What the viewer's corner says for a proxy frame shown first.
pub(crate) const PROXY_FIRST: &str = "proxy frame, sharpens when the playhead rests";
/// What it says for a proxy frame with proxies On.
pub(crate) const PROXY_ONLY: &str = "proxy frame (proxies On)";
/// What it says for a key frame shown first.
pub(crate) const KEY_FIRST: &str = "key frame before it, sharpening";

/// A key frame is only shown first when the moment is not this little
/// (seconds) behind the frame the decoder holds: for a step or a short drag
/// back, a picture from up to a key frame distance earlier is more
/// confusing than waiting for the right one.
pub(super) const GLANCE_BEHIND: f64 = 0.5;

/// Where a layer's first picture came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Quick {
    Kept,
    Proxy,
    KeyFrame,
}

impl Worker {
    /// A first picture for `r`, when its exact picture can't be had at
    /// once: some layer isn't kept, not near its decoder's place, and (with
    /// proxies On) has no proxy to be shown from. Each layer then comes
    /// kept, from its proxy, or as the key frame before its moment (unless
    /// key frames first are off). `None` when the exact picture is quick,
    /// while playing, or when some layer has no first picture to give. The flag says whether a proxy gave
    /// one (a newer request then gets a first picture at once, too).
    pub(super) fn stand_in(
        &mut self,
        r: &Request,
        sequence: &project::Sequence,
        started: Instant,
    ) -> Option<(Picture, bool)> {
        if self.playing {
            return None;
        }
        let mut out = Picture::black(r.at);
        let quick;
        if self.composing.is_some() {
            let size = preview_size(
                (sequence.settings.width, sequence.settings.height),
                (r.max_width, r.max_height),
            );
            let description = frame::describe(&r.project, sequence, r.at).at_size(size);
            let mut targets = Vec::with_capacity(description.layers.len());
            for layer in &description.layers {
                let media = r.project.media.get(&layer.media)?;
                targets.push(Target {
                    key: (layer.media, layer.stream),
                    path: &media.path,
                    fingerprint: &media.fingerprint,
                    rotation: media.info.video.as_ref().map_or(0, |v| v.rotation),
                    moment: layer.source_time.as_secs_f64(),
                    size: pixels_needed(layer),
                });
            }
            let (pictures, came) = self.quick_layers(&targets)?;
            quick = came;
            let pictures = pictures
                .into_iter()
                .enumerate()
                .map(|(index, picture)| (index, picture, None))
                .collect();
            let composing = self.composing.clone()?;
            let composer = self.composer.get_or_insert_with(|| Composer::new(composing));
            out.composed = Some(composer.compose(&description, pictures).ok()?);
        } else {
            let showing = video_at(&r.project, sequence, r.at)?;
            let target = Target {
                key: (showing.media.id, showing.clip.stream),
                path: &showing.media.path,
                fingerprint: &showing.media.fingerprint,
                rotation: showing.media.info.video.as_ref().map_or(0, |v| v.rotation),
                moment: showing.source_time.as_secs_f64(),
                size: (r.max_width, r.max_height),
            };
            let (mut pictures, came) = self.quick_layers(&[target])?;
            quick = came;
            out.frame = pictures.pop();
        }
        // The corner describes the top layer.
        out.method = quick.last().map(|q| q.says().to_owned());
        out.stand_in = true;
        out.took = started.elapsed();
        Some((out, quick.contains(&Quick::Proxy)))
    }

    /// Pictures for these layers to be had at once, when at least one of
    /// them can't be decoded at once; with where each came from. `None`
    /// when none is needed or some layer has nothing quick to give.
    pub(super) fn quick_layers(&mut self, targets: &[Target<'_>]) -> Option<(Vec<Decoded>, Vec<Quick>)> {
        let proxies_on = self.proxies.mode() == ProxyMode::On;
        let mut slow = false;
        for target in targets {
            let kept = self.frames.has(target.key, target.moment);
            let near = self
                .decoders
                .iter()
                .find(|(key, _)| *key == target.key)
                .is_some_and(|(_, decoder)| decoder.is_near(target.moment));
            let proxy_only = proxies_on && self.proxies.of(target.key, target.fingerprint).is_some();
            slow |= !kept && !near && !proxy_only;
        }
        if !slow {
            return None;
        }
        let mut pictures = Vec::with_capacity(targets.len());
        let mut quick = Vec::with_capacity(targets.len());
        for target in targets {
            if let Some((frame, _)) = self.frames.get(target.key, target.moment) {
                pictures.push(frame);
                quick.push(Quick::Kept);
                continue;
            }
            let proxy = self.proxies.of(target.key, target.fingerprint);
            if let Some(picture) = proxy.and_then(|path| self.proxies.picture(target, &path, self.convert)) {
                pictures.push(picture);
                quick.push(Quick::Proxy);
                continue;
            }
            if !self.key_frame_first {
                return None;
            }
            pictures.push(self.glance(target)?);
            quick.push(Quick::KeyFrame);
        }
        Some((pictures, quick))
    }

    /// The key frame before `target`'s moment, decoded on a decoder of its
    /// own. `None` when it is the one shown first last time (the screen
    /// shows it, or something better, already), when the moment is only
    /// a little behind the frame the file's decoder holds, or when it
    /// can't be had.
    pub(super) fn glance(&mut self, target: &Target<'_>) -> Option<Decoded> {
        let key = target.key;
        let held = self
            .decoders
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, decoder)| decoder.position());
        if held.is_some_and(|held| target.moment < held && held - target.moment < GLANCE_BEHIND) {
            return None;
        }
        let mut decoder = match self.glancer.take() {
            Some((open, decoder)) if open == key => decoder,
            _ => {
                let decoding = if self.ladder.contains(&Level::Hardware) {
                    Decoding::Best
                } else {
                    Decoding::Software
                };
                match VideoDecoder::open_for_glances(target.path, key.1, target.rotation, decoding) {
                    Ok(decoder) => decoder,
                    Err(err) => {
                        tracing::info!(file = %target.path.display(), "no key frame first: {}", err.report());
                        return None;
                    }
                }
            }
        };
        let key_time = decoder.key_frames().before(target.moment);
        if key_time.is_some() && self.last_glance == key_time.map(|t| (key, t)) {
            self.glancer = Some((key, decoder));
            return None;
        }
        let (w, h) = target.size;
        let rgba = matches!(self.convert, Convert::OnCpu);
        let got = decoder.key_frame_picture(target.moment, w, h, rgba);
        let largest = match self.convert {
            Convert::WhileDrawing { largest_side } => largest_side,
            Convert::OnCpu => u32::MAX,
        };
        let picture = match got {
            Ok(Decoded::Yuv(planes)) if planes.width.max(planes.height) > largest => None,
            Ok(picture) => Some(picture),
            Err(err) => {
                tracing::info!(file = %target.path.display(), "no key frame first: {}", err.report());
                None
            }
        };
        if picture.is_some() {
            self.last_glance = key_time.map(|t| (key, t));
        }
        self.glancer = Some((key, decoder));
        picture
    }
}

impl Quick {
    /// What the viewer's corner says for a picture that came this way.
    fn says(self) -> &'static str {
        match self {
            Quick::Kept => KEPT,
            Quick::Proxy => PROXY_FIRST,
            Quick::KeyFrame => KEY_FIRST,
        }
    }
}
