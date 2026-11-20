//! Decoding one file's picture at one moment, stepping down the ladder
//! when a way fails.

use super::fill::{Playhead, kept_within};
use super::ladder::below;
use super::stand_in::PROXY_ONLY;
use super::waiting::Waiting;
use super::{Convert, File, OPEN_DECODERS, OnDevice, Picture, Role, Target, Worker, source_of};
use media::{Decoded, Level, Newer, Reached, VideoDecoder};
use project::Time;

impl Worker {
    /// The picture of one file at one moment: kept, or decoded. `None` when
    /// the decode was given up for a newer request.
    pub(super) fn decoded(
        &mut self,
        on_timeline: Time,
        target: &Target<'_>,
        waiting: &mut Waiting<'_>,
        role: Role,
    ) -> Option<Picture> {
        let mut out = Picture::black(on_timeline);
        let (key, at) = (target.key, target.moment);
        let file = File {
            path: target.path,
            stream: key.1,
            rotation: target.rotation,
        };
        if !role.leads {
            return self.decoded_below(out, target, &file, waiting);
        }
        // The frames around the playhead are the last to make room, and
        // what is filled in behind when nothing else is going on.
        self.frames.look_at(key, at, self.playing);
        self.playhead = Some(Playhead {
            key,
            path: file.path.to_path_buf(),
            rotation: file.rotation,
            moment: at,
            size: target.size,
        });
        // Decoded before and still kept: nothing to decode.
        if let Some((frame, how)) = self.frames.get(key, at) {
            out.frame = Some(frame);
            out.method = Some(how.to_owned());
            self.last_glance = None;
            return Some(out);
        }
        // Proxies On: the proxy alone, and nothing filled in behind.
        if let Some(picture) = self.proxy_only(target) {
            self.playhead = None;
            out.frame = Some(picture);
            out.method = Some(PROXY_ONLY.to_owned());
            return Some(out);
        }
        let found = self.decoders.iter().position(|(k, _)| *k == key);
        let mut decoder = match found.and_then(|i| self.decoders.remove(i)) {
            Some((_, decoder)) => decoder,
            None => match self.open(key, &file, self.ladder_for(key)) {
                Ok(decoder) => decoder,
                Err(err) => {
                    out.error = Some(err);
                    return Some(out);
                }
            },
        };
        // While playing, only the last few frames passed are made pictures
        // to keep, so a decode that fell behind catches up at decoding speed.
        decoder.keep_within(kept_within(self.playing));
        // What the decoder is told between frames: what the newest request
        // waiting means for this file. Moments of this file that were asked
        // for meanwhile are remembered with their place on the timeline, to
        // name the picture if the decode stops at one of them.
        let mut asked_meanwhile: Vec<(f64, Time)> = Vec::new();
        let mut now_wanted = Newer::Nothing;
        // A decode that follows one that was given up is finished, whatever
        // arrives: requests that keep coming must not keep pictures from
        // coming.
        let give_up = role.give_up;
        let mut newer = || {
            if waiting.take_in()
                && role.follows
                && let Some(next) = &waiting.newest
            {
                now_wanted = match source_of(next) {
                    Some((k, moment)) if k == key => {
                        asked_meanwhile.push((moment, next.at));
                        Newer::At { moment, give_up }
                    }
                    _ if give_up => Newer::Elsewhere,
                    _ => Newer::Nothing,
                };
            }
            if waiting.closed {
                Newer::Elsewhere
            } else {
                now_wanted
            }
        };
        // Decode; when a way fails for this file (an unusual profile, a
        // driver problem), open it one way down and ask again.
        let mut dropped = false;
        loop {
            // Frames the decoder passes on the way are kept for going back to.
            let tick = decoder.tick();
            let frames = &mut self.frames;
            let mut keep = |picture: Decoded, until: f64| {
                if let Decoded::Gpu(frame) = picture {
                    frames.put(key, until, tick, frame);
                }
            };
            let decoded = decode(
                self.convert,
                self.on_device.as_ref(),
                &mut decoder,
                at,
                target.size,
                &mut newer,
                Some(&mut keep),
            );
            match decoded {
                Ok(Reached::Shown { picture, at: reached }) => {
                    out.frame = Some(picture);
                    // Stopped at a moment asked for meanwhile: it is that request's picture.
                    if (reached - at).abs() > 1e-9
                        && let Some((_, on_timeline)) = asked_meanwhile
                            .iter()
                            .rev()
                            .find(|(moment, _)| (moment - reached).abs() <= 1e-9)
                    {
                        out.at = *on_timeline;
                    }
                    break;
                }
                Ok(Reached::Dropped) => {
                    dropped = true;
                    break;
                }
                Err(err) => {
                    let failed = decoder.level();
                    if failed == Level::Device {
                        self.off_device.insert(key);
                        if let Some(on_device) = &self.on_device {
                            on_device.watch.reported_an_error();
                        }
                    }
                    let lower = below(self.ladder, failed);
                    if lower.is_empty() {
                        out.error = Some(err);
                        break;
                    }
                    tracing::warn!(file = %file.path.display(), "decoding with {} failed, stepping down: {}", decoder.method, err.report());
                    match self.open(key, &file, lower) {
                        Ok(next) => decoder = next,
                        Err(_) => {
                            out.error = Some(err);
                            break;
                        }
                    }
                }
            }
        }
        out.method = Some(decoder.method.clone());
        out.work = Some(decoder.last_work());
        // Most recently used first.
        self.decoders.push_front((key, decoder));
        self.decoders.truncate(OPEN_DECODERS);
        self.gave_up_last = dropped;
        if dropped {
            return None;
        }
        // The exact frame is on screen: a key frame shown first again later is news.
        self.last_glance = None;
        Some(out)
    }

    /// The picture of a layer under the top one: kept, or decoded to the
    /// end whatever is asked for meanwhile (only closing stops it). It
    /// leaves the playhead, and so what is filled in behind it, alone.
    pub(super) fn decoded_below(
        &mut self,
        mut out: Picture,
        target: &Target<'_>,
        file: &File<'_>,
        waiting: &mut Waiting<'_>,
    ) -> Option<Picture> {
        let (key, at) = (target.key, target.moment);
        if let Some((frame, how)) = self.frames.get(key, at) {
            out.frame = Some(frame);
            out.method = Some(how.to_owned());
            return Some(out);
        }
        if let Some(picture) = self.proxy_only(target) {
            out.frame = Some(picture);
            out.method = Some(PROXY_ONLY.to_owned());
            return Some(out);
        }
        let found = self.decoders.iter().position(|(k, _)| *k == key);
        let mut decoder = match found.and_then(|i| self.decoders.remove(i)) {
            Some((_, decoder)) => decoder,
            None => match self.open(key, file, self.ladder_for(key)) {
                Ok(decoder) => decoder,
                Err(err) => {
                    out.error = Some(err);
                    return Some(out);
                }
            },
        };
        let mut newer = || {
            waiting.take_in();
            if waiting.closed {
                Newer::Elsewhere
            } else {
                Newer::Nothing
            }
        };
        let mut closing = false;
        loop {
            let decoded = decode(
                self.convert,
                self.on_device.as_ref(),
                &mut decoder,
                at,
                target.size,
                &mut newer,
                None,
            );
            match decoded {
                Ok(Reached::Shown { picture, .. }) => {
                    out.frame = Some(picture);
                    break;
                }
                Ok(Reached::Dropped) => {
                    closing = true;
                    break;
                }
                Err(err) => {
                    let failed = decoder.level();
                    if failed == Level::Device {
                        self.off_device.insert(key);
                        if let Some(on_device) = &self.on_device {
                            on_device.watch.reported_an_error();
                        }
                    }
                    let lower = below(self.ladder, failed);
                    let next = if lower.is_empty() {
                        None
                    } else {
                        tracing::warn!(file = %file.path.display(), "decoding with {} failed, stepping down: {}", decoder.method, err.report());
                        self.open(key, file, lower).ok()
                    };
                    match next {
                        Some(next) => decoder = next,
                        None => {
                            out.error = Some(err);
                            break;
                        }
                    }
                }
            }
        }
        out.method = Some(decoder.method.clone());
        out.work = Some(decoder.last_work());
        self.decoders.push_front((key, decoder));
        self.decoders.truncate(OPEN_DECODERS);
        (!closing).then_some(out)
    }
}

/// Decodes towards `at`, asking `newer` between frames whether that is
/// still what is wanted and handing the frames passed to `keep`
/// (`VideoDecoder::picture_at_while`).
pub(super) fn decode(
    convert: Convert,
    on_device: Option<&OnDevice>,
    decoder: &mut VideoDecoder,
    at: f64,
    (w, h): (u32, u32),
    newer: &mut dyn FnMut() -> Newer,
    keep: Option<&mut dyn FnMut(Decoded, f64)>,
) -> diag::Result<Reached<Decoded>> {
    let rgba = |reached: Reached<media::Frame>| match reached {
        Reached::Shown { picture, at } => Reached::Shown {
            picture: Decoded::Rgba(picture),
            at,
        },
        Reached::Dropped => Reached::Dropped,
    };
    let Convert::WhileDrawing { largest_side } = convert else {
        return decoder.frame_at_while(at, w, h, newer).map(rgba);
    };
    let watch = match (on_device, decoder.level()) {
        (Some(on_device), Level::Device) => Some(&on_device.watch),
        _ => None,
    };
    if let Some(watch) = watch {
        watch.about_to_decode();
    }
    let reached = decoder.picture_at_while(at, w, h, newer, keep);
    if let (Some(watch), Ok(_)) = (watch, &reached) {
        watch.answered(); // an error is reported by the caller
    }
    let (picture, at) = match reached? {
        Reached::Shown { picture, at } => (picture, at),
        Reached::Dropped => return Ok(Reached::Dropped),
    };
    let picture = match picture {
        // The frame is held already: this only converts it.
        Decoded::Yuv(planes) if planes.width.max(planes.height) > largest_side => {
            Decoded::Rgba(decoder.frame_at(at, w, h)?)
        }
        // Pictures on the graphics card come at the file's size: one too
        // large for a texture goes down a way instead.
        Decoded::Gpu(frame) if frame.planes.size().0.max(frame.planes.size().1) > largest_side => {
            return Err(diag::AppError::new("The video can't be shown")
                .why("The picture is larger than a texture on this graphics card can be."));
        }
        picture => picture,
    };
    Ok(Reached::Shown { picture, at })
}
