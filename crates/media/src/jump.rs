//! Jump or decode on? Getting to a moment in a file means decoding forward
//! from a key frame, so a decoder that is already close behind the moment
//! is quicker carrying on, and one that has a key frame between itself and
//! the moment is quicker jumping to that key frame.
//!
//! Where the key frames are comes from the container's own list (an MP4's
//! sample table, an MKV's cues), read when the file is opened: no part of
//! the file is read for it. A container may list none, or not all of them;
//! then a fixed distance decides, as it did before there was a list.

/// Without a list of key frames: how far ahead (seconds) decoding on is
/// still thought cheaper than jumping.
const DECODE_ON_LIMIT: f64 = 3.0;

/// A listed key frame only counts as reached by a moment at least this much
/// (seconds) after it, whatever the stream says ([`KeyFrames::new`]).
const LEAST_MARGIN: f64 = 0.05;

/// The key frames a container lists for one stream.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyFrames {
    /// Seconds from the start of the stream, ascending.
    times: Vec<f64>,
    /// How far ahead decoding on is still tried where no key frame is
    /// listed in between: past the end of an incomplete list, for example.
    decode_on: f64,
    /// A key frame counts as reached by a moment this much after its
    /// listed time.
    margin: f64,
}

/// No key frames listed: the fixed distance decides.
impl Default for KeyFrames {
    fn default() -> KeyFrames {
        KeyFrames::new(Vec::new(), 0.0)
    }
}

impl KeyFrames {
    /// From the times the container lists, in any order.
    ///
    /// `shown_after` is how long after it is decoded a frame of this stream
    /// may be shown (the frames it is reordered by, as seconds). Some
    /// containers list the time a key frame is decoded, not the time it is
    /// shown; a moment only counts as having reached a key frame when it is
    /// later than the listed time by more than that, so the frames shown
    /// just before a key frame are never mistaken for being after it.
    pub fn new(mut times: Vec<f64>, shown_after: f64) -> KeyFrames {
        times.retain(|t| t.is_finite());
        times.sort_by(f64::total_cmp);
        times.dedup();
        let widest = times.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
        KeyFrames {
            times,
            decode_on: DECODE_ON_LIMIT.max(widest * 1.5),
            margin: LEAST_MARGIN.max(shown_after),
        }
    }

    /// How many are listed.
    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Whether a key frame is listed after `position` that `target` has reached.
    fn between(&self, position: f64, target: f64) -> bool {
        let first_after = self.times.partition_point(|t| *t <= position + 1e-6);
        self.times
            .get(first_after)
            .is_some_and(|key| key + self.margin <= target)
    }

    /// Whether getting to `target` is quicker by jumping than by decoding on.
    ///
    /// `held` is the time of the frame the decoder shows now and `position`
    /// the time of the newest frame it has decoded (the same, or one frame
    /// later); both `None` before the first frame, when the decoder stands
    /// at the start of the file. `ended` says the file was decoded to its end.
    pub fn jump(&self, held: Option<f64>, position: Option<f64>, target: f64, ended: bool) -> bool {
        // Behind what is shown: only a jump goes back.
        if held.is_some_and(|held| target + 1e-6 < held) {
            return true;
        }
        match position.or(held) {
            Some(position) => self.between(position, target) || target - position > self.decode_on,
            // Nothing decoded yet: decoding starts at the first key frame.
            None => {
                ended
                    || match self.times.first() {
                        Some(first) => self.between(*first, target) || target - first > self.decode_on,
                        None => target > DECODE_ON_LIMIT,
                    }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key frames every two seconds for a minute, as game recorders write them.
    fn every_two_seconds() -> KeyFrames {
        KeyFrames::new((0..30).map(|i| f64::from(i) * 2.0).collect(), 0.0)
    }

    #[test]
    fn a_key_frame_on_the_way_is_jumped_to() {
        let keys = every_two_seconds();
        // Showing 10.5 s, decoded up to 10.52: 11.9 is ahead in the same group.
        assert!(!keys.jump(Some(10.5), Some(10.52), 11.9, false));
        // 12.5 has passed the key frame at 12: jump there, don't decode 90 frames on.
        assert!(keys.jump(Some(10.5), Some(10.52), 12.5, false));
        // Just after a key frame the list's time may be a few frames early: decode on.
        assert!(!keys.jump(Some(11.9), Some(11.92), 12.02, false));
        assert!(keys.jump(Some(11.0), Some(11.02), 12.06, false));
        // Anything behind what is shown needs a jump, however near.
        assert!(keys.jump(Some(10.5), Some(10.52), 10.4, false));
        // The frame shown again, or the one decoded ahead of it: no jump.
        assert!(!keys.jump(Some(10.5), Some(10.52), 10.5, false));
        assert!(!keys.jump(Some(10.5), Some(10.52), 10.52, false));
    }

    #[test]
    fn frames_shown_just_before_a_key_frame_are_not_taken_for_after_it() {
        // A file at 24 frames a second whose frames are shown up to four
        // frames after they are decoded, and whose list has decode times:
        // the key frame listed at 12.0 is shown at 12.167.
        let keys = KeyFrames::new((0..30).map(|i| f64::from(i) * 2.0).collect(), 5.0 / 24.0);
        // The last frames of the group before it: decode on to them.
        assert!(!keys.jump(Some(11.0), Some(11.04), 12.125, false));
        // Well into the next group: jump.
        assert!(keys.jump(Some(11.0), Some(11.04), 12.5, false));
    }

    #[test]
    fn a_fresh_decoder_starts_at_the_first_key_frame() {
        let keys = every_two_seconds();
        assert!(!keys.jump(None, None, 0.0, false));
        assert!(!keys.jump(None, None, 1.9, false));
        assert!(keys.jump(None, None, 2.5, false));
        assert!(
            keys.jump(None, None, 0.5, true),
            "after the end only a jump goes on"
        );
    }

    #[test]
    fn without_a_list_a_fixed_distance_decides() {
        let none = KeyFrames::default();
        assert!(none.is_empty());
        assert!(!none.jump(Some(10.0), Some(10.02), 12.9, false));
        assert!(none.jump(Some(10.0), Some(10.02), 13.1, false));
        assert!(none.jump(Some(10.0), Some(10.02), 9.0, false));
        assert!(!none.jump(None, None, 2.9, false));
        assert!(none.jump(None, None, 3.1, false));
    }

    #[test]
    fn past_the_end_of_an_incomplete_list_the_distance_decides() {
        // Key frames 8 s apart, listed up to 16 s only.
        let keys = KeyFrames::new(vec![16.0, 0.0, 8.0, 8.0], 0.0);
        assert_eq!(keys.len(), 3);
        // Within the list a key frame decides, also 7.9 s ahead.
        assert!(!keys.jump(Some(8.0), Some(8.02), 15.9, false));
        assert!(keys.jump(Some(8.0), Some(8.02), 16.5, false));
        // After it: decode on up to one and a half of the widest gap, then jump.
        assert!(!keys.jump(Some(17.0), Some(17.02), 28.0, false));
        assert!(keys.jump(Some(17.0), Some(17.02), 29.5, false));
    }
}
