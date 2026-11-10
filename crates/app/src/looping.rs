//! Looping playback inside the timeline region.

use project::{Region, Time};

/// Where playback starts: inside the region when looping and the playhead is
/// outside it, otherwise the playhead.
pub fn start_at(playhead: Time, region: Option<Region>, looping: bool) -> Time {
    match region {
        Some(r) if looping && (playhead < r.start || playhead >= r.end) => r.start,
        _ => playhead,
    }
}

/// While looping, where to jump back to once playback reaches `at`:
/// the region's start once the end (or the timeline's end) is reached.
pub fn wrap(at: Time, region: Option<Region>, looping: bool, length: Time) -> Option<Time> {
    let r = region.filter(|_| looping)?;
    (at >= r.end.min(length)).then_some(r.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: i64) -> Time {
        Time::from_ratio(s, 1).unwrap()
    }

    #[test]
    fn starts_inside_the_region_when_looping() {
        let r = Region::between(t(2), t(4));
        assert_eq!(start_at(t(1), r, true), t(2));
        assert_eq!(start_at(t(3), r, true), t(3));
        assert_eq!(start_at(t(5), r, true), t(2));
        assert_eq!(start_at(t(5), r, false), t(5));
        assert_eq!(start_at(t(5), None, true), t(5));
    }

    #[test]
    fn jumps_back_at_the_end() {
        let r = Region::between(t(2), t(4));
        assert_eq!(wrap(t(3), r, true, t(10)), None);
        assert_eq!(wrap(t(4), r, true, t(10)), Some(t(2)));
        assert_eq!(wrap(t(4), r, false, t(10)), None);
        // A region past the end of the timeline loops at the end.
        assert_eq!(wrap(t(3), r, true, t(3)), Some(t(2)));
    }
}
