//! Volume and pan: the track's fader (volume, pan, mute and solo, changed
//! live) and its automation lines.
//!
//! A line is points (a sample from the timeline's start, a value), straight
//! between points and flat before the first and after the last. Lines are
//! worked out on a fixed grid, every [`GRID`] samples counted from the
//! timeline's start, and the gain goes in a straight line from one grid
//! point to the next. What a sample gets therefore depends only on where
//! it is on the timeline, never on where a block starts: playback and
//! export, or two plays started at different places, give the same sound.
//!
//! A change of the fader (a volume dragged, mute) glides over one block,
//! so it never clicks.

use super::from_db;

/// Samples between the grid points lines are worked out at.
pub const GRID: u64 = 64;

/// One line: points `(sample, value)` in time order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    points: Vec<(u64, f32)>,
    /// Where the last look-up was (playback moves forward, so the next is
    /// usually the same point or the one after).
    cursor: usize,
}

impl Line {
    /// A line through `points`, which are put in time order; of two at the
    /// same sample the later one given is kept.
    pub fn new(mut points: Vec<(u64, f32)>) -> Line {
        points.sort_by_key(|p| p.0);
        points.reverse();
        points.dedup_by_key(|p| p.0);
        points.reverse();
        Line { points, cursor: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The value at sample `at`, `None` for a line without points. The
    /// answer is the same however the line was looked up before.
    pub fn value_at(&mut self, at: u64) -> Option<f32> {
        let p = &self.points;
        let first = p.first()?;
        if at <= first.0 {
            return Some(first.1);
        }
        if self.cursor >= p.len() || p[self.cursor].0 > at {
            self.cursor = p.partition_point(|q| q.0 <= at).saturating_sub(1);
        }
        while self.cursor + 1 < p.len() && p[self.cursor + 1].0 <= at {
            self.cursor += 1;
        }
        let (t0, v0) = p[self.cursor];
        let Some(&(t1, v1)) = p.get(self.cursor + 1) else {
            return Some(v0);
        };
        let share = (at - t0) as f64 / (t1 - t0) as f64;
        Some((f64::from(v0) + (f64::from(v1) - f64::from(v0)) * share) as f32)
    }
}

/// A track's lines: volume (dB, added to the fader's volume) and pan (-1
/// all left to 1 all right; while it has points, it is the pan and the
/// fader's pan is not used).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lines {
    pub volume_db: Line,
    pub pan: Line,
}

impl Lines {
    pub fn is_empty(&self) -> bool {
        self.volume_db.is_empty() && self.pan.is_empty()
    }

    /// The line's left and right gains at grid point `cell`.
    fn gains_at(&mut self, cell: u64) -> [f32; 2] {
        let at = cell * GRID;
        let volume = self.volume_db.value_at(at).map_or(1.0, from_db);
        let [l, r] = self.pan.value_at(at).map_or([1.0, 1.0], pan_gains);
        [volume * l, volume * r]
    }
}

/// Left and right gains for a pan from -1 (all left) to 1 (all right):
/// equal power, with the centre leaving both sides as they are.
pub fn pan_gains(pan: f32) -> [f32; 2] {
    // In double precision, so the centre is exactly 1 on both sides.
    let angle = (f64::from(pan.clamp(-1.0, 1.0)) + 1.0) * std::f64::consts::FRAC_PI_4;
    let k = std::f64::consts::SQRT_2;
    [
        (angle.cos() * k).min(1.0) as f32,
        (angle.sin() * k).min(1.0) as f32,
    ]
}

/// The fader: volume, pan and whether the track is heard (mute and solo),
/// and the track's lines.
#[derive(Debug, Clone)]
pub struct Fader {
    pub volume_db: f32,
    pub pan: f32,
    /// Heard: not muted, and soloed when any track is. The mixer sets it.
    pub audible: bool,
    lines: Box<Lines>,
    /// The fader's left and right gains as the last block ended.
    now: [f32; 2],
}

impl Fader {
    pub fn new(volume_db: f32, pan: f32, audible: bool, lines: Box<Lines>) -> Fader {
        let mut f = Fader {
            volume_db,
            pan,
            audible,
            lines,
            now: [0.0; 2],
        };
        f.now = f.wanted();
        f
    }

    /// What the fader's gains should be now.
    fn wanted(&self) -> [f32; 2] {
        if !self.audible {
            return [0.0; 2];
        }
        let volume = super::volume_gain(self.volume_db);
        let [l, r] = if self.lines.pan.is_empty() {
            pan_gains(self.pan)
        } else {
            [1.0, 1.0]
        };
        [volume * l, volume * r]
    }

    /// Puts in new lines and hands back the old, to be freed elsewhere.
    pub fn swap_lines(&mut self, lines: &mut Box<Lines>) {
        std::mem::swap(&mut self.lines, lines);
    }

    /// The block starting `pos` samples from the timeline's start.
    pub fn process(&mut self, pos: u64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let from = self.now;
        let to = self.wanted();
        self.now = to;
        if from != to {
            let step = [(to[0] - from[0]) / n as f32, (to[1] - from[1]) / n as f32];
            for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
                let i = (i + 1) as f32;
                *a *= from[0] + step[0] * i;
                *b *= from[1] + step[1] * i;
            }
        } else if to != [1.0, 1.0] {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                *a *= to[0];
                *b *= to[1];
            }
        }
        if !self.lines.is_empty() {
            self.follow_lines(pos, &mut l[..n], &mut r[..n]);
        }
    }

    /// Applies the lines' gains, a straight line between grid points.
    fn follow_lines(&mut self, pos: u64, l: &mut [f32], r: &mut [f32]) {
        let mut done = 0;
        let mut cell = pos / GRID;
        let mut start = self.lines.gains_at(cell);
        while done < l.len() {
            let end = self.lines.gains_at(cell + 1);
            let cell_start = cell * GRID;
            let from = (pos + done as u64 - cell_start) as usize;
            let take = (GRID as usize - from).min(l.len() - done);
            for i in 0..take {
                let share = (from + i) as f32 / GRID as f32;
                l[done + i] *= start[0] + (end[0] - start[0]) * share;
                r[done + i] *= start[1] + (end[1] - start[1]) * share;
            }
            done += take;
            cell += 1;
            start = end;
        }
    }

    pub fn take_state(&mut self, old: &Fader) {
        self.now = old.now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Box<Lines> {
        Box::new(Lines {
            volume_db: Line::new(vec![(1_000, 0.0), (5_000, -12.0), (9_000, -12.0), (9_001, 6.0)]),
            pan: Line::new(vec![(0, -1.0), (20_000, 1.0)]),
        })
    }

    #[test]
    fn a_line_is_straight_between_points_and_flat_outside() {
        let mut line = Line::new(vec![(100, 2.0), (0, 0.0), (100, 4.0)]);
        assert_eq!(line.value_at(0), Some(0.0));
        assert_eq!(line.value_at(50), Some(2.0));
        assert_eq!(line.value_at(100), Some(4.0), "the later of two at one place");
        assert_eq!(line.value_at(1_000), Some(4.0));
        assert_eq!(line.value_at(25), Some(1.0), "looking back works");
        assert_eq!(Line::default().value_at(5), None);
    }

    #[test]
    fn where_a_block_starts_never_changes_the_sound() {
        let len = 30_000;
        let input: Vec<f32> = (0..len)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let run = |blocks: &[usize]| {
            let mut f = Fader::new(-3.0, 0.2, true, lines());
            let (mut l, mut r) = (input.clone(), input.clone());
            let (mut at, mut b) = (0, 0);
            while at < len {
                let n = blocks[b % blocks.len()].min(len - at);
                f.process(at as u64, &mut l[at..at + n], &mut r[at..at + n]);
                at += n;
                b += 1;
            }
            (l, r)
        };
        let whole = run(&[256]);
        for blocks in [&[1][..], &[100, 37], &[441], &[480], &[1024], &[255, 257]] {
            assert_eq!(run(blocks), whole, "{blocks:?}");
        }
        // Started 777 samples in, sample n of the timeline still gets the
        // same gain as when played from the start.
        let mut f = Fader::new(-3.0, 0.2, true, lines());
        let mut tail_l = input[777..].to_vec();
        let mut tail_r = tail_l.clone();
        for (i, (a, b)) in tail_l.chunks_mut(256).zip(tail_r.chunks_mut(256)).enumerate() {
            f.process(777 + i as u64 * 256, a, b);
        }
        assert_eq!(tail_l, whole.0[777..]);
        assert_eq!(tail_r, whole.1[777..]);
    }

    #[test]
    fn the_lines_give_their_values_at_grid_points() {
        let mut f = Fader::new(0.0, 0.0, true, lines());
        let mut l = vec![1f32; 20_480];
        let mut r = l.clone();
        f.process(0, &mut l, &mut r);
        // At 5 056 (a grid point near 5 000) the volume line is -12 dB;
        // the pan line is still left of centre, so left is untouched.
        let at = 5_056;
        let mut probe = lines();
        let want = probe.gains_at(at as u64 / GRID);
        assert_eq!([l[at], r[at]], want);
        assert!((super::super::to_db(l[at]) + 12.0).abs() < 0.01);
        // After the volume jumps up at 9 001, +6 dB.
        assert!((super::super::to_db(l[9_600]) - 6.0).abs() < 0.01);
        assert_eq!(r[0], 0.0, "all left at the start");
    }

    #[test]
    fn a_fader_change_glides_over_one_block() {
        let mut f = Fader::new(0.0, 0.0, true, Box::default());
        let mut l = vec![1f32; 256];
        let mut r = l.clone();
        f.audible = false;
        f.process(0, &mut l, &mut r);
        assert!(l[0] > 0.99 && l[255] == 0.0 && l[128] > 0.4 && l[128] < 0.6);
        let mut l = vec![1f32; 256];
        let mut r = l.clone();
        f.process(256, &mut l, &mut r);
        assert!(l.iter().all(|v| *v == 0.0));
        assert_eq!(pan_gains(0.0), [1.0, 1.0]);
        assert_eq!(pan_gains(-1.0)[1], 0.0);
    }
}
