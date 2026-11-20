//! Reading ahead of the playhead while the timeline plays.
//!
//! A recording's frames don't all cost the same to decode: one where the
//! whole picture changes at once (a flash, a cut, night vision switching
//! on in a game) is many times larger than one that only changes a little,
//! and takes the decoder that much longer. Decoded only as each is due, a
//! stretch of them comes late and the picture stalls, even when decoding
//! is fast enough on average. So while playing, between requests, the
//! decoder at the playhead decodes on to [`AHEAD`] past it and keeps those
//! frames: requests then find them kept, and a slow stretch uses up what
//! was read ahead instead of holding the picture up.

use super::Worker;
use super::fill::KEPT_BEHIND;
use super::waiting::Waiting;
use media::{Decoded, Level, Newer};

/// How far past the playhead is read ahead while playing (seconds of the
/// file): room for a stretch of slow frames, kept on the card (half a
/// second at 60 frames a second is 30 frames).
pub(super) const AHEAD: f64 = 0.5;

impl Worker {
    /// While playing and with nothing asked for: decodes on past the
    /// playhead with the decoder that is there, keeping every frame, up to
    /// [`AHEAD`] past it or until a request arrives. Returns whether there
    /// may be more to read ahead (it stopped for a request, or got there).
    pub(super) fn read_ahead(&mut self, waiting: &mut Waiting<'_>) -> bool {
        waiting.take_in();
        if !self.playing || waiting.newest.is_some() || waiting.closed {
            return false;
        }
        let (Some(playhead), Some(on_device)) = (&self.playhead, &self.on_device) else {
            return false;
        };
        let key = playhead.key;
        let Some(found) = self.decoders.iter().position(|(k, _)| *k == key) else {
            return false;
        };
        let up_to = playhead.moment + AHEAD;
        // Only on the window's device, where frames are kept; only on from
        // where the decoder is: never back to an earlier key frame, nor on
        // from somewhere behind the playhead (a request that comes does
        // that), nor past what there is room to keep.
        let decoder = &self.decoders[found].1;
        if decoder.level() != Level::Device {
            return false;
        }
        let tick = decoder.tick();
        let on_from = decoder.position();
        let worth_it = on_from.is_some_and(|at| at + tick >= playhead.moment && at + tick < up_to)
            && self.frames.would_keep(key, up_to);
        if !worth_it {
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
        decoder.keep_within(KEPT_BEHIND);
        on_device.watch.about_to_decode();
        let (w, h) = playhead.size;
        let read = decoder.picture_at_while(up_to, w, h, &mut newer, Some(&mut keep));
        let more = match read {
            Ok(_) => {
                on_device.watch.answered();
                // Nothing further at the end of the file: not again until a request moves it.
                decoder.position() != on_from
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
