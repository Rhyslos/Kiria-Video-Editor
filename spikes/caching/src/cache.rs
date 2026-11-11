//! Approach B: decoded pictures kept in memory, found by the time they are
//! on screen, with a byte budget. The picture used longest ago goes first.

use crate::decoder::Picture;
use std::collections::BTreeMap;

struct Entry {
    picture: Picture,
    used: u64,
}

pub struct Cache {
    /// Pictures by start time in microseconds.
    pictures: BTreeMap<i64, Entry>,
    /// Start times by when they were last used, oldest first.
    by_use: BTreeMap<u64, i64>,
    budget: usize,
    bytes: usize,
    clock: u64,
    /// The most bytes held at once.
    pub peak_bytes: usize,
    /// Pictures starting in this stretch (microseconds) are dropped last:
    /// the window around the playhead.
    protected: (i64, i64),
}

fn key(time: f64) -> i64 {
    (time * 1e6).round() as i64
}

impl Cache {
    pub fn new(budget_bytes: usize) -> Cache {
        Cache {
            pictures: BTreeMap::new(),
            by_use: BTreeMap::new(),
            budget: budget_bytes,
            bytes: 0,
            clock: 0,
            peak_bytes: 0,
            protected: (0, -1),
        }
    }

    /// Pictures between `from` and `to` seconds are about to be needed:
    /// drop the others first. Without this, playing backwards drops the
    /// frames it is about to show, because they were decoded longest ago.
    pub fn protect(&mut self, from: f64, to: f64) {
        self.protected = (key(from), key(to));
    }

    fn find(&self, t: f64) -> Option<i64> {
        let (k, entry) = self.pictures.range(..=key(t)).next_back()?;
        entry.picture.stamp.covers(t).then_some(*k)
    }

    /// When the cached picture on screen at `t` ends, if there is one.
    /// Doesn't count as a use.
    pub fn covered_until(&self, t: f64) -> Option<f64> {
        self.find(t).map(|k| self.pictures[&k].picture.stamp.end)
    }

    /// The picture on screen at `t`.
    pub fn get(&mut self, t: f64) -> Option<&Picture> {
        let k = self.find(t)?;
        self.clock += 1;
        let entry = self.pictures.get_mut(&k)?;
        self.by_use.remove(&entry.used);
        entry.used = self.clock;
        self.by_use.insert(self.clock, k);
        Some(&entry.picture)
    }

    /// Start time of the closest cached picture at or before `t`.
    pub fn nearest_before(&self, t: f64) -> Option<f64> {
        let (_, entry) = self.pictures.range(..=key(t)).next_back()?;
        Some(entry.picture.stamp.time)
    }

    /// Keeps a picture, dropping the ones used longest ago to stay in budget.
    pub fn insert(&mut self, picture: Picture) {
        let k = key(picture.stamp.time);
        self.remove(k);
        self.clock += 1;
        self.bytes += picture.data.len();
        self.by_use.insert(self.clock, k);
        self.pictures.insert(
            k,
            Entry {
                picture,
                used: self.clock,
            },
        );
        while self.bytes > self.budget && self.pictures.len() > 1 {
            let (from, to) = self.protected;
            let outside = self.by_use.values().find(|k| **k < from || **k > to);
            let Some(oldest) = outside.or_else(|| self.by_use.values().next()).copied() else {
                break;
            };
            self.remove(oldest);
        }
        self.peak_bytes = self.peak_bytes.max(self.bytes);
    }

    /// The frame that follows the one starting at `time` turned out to start
    /// at `end`: with a variable frame rate a frame stays on screen until then.
    pub fn set_end(&mut self, time: f64, end: f64) {
        if let Some(entry) = self.pictures.get_mut(&key(time)) {
            entry.picture.stamp.end = end;
        }
    }

    fn remove(&mut self, k: i64) {
        if let Some(entry) = self.pictures.remove(&k) {
            self.bytes -= entry.picture.data.len();
            self.by_use.remove(&entry.used);
        }
    }

    pub fn clear(&mut self) {
        self.pictures.clear();
        self.by_use.clear();
        self.bytes = 0;
        self.peak_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::Stamp;

    fn picture(frame: u32, bytes: usize) -> Picture {
        let time = f64::from(frame) / 60.0;
        Picture {
            stamp: Stamp {
                time,
                end: time + 1.0 / 60.0,
            },
            width: 2,
            height: 2,
            data: vec![0; bytes],
        }
    }

    #[test]
    fn finds_the_picture_on_screen_at_a_time() {
        let mut cache = Cache::new(1000);
        cache.insert(picture(3, 10));
        cache.insert(picture(4, 10));
        assert_eq!(cache.get(3.5 / 60.0).map(|p| p.stamp.time), Some(3.0 / 60.0));
        assert!(cache.get(5.5 / 60.0).is_none(), "frame 5 was never kept");
        assert!(cache.get(2.5 / 60.0).is_none());
        assert_eq!(cache.nearest_before(9.0 / 60.0), Some(4.0 / 60.0));
    }

    #[test]
    fn drops_the_picture_used_longest_ago() {
        let mut cache = Cache::new(30);
        for f in 0..3 {
            cache.insert(picture(f, 10));
        }
        assert!(cache.get(0.5 / 60.0).is_some()); // frame 0 is now the newest use
        cache.insert(picture(3, 10));
        assert_eq!(cache.pictures.len(), 3);
        assert!(cache.get(1.5 / 60.0).is_none(), "frame 1 was used longest ago");
        assert!(cache.get(0.5 / 60.0).is_some());
        assert_eq!(cache.peak_bytes, 30);
    }

    #[test]
    fn pictures_around_the_playhead_are_dropped_last() {
        let mut cache = Cache::new(30);
        for f in 0..3 {
            cache.insert(picture(f, 10));
        }
        cache.protect(0.0, 1.5 / 60.0); // frames 0 and 1
        cache.insert(picture(3, 10));
        assert!(cache.covered_until(0.5 / 60.0).is_some());
        assert!(cache.covered_until(1.5 / 60.0).is_some());
        assert!(
            cache.covered_until(2.5 / 60.0).is_none(),
            "frame 2 was outside the window"
        );
    }

    #[test]
    fn a_frame_lasts_until_the_next_one_starts() {
        let mut cache = Cache::new(1000);
        cache.insert(picture(0, 10));
        assert!(cache.covered_until(1.5 / 60.0).is_none());
        cache.set_end(0.0, 3.0 / 60.0);
        assert_eq!(cache.covered_until(1.5 / 60.0), Some(3.0 / 60.0));
    }
}
