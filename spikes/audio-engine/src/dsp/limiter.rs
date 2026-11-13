//! The master's true-peak limiter: looks ahead 64 samples, so the gain is
//! already down when a peak (found between samples too) arrives, and
//! comes back up slowly.
//!
//! The gain wanted for each sample is ceiling / true peak (at most 1);
//! the lowest of those over the look-ahead is held, let rise only at the
//! release speed, then averaged over the look-ahead, so it reaches the
//! peak's value exactly when the peak goes out, with a smooth slope.

use super::meter::TruePeak;

pub const AHEAD: usize = 64;
const DELAY: usize = AHEAD - 1 + TruePeak::DELAY;
const RING: usize = 128; // a power of two above DELAY

#[derive(Debug, Clone)]
pub struct Limiter {
    ceiling: f32,
    rel: f32,
    tp: TruePeak,
    // Running minimum over the look-ahead: (sample number, wanted gain).
    minq: [(u64, f32); AHEAD + 1],
    head: usize,
    len: usize,
    n: u64,
    env: f32,
    // Box average over the look-ahead.
    boxed: [f32; AHEAD],
    box_at: usize,
    box_sum: f64,
    delay: [[f32; RING]; 2],
    d_at: usize,
    /// Deepest gain reduction in the last block (dB).
    pub gr_meter: f32,
}

impl Limiter {
    pub fn new(rate: f32, ceiling_db: f32, release_ms: f32) -> Limiter {
        Limiter {
            ceiling: 10f32.powf(ceiling_db / 20.0),
            rel: super::dynamics::coef(rate, release_ms),
            tp: TruePeak::new(),
            minq: [(0, 1.0); AHEAD + 1],
            head: 0,
            len: 0,
            n: 0,
            env: 1.0,
            boxed: [1.0; AHEAD],
            box_at: 0,
            box_sum: AHEAD as f64,
            delay: [[0.0; RING]; 2],
            d_at: 0,
            gr_meter: 0.0,
        }
    }

    fn push_min(&mut self, v: f32) {
        let cap = AHEAD + 1;
        // Drop from the back everything not lower than the new value.
        while self.len > 0 {
            let back = (self.head + self.len - 1) % cap;
            if self.minq[back].1 >= v {
                self.len -= 1;
            } else {
                break;
            }
        }
        self.minq[(self.head + self.len) % cap] = (self.n, v);
        self.len += 1;
        // Drop from the front what fell out of the window.
        while self.minq[self.head].0 + (AHEAD as u64) <= self.n {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mut deepest = 1f32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let peak = self.tp.push(*a, *b);
            let want = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.push_min(want);
            self.n += 1;
            let held = self.minq[self.head].1;
            self.env = if held < self.env {
                held
            } else {
                held + self.rel * (self.env - held)
            };
            self.box_sum += f64::from(self.env) - f64::from(self.boxed[self.box_at]);
            self.boxed[self.box_at] = self.env;
            self.box_at = (self.box_at + 1) % AHEAD;
            let g = (self.box_sum / AHEAD as f64) as f32;
            deepest = deepest.min(g);
            self.delay[0][self.d_at] = *a;
            self.delay[1][self.d_at] = *b;
            let out = (self.d_at + RING - DELAY) % RING;
            *a = self.delay[0][out] * g;
            *b = self.delay[1][out] * g;
            self.d_at = (self.d_at + 1) % RING;
        }
        self.gr_meter = 20.0 * deepest.log10();
    }
}
