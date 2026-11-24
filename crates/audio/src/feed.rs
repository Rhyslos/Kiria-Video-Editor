//! A track's input: one lock-free ring per track, which a feeder thread
//! fills with decoded sound ahead of time and the mixer empties a block at
//! a time.
//!
//! **When a ring runs dry** the mixer plays silence for what is missing and
//! counts it; the feeder takes that count ([`FeedWriter::take_owed`]) and
//! skips as much of its sound, so a late file comes back in step instead of
//! drifting behind for the rest of the play.
//!
//! **A track added while playing** says which sample of the timeline its
//! first frame is ([`crate::Controller::add_track_at`]); when the mixer
//! takes it, it plays silence until then (not owed: there was nothing to
//! play), or, when that moment has passed, owes the frames since, so the
//! new track is in step with the others to the sample however late it
//! comes in.
//!
//! **Jumping somewhere else** (the playhead moved): the feeder stops
//! writing, the controller sends [`crate::Command::Locate`] with a new
//! number, and the mixer, at its next block, empties every ring, forgets
//! what was owed, carries on from the new place and says so
//! ([`crate::Controller::located`]). Only then does the feeder write sound
//! from the new place, taking what it owes first. Sound written before the
//! mixer said so would be thrown away with the rest.
//!
//! **A sample that is not a number** (NaN or infinity, from a damaged
//! recording) is written as 0 and counted ([`FeedWriter::take_replaced`]):
//! once inside, it would stay in the filters' and the limiter's state and
//! silence or poison the track, or the whole mix, until playback started
//! again. Every sound this crate's mixer hears comes in here, so its mixer
//! never takes one whatever writes. It is not the only check: the
//! engine's clips' sound (`engine::mix::Mixer::mix_track`) is already
//! clean before it is written, and that also covers what reaches a
//! [`crate::strip::Strip`] without a ring (the clarity check).
//!
//! **Sound already in a ring that a change makes wrong** (a clip's volume
//! being dragged while it plays) is cut short instead of played out: the
//! feeder asks for a cut ([`FeedWriter::ask_cut`]) and stops writing; the
//! mixer, at its next block, drops what is in the ring past that point
//! (never what it is already playing) and says where it cut; the feeder
//! then writes again from there ([`FeedWriter::cut_done`]), so the change
//! is heard within a few blocks instead of after the whole ring.
//!
//! The ring is samples kept as the bits of `f32`s in atomics, with
//! counters of what was written and read: both ends wait for nothing and
//! the reader can move the writer's end back while the writer waits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// What both ends share.
#[derive(Debug)]
struct Shared {
    /// The samples, interleaved stereo, as `f32` bits.
    samples: Box<[AtomicU32]>,
    /// Samples written and read since the ring was made: what is in it is
    /// the difference.
    written: AtomicU64,
    read: AtomicU64,
    /// Frames the mixer filled with silence and the feeder has not yet skipped.
    owed: AtomicU64,
    /// Cuts asked for and made, counted; where the feeder asked to cut
    /// (a count of samples written) and where the mixer did.
    cuts_asked: AtomicU64,
    cuts_made: AtomicU64,
    cut_to: AtomicU64,
    cut_at: AtomicU64,
}

impl Shared {
    fn slot(&self, at: u64) -> &AtomicU32 {
        &self.samples[(at % self.samples.len() as u64) as usize]
    }
}

/// The feeder's end of a track's ring.
#[derive(Debug)]
pub struct FeedWriter {
    shared: Arc<Shared>,
    /// Samples written as 0 because they were not numbers, not yet taken.
    replaced: u64,
    /// A cut asked for and not yet made: what had been written then.
    cutting: Option<u64>,
}

/// The mixer's end of a track's ring.
#[derive(Debug)]
pub struct FeedReader {
    shared: Arc<Shared>,
    /// Frames of silence to play before reading (a track whose first frame
    /// is later than where the mixer is): not owed.
    wait: u64,
}

/// A ring for one track holding up to `frames` stereo frames.
pub fn ring(frames: usize) -> (FeedWriter, FeedReader) {
    let samples = (0..frames.max(1) * 2).map(|_| AtomicU32::new(0)).collect();
    let shared = Arc::new(Shared {
        samples,
        written: AtomicU64::new(0),
        read: AtomicU64::new(0),
        owed: AtomicU64::new(0),
        cuts_asked: AtomicU64::new(0),
        cuts_made: AtomicU64::new(0),
        cut_to: AtomicU64::new(0),
        cut_at: AtomicU64::new(0),
    });
    (
        FeedWriter {
            shared: Arc::clone(&shared),
            replaced: 0,
            cutting: None,
        },
        FeedReader { shared, wait: 0 },
    )
}

impl FeedWriter {
    /// Frames there is room for now (none while a cut is waited for).
    pub fn room(&self) -> usize {
        if self.cutting.is_some() {
            return 0;
        }
        let s = &self.shared;
        let held = s.written.load(Ordering::Relaxed) - s.read.load(Ordering::Acquire);
        (s.samples.len() - held as usize) / 2
    }

    /// Frames in the ring not yet played.
    pub fn held(&self) -> usize {
        let s = &self.shared;
        let held = s.written.load(Ordering::Relaxed) - s.read.load(Ordering::Acquire);
        held as usize / 2
    }

    /// Writes as many whole frames of `interleaved` stereo as there is room
    /// for; returns how many frames it wrote. A sample that is NaN or
    /// infinite is written as 0 and counted.
    pub fn write(&mut self, interleaved: &[f32]) -> usize {
        let frames = (interleaved.len() / 2).min(self.room());
        if frames == 0 {
            return 0;
        }
        let s = &self.shared;
        let at = s.written.load(Ordering::Relaxed);
        let mut replaced = 0;
        for (i, from) in interleaved[..frames * 2].iter().enumerate() {
            let v = if from.is_finite() {
                *from
            } else {
                replaced += 1;
                0.0
            };
            s.slot(at + i as u64).store(v.to_bits(), Ordering::Relaxed);
        }
        s.written.store(at + frames as u64 * 2, Ordering::Release);
        self.replaced += replaced;
        frames
    }

    /// Samples written as 0 since last asked because they were NaN or
    /// infinite (a damaged recording): for the log.
    pub fn take_replaced(&mut self) -> u64 {
        std::mem::take(&mut self.replaced)
    }

    /// Frames the mixer had to fill with silence since last asked: skip
    /// that much sound before writing more.
    pub fn take_owed(&self) -> u64 {
        self.shared.owed.swap(0, Ordering::AcqRel)
    }

    /// Asks the mixer to drop what the ring holds past `keep` frames from
    /// what it plays now. Nothing is written until [`FeedWriter::cut_done`]
    /// says it was made.
    pub fn ask_cut(&mut self, keep: usize) {
        if self.cutting.is_some() {
            return;
        }
        let s = &self.shared;
        let written = s.written.load(Ordering::Relaxed);
        let to = (s.read.load(Ordering::Acquire) + keep as u64 * 2).min(written);
        s.cut_to.store(to, Ordering::Relaxed);
        s.cuts_asked.fetch_add(1, Ordering::Release);
        self.cutting = Some(written);
    }

    /// Whether a cut is being waited for.
    pub fn cutting(&self) -> bool {
        self.cutting.is_some()
    }

    /// Once the mixer has made the cut asked for: how many frames it
    /// dropped (write again from that much earlier). `None` while it is
    /// still to come, or when none was asked for.
    pub fn cut_done(&mut self) -> Option<u64> {
        let was = self.cutting?;
        let s = &self.shared;
        if s.cuts_made.load(Ordering::Acquire) != s.cuts_asked.load(Ordering::Relaxed) {
            return None;
        }
        self.cutting = None;
        Some((was - s.cut_at.load(Ordering::Relaxed)) / 2)
    }
}

impl FeedReader {
    /// Makes a cut the feeder asked for: what the ring holds past where it
    /// asked (never before what has been read) is dropped. Never waits.
    fn make_cut(&self) {
        let s = &self.shared;
        let asked = s.cuts_asked.load(Ordering::Acquire);
        if asked == s.cuts_made.load(Ordering::Relaxed) {
            return;
        }
        let read = s.read.load(Ordering::Relaxed);
        let written = s.written.load(Ordering::Acquire);
        let at = s.cut_to.load(Ordering::Relaxed).clamp(read, written);
        s.written.store(at, Ordering::Release);
        s.cut_at.store(at, Ordering::Relaxed);
        s.cuts_made.store(asked, Ordering::Release);
    }

    /// Fills `l` and `r` from the ring; what is missing is silence, and
    /// counted as owed. Returns how many frames were missing.
    pub fn read(&mut self, l: &mut [f32], r: &mut [f32]) -> usize {
        self.make_cut();
        let n = l.len().min(r.len());
        if self.wait > 0 {
            let quiet = usize::try_from(self.wait).unwrap_or(usize::MAX).min(n);
            l[..quiet].fill(0.0);
            r[..quiet].fill(0.0);
            self.wait -= quiet as u64;
            let (l, r) = (&mut l[quiet..n], &mut r[quiet..n]);
            return if l.is_empty() { 0 } else { self.read(l, r) };
        }
        let s = &self.shared;
        let read = s.read.load(Ordering::Relaxed);
        let written = s.written.load(Ordering::Acquire);
        let have = (((written - read) / 2) as usize).min(n);
        for i in 0..have {
            let at = read + i as u64 * 2;
            l[i] = f32::from_bits(s.slot(at).load(Ordering::Relaxed));
            r[i] = f32::from_bits(s.slot(at + 1).load(Ordering::Relaxed));
        }
        s.read.store(read + have as u64 * 2, Ordering::Release);
        let missing = n - have;
        if missing > 0 {
            l[have..n].fill(0.0);
            r[have..n].fill(0.0);
            s.owed.fetch_add(missing as u64, Ordering::AcqRel);
        }
        missing
    }

    /// Lines the ring up with the mixer: its first frame is sample `first`
    /// of the timeline and the mixer's next block starts at `pos`.
    pub fn align(&mut self, first: u64, pos: u64) {
        if first >= pos {
            self.wait = first - pos;
        } else {
            self.shared.owed.fetch_add(pos - first, Ordering::AcqRel);
        }
    }

    /// Throws away everything in the ring and forgets what was owed (a jump).
    pub fn clear(&mut self) {
        self.make_cut();
        self.wait = 0;
        let s = &self.shared;
        s.read.store(s.written.load(Ordering::Acquire), Ordering::Release);
        s.owed.store(0, Ordering::Release);
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

    #[test]
    fn samples_that_are_not_numbers_are_written_as_zero_and_counted() {
        let (mut w, mut r) = ring(4);
        w.write(&[0.5, -0.5]);
        let (mut l, mut rr) = ([9f32; 1], [9f32; 1]);
        r.read(&mut l, &mut rr);
        // Wraps around the ring's end, so both of the chunk's slices are checked.
        let written = [
            f32::NAN,
            0.25,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.25,
            f32::NAN,
            1.0,
            1.0,
        ];
        assert_eq!(w.write(&written), 4);
        let (mut l, mut rr) = ([9f32; 4], [9f32; 4]);
        assert_eq!(r.read(&mut l, &mut rr), 0);
        assert_eq!(l, [0.0, 0.0, -0.25, 1.0]);
        assert_eq!(rr, [0.25, 0.0, 0.0, 1.0]);
        assert_eq!(w.take_replaced(), 4);
        assert_eq!(w.take_replaced(), 0);
    }

    #[test]
    fn a_late_or_early_track_lines_up_with_the_mixer() {
        // Its first frame is three frames after where the mixer is: silence
        // first, not owed, then the ring.
        let (mut w, mut r) = ring(8);
        r.align(13, 10);
        w.write(&[1.0, 1.0, 2.0, 2.0]);
        let (mut l, mut rr) = ([9f32; 4], [9f32; 4]);
        assert_eq!(r.read(&mut l, &mut rr), 0);
        assert_eq!(l, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(r.read(&mut l[..1], &mut rr[..1]), 0);
        assert_eq!(l[0], 2.0);
        assert_eq!(w.take_owed(), 0);
        // Its first frame has gone by: the frames since are owed.
        let (w, mut r) = ring(8);
        r.align(10, 14);
        assert_eq!(w.take_owed(), 4);
    }

    #[test]
    fn a_cut_drops_what_was_not_played_and_the_feeder_writes_on_from_there() {
        let (mut w, mut r) = ring(16);
        let frames: Vec<f32> = (0..10).flat_map(|i| [i as f32, -(i as f32)]).collect();
        assert_eq!(w.write(&frames), 10);
        let (mut l, mut rr) = ([9f32; 3], [9f32; 3]);
        r.read(&mut l, &mut rr);
        assert_eq!(l, [0.0, 1.0, 2.0]);
        // Keep two more frames past what is played; the rest goes.
        w.ask_cut(2);
        assert!(w.cutting());
        assert_eq!(w.room(), 0, "nothing written while waiting");
        assert_eq!(w.cut_done(), None, "the mixer hasn't been");
        let (mut l, mut rr) = ([9f32; 1], [9f32; 1]);
        r.read(&mut l, &mut rr);
        assert_eq!(l, [3.0], "what it was about to play is kept");
        assert_eq!(w.cut_done(), Some(5), "frames 5 to 9 dropped");
        assert!(!w.cutting());
        // Written again from frame 5, new sound.
        assert_eq!(w.write(&[50.0, -50.0, 60.0, -60.0]), 2);
        let (mut l, mut rr) = ([9f32; 4], [9f32; 4]);
        assert_eq!(r.read(&mut l, &mut rr), 1);
        assert_eq!(l, [4.0, 50.0, 60.0, 0.0]);
    }

    #[test]
    fn a_cut_never_takes_back_what_was_played() {
        let (mut w, mut r) = ring(8);
        w.write(&[1.0, 1.0, 2.0, 2.0]);
        w.ask_cut(4);
        // Played past where it asked to cut before the mixer saw the ask.
        let (mut l, mut rr) = ([9f32; 2], [9f32; 2]);
        r.read(&mut l, &mut rr);
        assert_eq!(w.cut_done(), Some(0), "nothing past what is held");
    }
}
