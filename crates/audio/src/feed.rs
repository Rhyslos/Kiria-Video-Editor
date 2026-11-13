//! A track's input: one lock-free ring per track, which a feeder thread
//! fills with decoded sound ahead of time and the mixer empties a block at
//! a time.
//!
//! **When a ring runs dry** the mixer plays silence for what is missing and
//! counts it; the feeder takes that count ([`FeedWriter::take_owed`]) and
//! skips as much of its sound, so a late file comes back in step instead of
//! drifting behind for the rest of the play.
//!
//! **Jumping somewhere else** (the playhead moved): the feeder stops
//! writing, the controller sends [`crate::Command::Locate`] with a new
//! number, and the mixer, at its next block, empties every ring, forgets
//! what was owed, carries on from the new place and says so
//! ([`crate::Controller::located`]). Only then does the feeder write sound
//! from the new place, taking what it owes first. Sound written before the
//! mixer said so would be thrown away with the rest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What both ends share.
#[derive(Debug, Default)]
struct Shared {
    /// Frames the mixer filled with silence and the feeder has not yet skipped.
    owed: AtomicU64,
}

/// The feeder's end of a track's ring.
#[derive(Debug)]
pub struct FeedWriter {
    to: rtrb::Producer<f32>,
    shared: Arc<Shared>,
}

/// The mixer's end of a track's ring.
#[derive(Debug)]
pub struct FeedReader {
    from: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
}

/// A ring for one track holding up to `frames` stereo frames.
pub fn ring(frames: usize) -> (FeedWriter, FeedReader) {
    let (to, from) = rtrb::RingBuffer::new(frames.max(1) * 2);
    let shared = Arc::new(Shared::default());
    (
        FeedWriter {
            to,
            shared: Arc::clone(&shared),
        },
        FeedReader { from, shared },
    )
}

impl FeedWriter {
    /// Frames there is room for now.
    pub fn room(&self) -> usize {
        self.to.slots() / 2
    }

    /// Writes as many whole frames of `interleaved` stereo as there is room
    /// for; returns how many frames it wrote.
    pub fn write(&mut self, interleaved: &[f32]) -> usize {
        let frames = (interleaved.len() / 2).min(self.room());
        if frames == 0 {
            return 0;
        }
        if let Ok(mut chunk) = self.to.write_chunk(frames * 2) {
            let (a, b) = chunk.as_mut_slices();
            let split = a.len();
            a.copy_from_slice(&interleaved[..split]);
            b.copy_from_slice(&interleaved[split..frames * 2]);
            chunk.commit_all();
            frames
        } else {
            0
        }
    }

    /// Frames the mixer had to fill with silence since last asked: skip
    /// that much sound before writing more.
    pub fn take_owed(&self) -> u64 {
        self.shared.owed.swap(0, Ordering::AcqRel)
    }
}

impl FeedReader {
    /// Fills `l` and `r` from the ring; what is missing is silence, and
    /// counted as owed. Returns how many frames were missing.
    pub fn read(&mut self, l: &mut [f32], r: &mut [f32]) -> usize {
        let n = l.len().min(r.len());
        let have = (self.from.slots() / 2).min(n);
        if have > 0
            && let Ok(chunk) = self.from.read_chunk(have * 2)
        {
            let (a, b) = chunk.as_slices();
            for (i, pair) in a.chunks_exact(2).chain(b.chunks_exact(2)).enumerate() {
                l[i] = pair[0];
                r[i] = pair[1];
            }
            // A chunk of whole frames can split a frame between its two
            // slices only if the ring's length were odd; it is even.
            chunk.commit_all();
        }
        let missing = n - have;
        if missing > 0 {
            l[have..n].fill(0.0);
            r[have..n].fill(0.0);
            self.shared.owed.fetch_add(missing as u64, Ordering::AcqRel);
        }
        missing
    }

    /// Throws away everything in the ring and forgets what was owed (a jump).
    pub fn clear(&mut self) {
        let n = self.from.slots();
        if let Ok(chunk) = self.from.read_chunk(n) {
            chunk.commit_all();
        }
        self.shared.owed.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_is_read_and_what_is_missing_is_owed() {
        let (mut w, mut r) = ring(4);
        assert_eq!(w.write(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0]), 3);
        let (mut l, mut rr) = ([9f32; 2], [9f32; 2]);
        assert_eq!(r.read(&mut l, &mut rr), 0);
        assert_eq!((l, rr), ([1.0, 2.0], [-1.0, -2.0]));
        // Wraps around the ring's end.
        assert_eq!(w.write(&[4.0, -4.0, 5.0, -5.0, 6.0, -6.0]), 3);
        let (mut l, mut rr) = ([9f32; 6], [9f32; 6]);
        assert_eq!(r.read(&mut l, &mut rr), 2, "four there, six wanted");
        assert_eq!(l, [3.0, 4.0, 5.0, 6.0, 0.0, 0.0]);
        assert_eq!(rr, [-3.0, -4.0, -5.0, -6.0, 0.0, 0.0]);
        assert_eq!(w.take_owed(), 2);
        assert_eq!(w.take_owed(), 0);
        assert_eq!(w.write(&[1.0; 20]), 4, "only the room there is");
        r.clear();
        assert_eq!(w.room(), 4);
    }
}
