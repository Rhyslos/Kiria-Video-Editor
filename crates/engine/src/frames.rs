//! Frames kept after they were decoded, so going back to them costs nothing.
//!
//! Showing a frame means decoding every frame from the key frame before it.
//! Those frames used to be thrown away, so one step back decoded them all
//! again. Kept, a step back (or a drag, or playing a stretch again) shows
//! pictures that are already there.
//!
//! A frame is found by its file and the moment asked for: it is the one on
//! screen from its own time until the next frame's, so both are kept.
//! Variable frame rates need nothing special. What is kept is bounded by a
//! budget in bytes. When room is needed, frames of other files go first,
//! the one used longest ago before the others; then the frames of the file
//! being looked at that are furthest from where it is looked at
//! ([`FrameCache::look_at`]), so the stretch around the playhead stays.

use std::collections::BTreeMap;
use std::hash::Hash;

/// The least and the most kept on a graphics card, whatever its size, and
/// the share of its memory in between.
const LEAST: u64 = 256 << 20;
const MOST: u64 = 2 << 30;
const SHARE: u64 = 4;

/// How many bytes of frames to keep on a graphics card with `card_memory`
/// bytes of its own: a quarter, but never more than 2 GB (two seconds of
/// 1440p at 60 frames a second is about 1 GB) and never less than 256 MB.
pub fn budget_on_card(card_memory: u64) -> u64 {
    (card_memory / SHARE).clamp(LEAST, MOST)
}

fn micros(seconds: f64) -> i64 {
    (seconds * 1_000_000.0).round() as i64
}

#[derive(Debug)]
struct Kept<T> {
    picture: T,
    /// When the next frame starts (microseconds); `i64::MAX` for a file's last frame.
    until: i64,
    /// How far the frame's stored time may be from its true time (microseconds).
    tick: i64,
    bytes: u64,
    /// When it was last put in or asked for, on the cache's own clock.
    used: u64,
}

/// Frames of files `K`, by the time they start.
#[derive(Debug)]
pub struct FrameCache<K, T> {
    budget: u64,
    held: u64,
    clock: u64,
    frames: BTreeMap<(K, i64), Kept<T>>,
    /// The file and moment (microseconds) being looked at: what stays longest.
    looked_at: Option<(K, i64)>,
}

impl<K: Ord + Hash + Copy, T: Clone> FrameCache<K, T> {
    /// A cache that keeps at most `budget` bytes.
    pub fn new(budget: u64) -> FrameCache<K, T> {
        FrameCache {
            budget,
            held: 0,
            clock: 0,
            frames: BTreeMap::new(),
            looked_at: None,
        }
    }

    /// Says where the playhead is: frames near `moment` of `file` are the
    /// last to make room.
    pub fn look_at(&mut self, file: K, moment: f64) {
        self.looked_at = Some((file, micros(moment)));
    }

    /// The frame to give up next: one of another file than the one looked
    /// at, used longest ago; failing that, the one furthest from the moment
    /// looked at. Never `keep`, the frame just put in.
    fn next_to_go(&self, keep: (K, i64)) -> Option<(K, i64)> {
        let others = self.frames.iter().filter(|(key, _)| **key != keep);
        let Some((file, moment)) = self.looked_at else {
            return others.min_by_key(|(_, kept)| kept.used).map(|(key, _)| *key);
        };
        others
            .max_by_key(|((of, starts), kept)| {
                if *of == file {
                    (0, starts.abs_diff(moment), 0)
                } else {
                    (1, 0, u64::MAX - kept.used)
                }
            })
            .map(|(key, _)| *key)
    }

    /// Whether a frame of `file` at `moment`, as large as the ones kept
    /// around the playhead, would stay if it were put in now: there is
    /// room, or something further from the playhead would make room for
    /// it. When not, decoding it for keeping is wasted work.
    pub fn would_keep(&self, file: K, moment: f64) -> bool {
        let Some((looked_file, looked_at)) = self.looked_at else {
            return true;
        };
        let at = micros(moment);
        let like = self
            .frames
            .range((file, i64::MIN)..=(file, i64::MAX))
            .next()
            .map_or(0, |(_, kept)| kept.bytes);
        if self.held + like <= self.budget {
            return true;
        }
        if file != looked_file {
            return false;
        }
        self.frames
            .iter()
            .any(|((of, starts), _)| *of != file || starts.abs_diff(looked_at) > at.abs_diff(looked_at))
    }

    /// Where the unbroken stretch of kept frames around `moment` of `file`
    /// begins (seconds): the start of the earliest frame from which every
    /// frame up to the one on screen at `moment` is kept. `None` when that
    /// frame itself isn't kept.
    pub fn stretch_start(&self, file: K, moment: f64) -> Option<f64> {
        let at = micros(moment);
        let reach = at.saturating_add(micros(0.004));
        let mut behind = self.frames.range((file, i64::MIN)..=(file, reach)).rev();
        let ((_, first), _) =
            behind.find(|((_, starts), kept)| *starts <= at + kept.tick && at + kept.tick < kept.until)?;
        let mut start = *first;
        for ((_, starts), kept) in behind {
            // The frame before runs up to this one, give or take a tick.
            if (kept.until - start).abs() > kept.tick.max(1) {
                break;
            }
            start = *starts;
        }
        Some(start as f64 / 1_000_000.0)
    }

    /// Bytes kept now.
    pub fn held(&self) -> u64 {
        self.held
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Keeps the frame of `file` that is on screen from `time` until
    /// `until` (seconds from the start of the file; infinite for the last
    /// frame), taking `bytes`. `tick` is how far the frame's stored time
    /// may be off (as the decoder allows when it picks a frame). Other
    /// frames make room when the budget is passed.
    pub fn put(&mut self, file: K, time: f64, until: f64, tick: f64, bytes: u64, picture: T) {
        self.clock += 1;
        let kept = Kept {
            picture,
            until: if until.is_finite() {
                micros(until)
            } else {
                i64::MAX
            },
            tick: micros(tick),
            bytes,
            used: self.clock,
        };
        let key = (file, micros(time));
        if let Some(old) = self.frames.insert(key, kept) {
            self.held -= old.bytes;
        }
        self.held += bytes;
        while self.held > self.budget {
            let Some(gone) = self.next_to_go(key).and_then(|key| self.frames.remove(&key)) else {
                break;
            };
            self.held -= gone.bytes;
        }
    }

    /// The frame of `file` on screen at `moment`, if it is kept.
    pub fn get(&mut self, file: K, moment: f64) -> Option<T> {
        let at = micros(moment);
        // The last frame that starts by then, give or take a tick. Ticks are
        // at most a few milliseconds: nothing kept starts later than that.
        let reach = at.saturating_add(micros(0.004));
        let (_, kept) = self
            .frames
            .range_mut((file, i64::MIN)..=(file, reach))
            .rev()
            .find(|((_, starts), kept)| *starts <= at + kept.tick)?;
        if at + kept.tick >= kept.until {
            return None; // a later frame is on screen by then, and it isn't kept
        }
        self.clock += 1;
        kept.used = self.clock;
        Some(kept.picture.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixty frames a second from `from`, each a megabyte, named by its number.
    fn fill(cache: &mut FrameCache<u8, u32>, file: u8, from: u32, count: u32) {
        for n in from..from + count {
            let (time, until) = (f64::from(n) / 60.0, f64::from(n + 1) / 60.0);
            cache.put(file, time, until, 0.0005, 1 << 20, n);
        }
    }

    #[test]
    fn a_frame_is_found_for_every_moment_it_is_on_screen() {
        let mut cache = FrameCache::new(1 << 30);
        fill(&mut cache, 1, 0, 120);
        assert_eq!(cache.get(1, 0.0), Some(0));
        assert_eq!(cache.get(1, 1.0), Some(60));
        assert_eq!(cache.get(1, 1.0 + 0.016), Some(60), "until the next one starts");
        assert_eq!(cache.get(1, 1.0 + 1.0 / 60.0), Some(61));
        assert_eq!(cache.get(1, 119.0 / 60.0), Some(119));
        assert_eq!(cache.get(1, 2.0), None, "after the last one kept");
        assert_eq!(cache.get(2, 1.0), None, "another file");
    }

    #[test]
    fn a_frame_stored_a_little_late_is_still_the_frame_for_its_moment() {
        // A recording counting in milliseconds: the frame due at 16.667 ms is stored at 17.
        let mut cache = FrameCache::new(1 << 30);
        cache.put(1, 0.0, 0.017, 0.0005, 1, "first");
        cache.put(1, 0.017, 0.033, 0.0005, 1, "second");
        cache.put(1, 0.033, 0.050, 0.0005, 1, "third");
        assert_eq!(cache.get(1, 1.0 / 60.0), Some("second"));
        assert_eq!(cache.get(1, 2.0 / 60.0), Some("third"));
        assert_eq!(cache.get(1, 0.016), Some("first"));
    }

    #[test]
    fn a_hole_is_not_papered_over() {
        let mut cache = FrameCache::new(1 << 30);
        fill(&mut cache, 1, 0, 10);
        fill(&mut cache, 1, 20, 10);
        assert_eq!(cache.get(1, 9.0 / 60.0), Some(9));
        assert_eq!(cache.get(1, 15.0 / 60.0), None, "frames 10 to 19 were never kept");
        assert_eq!(cache.get(1, 20.0 / 60.0), Some(20));
        // A file's last frame stays on screen for good.
        cache.put(1, 0.5, f64::INFINITY, 0.0005, 1, 30);
        assert_eq!(cache.get(1, 99.0), Some(30));
    }

    #[test]
    fn the_frames_used_longest_ago_make_room() {
        let mut cache = FrameCache::new(100 << 20);
        fill(&mut cache, 1, 0, 120);
        assert_eq!((cache.len(), cache.held()), (100, 100 << 20));
        assert_eq!(cache.get(1, 0.0), None, "the first twenty went");
        assert_eq!(cache.get(1, 20.0 / 60.0), Some(20));
        // Frame 20 was just used: the next to go is 21.
        fill(&mut cache, 2, 0, 1);
        assert_eq!(cache.get(1, 20.0 / 60.0), Some(20));
        assert_eq!(cache.get(1, 21.0 / 60.0), None);
        // Putting a frame in again replaces it without counting it twice.
        fill(&mut cache, 2, 0, 1);
        assert_eq!((cache.len(), cache.held()), (100, 100 << 20));
        // One frame larger than the budget is still kept: it is the one on screen.
        let mut small = FrameCache::new(10);
        small.put(1, 0.0, 1.0, 0.0, 1000, 7);
        assert_eq!(small.get(1, 0.5), Some(7));
    }

    #[test]
    fn the_stretch_around_the_playhead_stays() {
        let mut cache = FrameCache::new(100 << 20);
        // Another file's frames, then two seconds of the file being looked at.
        fill(&mut cache, 9, 0, 30);
        cache.look_at(1, 1.5);
        fill(&mut cache, 1, 0, 120);
        assert_eq!(cache.len(), 100);
        assert_eq!(cache.get(9, 0.0), None, "the other file's frames went first");
        // Of this file, the ones furthest from 1.5 s went: the first twenty.
        assert_eq!(cache.get(1, 19.0 / 60.0), None);
        assert_eq!(cache.get(1, 20.0 / 60.0), Some(20));
        assert_eq!(cache.get(1, 119.0 / 60.0), Some(119));
        // Filling in behind the playhead pushes out what is furthest from
        // it, not what was put in before: the frame at the playhead stays.
        cache.look_at(1, 21.0 / 60.0);
        fill(&mut cache, 1, 0, 20);
        assert_eq!(cache.get(1, 0.0), Some(0));
        assert_eq!(cache.get(1, 21.0 / 60.0), Some(21));
        assert_eq!(cache.get(1, 119.0 / 60.0), None);
    }

    #[test]
    fn a_frame_is_worth_decoding_only_if_it_would_stay() {
        let mut cache = FrameCache::new(100 << 20);
        cache.look_at(1, 1.0);
        fill(&mut cache, 1, 30, 60); // 0.5 s to 1.5 s around the playhead
        assert!(cache.would_keep(1, 0.4), "there is room");
        fill(&mut cache, 1, 90, 40); // full now: 0.5 s to 2.17 s
        assert_eq!(cache.len(), 100);
        // Just behind what is kept: the frames after 2 s are further from the playhead.
        assert!(cache.would_keep(1, 0.49));
        // Further away than anything kept: it would go at once.
        assert!(!cache.would_keep(1, -0.5));
        // Another file's frames always make room for the file looked at.
        let mut cache = FrameCache::new(100 << 20);
        cache.look_at(1, 1.0);
        fill(&mut cache, 2, 0, 100);
        fill(&mut cache, 1, 60, 1);
        assert!(cache.would_keep(1, -5.0));
        assert!(!cache.would_keep(2, 3.0), "but not the other way round");
    }

    #[test]
    fn the_start_of_the_stretch_kept_around_a_moment() {
        let mut cache = FrameCache::new(1 << 30);
        assert_eq!(cache.stretch_start(1, 1.0), None);
        fill(&mut cache, 1, 30, 60); // frames 30 to 89: 0.5 s to 1.5 s
        assert_eq!(cache.stretch_start(1, 1.0), Some(0.5));
        assert_eq!(cache.stretch_start(1, 0.5), Some(0.5));
        assert_eq!(cache.stretch_start(1, 0.4), None, "before it");
        assert_eq!(cache.stretch_start(1, 1.6), None, "after it");
        // An earlier stretch with a hole before this one doesn't count.
        fill(&mut cache, 1, 0, 20);
        assert_eq!(cache.stretch_start(1, 1.0), Some(0.5));
        // The hole filled in: one stretch from the start.
        fill(&mut cache, 1, 20, 10);
        assert_eq!(cache.stretch_start(1, 1.0), Some(0.0));
        assert_eq!(cache.stretch_start(2, 1.0), None, "another file");
    }

    #[test]
    fn a_quarter_of_the_card_within_bounds() {
        let gb = 1u64 << 30;
        assert_eq!(budget_on_card(20 * gb), 2 * gb);
        assert_eq!(budget_on_card(4 * gb), gb);
        assert_eq!(budget_on_card(0), 256 << 20);
    }
}
