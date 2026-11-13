//! Volume and pan, following their automation lines: the values at the
//! block's start and end are looked up, and the gain moves in a straight
//! line between them sample by sample (no steps, no zipper noise).

use super::dynamics::from_db;

/// A line of points (sample, value), sorted; straight between points,
/// flat before the first and after the last.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub points: Vec<(u64, f32)>,
    cursor: usize,
}

impl Line {
    pub fn new(points: Vec<(u64, f32)>) -> Line {
        Line { points, cursor: 0 }
    }

    /// The value at `at`. Walks on from the last look-up (playback moves
    /// forward); a jump back starts again from a binary search.
    pub fn at(&mut self, at: u64) -> f32 {
        let p = &self.points;
        if p.is_empty() {
            return 0.0;
        }
        if self.cursor >= p.len() || p[self.cursor].0 > at {
            self.cursor = p.partition_point(|q| q.0 <= at).saturating_sub(1);
        }
        while self.cursor + 1 < p.len() && p[self.cursor + 1].0 <= at {
            self.cursor += 1;
        }
        let (t0, v0) = p[self.cursor];
        if at <= t0 || self.cursor + 1 == p.len() {
            return v0;
        }
        let (t1, v1) = p[self.cursor + 1];
        v0 + (v1 - v0) * ((at - t0) as f32 / (t1 - t0) as f32)
    }
}

/// Equal-power pan; the centre leaves both sides as they are.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let k = std::f32::consts::SQRT_2;
    ((a.cos() * k).min(1.0), (a.sin() * k).min(1.0))
}

#[derive(Debug, Clone, Default)]
pub struct Fader {
    pub volume_db: Line,
    pub pan: Line,
}

impl Fader {
    pub fn process(&mut self, pos: u64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len() as u64;
        let (g0, g1) = (
            from_db(self.volume_db.at(pos)),
            from_db(self.volume_db.at(pos + n)),
        );
        let (p0, p1) = (pan_gains(self.pan.at(pos)), pan_gains(self.pan.at(pos + n)));
        let (l0, r0) = (g0 * p0.0, g0 * p0.1);
        let (dl, dr) = ((g1 * p1.0 - l0) / n as f32, (g1 * p1.1 - r0) / n as f32);
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let i = i as f32;
            *a *= l0 + dl * i;
            *b *= r0 + dr * i;
        }
    }

    pub fn take_state(&mut self, old: &Fader) {
        self.volume_db.cursor = old.volume_db.cursor.min(self.volume_db.points.len());
        self.pan.cursor = old.pan.cursor.min(self.pan.points.len());
    }
}
