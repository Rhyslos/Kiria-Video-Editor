//! Reading ahead of the playhead.
//!
//! A recording's frames don't all cost the same to decode: where the whole
//! picture changes at once (a flash, a cut, night vision switching on in a
//! game) a recorder writes frames many times larger than usual for a second
//! or two, and the decoder takes that much longer on each. Decoded only as
//! each is due, such a stretch comes late and the picture stalls, even when
//! decoding is fast enough on average. So the decoder at the playhead reads
//! ahead and keeps what it decodes: as far as the memory for frames on the
//! card has room for, up to [`MOST_AHEAD`]. While playing it does so between
//! requests; while paused, once what is behind the playhead is filled in,
//! so playing starts with frames in hand. A stretch of slow frames shorter
//! than what was read ahead plays without a stall.

use super::Worker;
use super::waiting::Waiting;
use media::{Decoded, Level, Newer};

/// The furthest past the playhead that is read ahead (seconds of the file),
/// when the memory for frames has room: at 3440×1440 and 60 frames a second,
/// ten seconds is 600 frames, about 4.4 GB.
pub(super) const MOST_AHEAD: f64 = 10.0;

/// How much is read at a time before looking again whether it would be
/// kept (seconds of the file).
const STEP: f64 = 0.25;

impl Worker {
    /// With nothing asked for: decodes on past the playhead with the decoder
    /// that is there, keeping every frame, a step at a time, until
    /// [`MOST_AHEAD`] past it, until the frames kept have no more room, or
    /// until a request arrives. Returns whether there may be more to read
    /// (it stopped for a request, or read a step).
    pub(super) fn read_ahead(&mut self, waiting: &mut Waiting<'_>) -> bool {
        waiting.take_in();
        if waiting.newest.is_some() || waiting.closed {
            return false;
        }
        let (Some(playhead), Some(on_device)) = (&self.playhead, &self.on_device) else {
            return false;
        };
        let key = playhead.key;
        let Some(found) = self.decoders.iter().position(|(k, _)| *k == key) else {
            return false;
        };
        // Only on the window's device, where frames are kept; only on from
        // where the decoder is: never back to an earlier key frame, nor on
        // from somewhere behind the playhead (a request that comes does
        // that), nor past what there is room to keep.
        let decoder = &self.decoders[found].1;
        if decoder.level() != Level::Device {
            return false;
        }
        let tick = decoder.tick();
        let Some(on_from) = decoder.position().filter(|at| at + tick >= playhead.moment) else {
            return false;
        };
        let up_to = (on_from + STEP).min(playhead.moment + MOST_AHEAD);
        if on_from + tick >= up_to || !self.frames.would_keep(key, up_to) {
            return false;
        }
        let Some((_, mut decoder)) = self.decoders.remove(found) else {
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
        // Every request comes first: it stops the reading between frames.
        let mut newer = || {
            waiting.take_in();
            if waiting.newest.is_some() || waiting.closed {
                Newer::Elsewhere
            } else {
                Newer::Nothing
            }
        };
        decoder.keep_within(2.0 * STEP);
        on_device.watch.about_to_decode();
        let (w, h) = playhead.size;
        let read = decoder.picture_at_while(up_to, w, h, &mut newer, Some(&mut keep));
        let more = match read {
            Ok(_) => {
                on_device.watch.answered();
                // Nothing further at the end of the file: not again until a request moves it.
                decoder.position() != Some(on_from)
            }
            Err(err) => {
                on_device.watch.reported_an_error();
                // The next request decodes as it always does, and steps down if it must.
                tracing::info!(file = %playhead.path.display(), "reading ahead stopped: {}", err.report());
                false
            }
        };
        self.decoders.push_front((key, decoder));
        more
    }
}
