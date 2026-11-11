//! Measuring the master output while it plays: the peak of each channel and
//! the loudness a listener hears (K-weighted, as broadcast loudness meters
//! measure it), over the last 0.4 s (momentary) and 3 s (short-term).
//!
//! The audio thread feeds every mixed block in; the interface reads the
//! latest values through [`Reading`] without locking.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The newest values, shared with the interface. Silence reads as `-inf`.
#[derive(Debug, Default)]
pub struct Reading {
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    momentary: AtomicU32,
    short_term: AtomicU32,
}

/// One look at the meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// Highest sample of each channel in the last block, in dB below full scale.
    pub peak_db: [f32; 2],
    /// Loudness over the last 0.4 s, in LUFS.
    pub momentary: f32,
    /// Loudness over the last 3 s, in LUFS.
    pub short_term: f32,
}

impl Levels {
    pub const SILENT: Levels = Levels {
        peak_db: [f32::NEG_INFINITY; 2],
        momentary: f32::NEG_INFINITY,
        short_term: f32::NEG_INFINITY,
    };
}

impl Reading {
    pub fn new() -> Arc<Reading> {
        let r = Arc::new(Reading::default());
        r.store(Levels::SILENT);
        r
    }

    pub fn levels(&self) -> Levels {
        let get = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        Levels {
            peak_db: [get(&self.peak_left), get(&self.peak_right)],
            momentary: get(&self.momentary),
            short_term: get(&self.short_term),
        }
    }

    fn store(&self, l: Levels) {
        self.peak_left.store(l.peak_db[0].to_bits(), Ordering::Relaxed);
        self.peak_right.store(l.peak_db[1].to_bits(), Ordering::Relaxed);
        self.momentary.store(l.momentary.to_bits(), Ordering::Relaxed);
        self.short_term.store(l.short_term.to_bits(), Ordering::Relaxed);
    }
}

/// A two-pole filter section.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for `rate` samples per second: a high shelf
/// (the head's effect) and a high pass.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    [shelf, pass]
}

/// Blocks of 100 ms kept: enough for the 3 s short-term window.
const BLOCKS: usize = 30;

/// Measures interleaved stereo as it is fed in.
#[derive(Debug)]
pub struct Meter {
    filters: [[Biquad; 2]; 2],
    block_len: usize,
    /// Weighted energy and samples in the 100 ms block being filled.
    filling: (f64, usize),
    /// Energy of the finished blocks, newest last.
    blocks: Vec<f64>,
    reading: Arc<Reading>,
}

impl Meter {
    pub fn new(rate: u32, reading: Arc<Reading>) -> Meter {
        let k = k_weighting(f64::from(rate));
        Meter {
            filters: [k, k],
            block_len: (rate / 10).max(1) as usize,
            filling: (0.0, 0),
            blocks: Vec::with_capacity(BLOCKS),
            reading,
        }
    }

    /// Takes in one mixed block (interleaved left, right) and publishes the
    /// new levels.
    pub fn feed(&mut self, samples: &[f32]) {
        let mut peak = [0f32; 2];
        for frame in samples.chunks_exact(2) {
            let mut energy = 0.0;
            for (ch, s) in frame.iter().enumerate() {
                peak[ch] = peak[ch].max(s.abs());
                let [a, b] = &mut self.filters[ch];
                let y = b.run(a.run(f64::from(*s)));
                energy += y * y;
            }
            self.filling.0 += energy;
            self.filling.1 += 1;
            if self.filling.1 == self.block_len {
                if self.blocks.len() == BLOCKS {
                    self.blocks.remove(0);
                }
                self.blocks.push(self.filling.0 / self.block_len as f64);
                self.filling = (0.0, 0);
            }
        }
        let loudness = |n: usize| {
            let recent = &self.blocks[self.blocks.len().saturating_sub(n)..];
            if recent.len() < n.min(4) {
                return f32::NEG_INFINITY;
            }
            let mean = recent.iter().sum::<f64>() / recent.len() as f64;
            if mean <= 0.0 {
                f32::NEG_INFINITY
            } else {
                (-0.691 + 10.0 * mean.log10()) as f32
            }
        };
        let db = |p: f32| {
            if p > 0.0 {
                20.0 * p.log10()
            } else {
                f32::NEG_INFINITY
            }
        };
        self.reading.store(Levels {
            peak_db: [db(peak[0]), db(peak[1])],
            momentary: loudness(4),
            short_term: loudness(BLOCKS),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v =
                    amplitude * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn a_full_scale_tone_in_both_channels_reads_near_zero() {
        // The reference: 1 kHz at 0 dB in both channels measures about 0 LUFS.
        for rate in [44_100, 48_000, 96_000] {
            let reading = Reading::new();
            let mut meter = Meter::new(rate, Arc::clone(&reading));
            for block in sine(rate, 1_000.0, 0.1, 3.5).chunks(2048) {
                meter.feed(block);
            }
            let l = reading.levels();
            assert!((l.momentary + 20.0).abs() < 0.3, "{rate}: {}", l.momentary);
            assert!((l.short_term + 20.0).abs() < 0.3, "{rate}: {}", l.short_term);
            assert!((l.peak_db[0] + 20.0).abs() < 0.1);
        }
    }

    #[test]
    fn silence_reads_minus_infinity() {
        let reading = Reading::new();
        let mut meter = Meter::new(48_000, Arc::clone(&reading));
        meter.feed(&vec![0.0; 48_000]);
        let l = reading.levels();
        assert_eq!(l.momentary, f32::NEG_INFINITY);
        assert_eq!(l.peak_db[0], f32::NEG_INFINITY);
    }
}
