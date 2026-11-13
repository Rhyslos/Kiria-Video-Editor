//! Meters: per track (peak and momentary loudness), and the true peak
//! (four times oversampled) the limiter and the master meter use.

use super::biquad::{Biquad, Coefs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// What a track publishes for the interface, without locks.
#[derive(Debug, Default)]
pub struct Readout {
    pub peak_db: AtomicU32,
    pub momentary: AtomicU32,
}

impl Readout {
    pub fn put(slot: &AtomicU32, v: f32) {
        slot.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn get(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.load(Ordering::Relaxed))
    }
}

/// Peak and momentary loudness (K-weighted, 0.4 s) of a track.
#[derive(Debug, Clone)]
pub struct TrackMeter {
    k: [Biquad; 2],
    sum: f64,
    count: usize,
    per_block: usize,
    blocks: [f64; 4],
    next: usize,
    pub readout: Arc<Readout>,
}

impl TrackMeter {
    pub fn new(rate: f32) -> TrackMeter {
        let (shelf, high) = Coefs::k_weighting(rate);
        TrackMeter {
            k: [Biquad::new(shelf), Biquad::new(high)],
            sum: 0.0,
            count: 0,
            per_block: (rate / 10.0) as usize,
            blocks: [0.0; 4],
            next: 0,
            readout: Arc::new(Readout::default()),
        }
    }

    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        let mut peak = 0f32;
        for (&a, &b) in l.iter().zip(r) {
            peak = peak.max(a.abs()).max(b.abs());
            let (sa, sb) = (self.k[0].tick(0, a), self.k[0].tick(1, b));
            let ka = self.k[1].tick(0, sa);
            let kb = self.k[1].tick(1, sb);
            self.sum += f64::from(ka * ka + kb * kb);
            self.count += 1;
            if self.count == self.per_block {
                self.blocks[self.next] = self.sum / self.count as f64;
                self.next = (self.next + 1) % 4;
                self.sum = 0.0;
                self.count = 0;
                let mean = self.blocks.iter().sum::<f64>() / 4.0;
                Readout::put(
                    &self.readout.momentary,
                    (-0.691 + 10.0 * (mean + 1e-12).log10()) as f32,
                );
            }
        }
        Readout::put(&self.readout.peak_db, 20.0 * (peak + 1e-9).log10());
    }

    /// Takes the old meter's readout too, by swapping: assigning would
    /// free this meter's own readout here, on the real-time thread (the
    /// spike's allocation counter caught exactly that).
    pub fn take_state(&mut self, old: &mut TrackMeter) {
        self.k = old.k;
        self.sum = old.sum;
        self.count = old.count;
        self.blocks = old.blocks;
        self.next = old.next;
        std::mem::swap(&mut self.readout, &mut old.readout);
    }
}

const TAPS: usize = 16;

/// The highest of a sample and the three points between it and the next,
/// found by four-times oversampling (windowed sinc, three phases of 16
/// taps; the first phase is the samples themselves). The answer is for
/// the sample `TruePeak::DELAY` samples back.
#[derive(Debug, Clone)]
pub struct TruePeak {
    phases: [[f32; TAPS]; 3],
    hist: [[f32; 2 * TAPS]; 2],
    at: usize,
}

impl Default for TruePeak {
    fn default() -> TruePeak {
        TruePeak::new()
    }
}

impl TruePeak {
    pub const DELAY: usize = TAPS / 2;

    pub fn new() -> TruePeak {
        let mut phases = [[0f32; TAPS]; 3];
        for (p, phase) in phases.iter_mut().enumerate() {
            let frac = (p + 1) as f64 / 4.0;
            for (j, c) in phase.iter_mut().enumerate() {
                // Distance from the point between samples to tap j.
                let d = j as f64 - (TAPS / 2 - 1) as f64 - frac;
                let sinc = if d == 0.0 {
                    1.0
                } else {
                    (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d)
                };
                let w = 0.5 * (1.0 + (std::f64::consts::PI * d / (TAPS as f64 / 2.0 + 0.5)).cos());
                *c = (sinc * w) as f32;
            }
        }
        TruePeak {
            phases,
            hist: [[0.0; 2 * TAPS]; 2],
            at: 0,
        }
    }

    /// Takes one stereo sample; returns the true peak (linear) of both
    /// channels around the sample `DELAY` back.
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) -> f32 {
        let at = self.at;
        self.hist[0][at] = l;
        self.hist[0][at + TAPS] = l;
        self.hist[1][at] = r;
        self.hist[1][at + TAPS] = r;
        self.at = (at + 1) % TAPS;
        let mut peak = 0f32;
        for h in &self.hist {
            let w = &h[self.at..self.at + TAPS]; // oldest first
            peak = peak.max(w[TAPS / 2 - 1].abs());
            for ph in &self.phases {
                let mut s = 0f32;
                for j in 0..TAPS {
                    s += w[j] * ph[j];
                }
                peak = peak.max(s.abs());
            }
        }
        peak
    }
}
