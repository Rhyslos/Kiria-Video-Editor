//! Filling in behind the playhead while nothing is asked for.

use super::Worker;
use super::waiting::Waiting;
use media::{Decoded, Newer, Reached, VideoDecoder};
use project::MediaId;
use std::time::Duration;

/// How long nothing must have been asked for before the worker fills in
/// behind the playhead: longer than the gap between two requests while
/// playing or dragging, so filling never competes with either.
pub(super) const IDLE_AFTER: Duration = Duration::from_millis(80);

/// How much is kept behind the playhead (seconds of video), when the frames
/// are there to be kept: less than this behind it, and the stretch before
/// is filled in.
pub(super) const KEPT_BEHIND: f64 = 2.0;

/// How much of what a decode passes on its way is kept while the timeline
/// plays (seconds of video): a few frames. Keeping a frame costs a copy on
/// the graphics card, waited for, and a decode that has fallen behind (a
/// stretch of heavy frames, such as the whole picture changing at once)
/// passes many frames to catch up: copying each of them made it fall
/// further behind, until the picture stalled for as long as the stretch
/// lasted. Played frames not kept are filled in behind the playhead once
/// playing stops.
pub(super) const KEPT_WHILE_PLAYING: f64 = 0.05;

/// How much of what a decode passes is kept: while playing, a few frames;
/// otherwise what is kept behind the playhead anyway.
pub(super) fn kept_within(playing: bool) -> f64 {
    if playing { KEPT_WHILE_PLAYING } else { KEPT_BEHIND }
}

/// A file the viewer is showing, and where in it.
#[derive(Debug, Clone)]
pub(super) struct Playhead {
    pub(super) key: (MediaId, u32),
    pub(super) path: std::path::PathBuf,
    pub(super) rotation: u16,
    /// Seconds from the start of the file.
    pub(super) moment: f64,
    /// The size pictures are asked for at.
    pub(super) size: (u32, u32),
}

/// The moment to decode up to so that the stretch before the frames kept
/// around `moment` is kept too, when it should be: `kept_from` is where the
/// kept frames begin. `None` when enough is kept behind `moment`, or the
/// kept frames begin at the start of the file.
pub(super) fn fill_up_to(moment: f64, kept_from: f64, tick: f64) -> Option<f64> {
    // Just before the first kept frame: the frame on screen then is the last one missing.
    let last_missing = kept_from - tick - 0.0002;
    (moment - kept_from < KEPT_BEHIND && last_missing > 0.0).then_some(last_missing)
}

impl Worker {
    /// Nothing has been asked for a moment: lets go of proxies that went,
    /// then fills in behind the playhead ([`Worker::fill`]). Returns whether
    /// there may be more to do.
    pub(super) fn idle(&mut self, waiting: &mut Waiting<'_>) -> bool {
        self.proxies.notice_changes();
        self.fill(waiting)
    }

    /// Fills in behind the playhead: decodes the stretch before the frames
    /// kept around it, from the key frame before them, keeping every frame.
    /// Stops between frames as soon as a request arrives. Returns whether
    /// anything was done (there may be more to do).
    pub(super) fn fill(&mut self, waiting: &mut Waiting<'_>) -> bool {
        let Some(playhead) = self.playhead.clone() else {
            return false;
        };
        let key = playhead.key;
        if self.unfillable.contains(&key) || self.off_device.contains(&key) {
            return false;
        }
        let Some(kept_from) = self.frames.stretch_start(key, playhead.moment) else {
            return false; // the frame at the playhead isn't kept: nothing to add to
        };
        if self.stuck == Some((key, kept_from)) {
            return false;
        }
        // The filler is a decoder of its own: the one at the playhead keeps its place.
        // A second decoder on the device failing says nothing about the
        // first: the file is just not filled in.
        let Some(on_device) = &self.on_device else {
            return false;
        };
        let opened = match self.filler.take() {
            Some((open, decoder)) if open == key => Ok(decoder),
            _ => {
                on_device.watch.about_to_decode();
                let opened =
                    VideoDecoder::open_on_device(&playhead.path, key.1, playhead.rotation, &on_device.device);
                match &opened {
                    Ok(_) => on_device.watch.answered(),
                    Err(_) => on_device.watch.reported_an_error(),
                }
                opened
            }
        };
        let mut decoder = match opened {
            Ok(decoder) => decoder,
            Err(err) => {
                tracing::info!(file = %playhead.path.display(), "not filling in behind the playhead: {}", err.report());
                self.unfillable.insert(key);
                return false;
            }
        };
        let tick = decoder.tick();
        // Enough is kept behind the playhead already, or what would be
        // added is further from it than everything there is room for.
        let up_to =
            fill_up_to(playhead.moment, kept_from, tick).filter(|up_to| self.frames.would_keep(key, *up_to));
        let Some(up_to) = up_to else {
            self.filler = Some((key, decoder));
            return false;
        };
        let frames = &mut self.frames;
        let mut keep = |picture: Decoded, until: f64| {
            if let Decoded::Gpu(frame) = picture {
                let (w, h) = frame.planes.size();
                let bytes = u64::from(w) * u64::from(h) * 3 / 2;
                frames.put(key, frame.time, until, tick, bytes, frame);
            }
        };
        // Anything asked for comes first.
        let mut newer = || {
            waiting.take_in();
            if waiting.newest.is_some() || waiting.closed {
                Newer::Elsewhere
            } else {
                Newer::Nothing
            }
        };
        on_device.watch.about_to_decode();
        let (w, h) = playhead.size;
        let reached = decoder.picture_at_while(up_to, w, h, &mut newer, Some(&mut keep));
        match reached {
            Ok(reached) => {
                on_device.watch.answered();
                self.filler = Some((key, decoder));
                // Finished, and the kept stretch begins where it did: the
                // frames didn't join up (times that don't follow on). Not again.
                if matches!(reached, Reached::Shown { .. })
                    && self.frames.stretch_start(key, playhead.moment) == Some(kept_from)
                {
                    self.stuck = Some((key, kept_from));
                }
                // Given up for a request: more to do later. Finished: look again.
                true
            }
            Err(err) => {
                on_device.watch.reported_an_error();
                tracing::warn!(file = %playhead.path.display(), "filling in behind the playhead failed: {}", err.report());
                self.unfillable.insert(key);
                false
            }
        }
    }
}
