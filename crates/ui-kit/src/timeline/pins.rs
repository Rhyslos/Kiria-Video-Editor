//! Which audio tracks have a column beside the master column (the tall
//! fader and meter): those pinned from their menu, in the order they were
//! pinned, or with [`Pins::all`] every audio track but those taken off by
//! hand. Kept with the project's layout; a setting says whether every
//! track gets one.

/// The pins of one timeline view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pins {
    /// Tracks pinned by hand, in the order they were pinned.
    pub pinned: Vec<u64>,
    /// Tracks taken off by hand while every track has a column, so they
    /// stay off.
    pub unpinned: Vec<u64>,
    /// Every audio track has a column (a setting), but those unpinned.
    pub all: bool,
}

impl Pins {
    /// The tracks with a column, of the audio tracks `audio` (in their
    /// order): those pinned by hand first, in the order they were pinned,
    /// then (with `all`) the others in track order.
    pub fn columns(&self, audio: &[u64]) -> Vec<u64> {
        let mut shown: Vec<u64> = self
            .pinned
            .iter()
            .copied()
            .filter(|id| audio.contains(id) && !(self.all && self.unpinned.contains(id)))
            .collect();
        if self.all {
            for id in audio {
                if !shown.contains(id) && !self.unpinned.contains(id) {
                    shown.push(*id);
                }
            }
        }
        shown
    }

    /// Whether track `id` has a column.
    pub fn has(&self, id: u64) -> bool {
        match self.all {
            true => !self.unpinned.contains(&id),
            false => self.pinned.contains(&id),
        }
    }

    /// Gives track `id` a column (at the end of those pinned by hand), or
    /// takes its column away (and keeps it away while every track has one).
    pub fn set(&mut self, id: u64, on: bool) {
        self.pinned.retain(|t| *t != id);
        self.unpinned.retain(|t| *t != id);
        if on {
            self.pinned.push(id);
        } else {
            self.unpinned.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_by_hand_keeps_the_order_and_each_track_once() {
        let mut pins = Pins::default();
        pins.set(3, true);
        pins.set(5, true);
        pins.set(3, true);
        assert_eq!(pins.columns(&[3, 5, 7]), [5, 3], "pinned again: at the end");
        pins.set(7, true);
        pins.set(5, false);
        assert_eq!(pins.columns(&[3, 5, 7]), [3, 7]);
        assert_eq!(pins.columns(&[3]), [3], "a track that is gone has none");
        assert!(pins.has(3) && !pins.has(5));
    }

    #[test]
    fn every_track_but_those_taken_off_when_all_have_one() {
        let mut pins = Pins {
            all: true,
            ..Pins::default()
        };
        assert_eq!(pins.columns(&[1, 2, 3]), [1, 2, 3], "in track order");
        pins.set(2, false);
        assert_eq!(pins.columns(&[1, 2, 3, 4]), [1, 3, 4], "a new track gets one too");
        assert!(!pins.has(2) && pins.has(4));
        pins.set(3, true);
        assert_eq!(pins.columns(&[1, 2, 3, 4]), [3, 1, 4], "pinned by hand first");
        pins.set(2, true);
        assert_eq!(pins.columns(&[1, 2, 3, 4]), [3, 2, 1, 4], "back again");
        // Without every track, only those pinned by hand.
        pins.all = false;
        assert_eq!(pins.columns(&[1, 2, 3, 4]), [3, 2]);
    }
}
