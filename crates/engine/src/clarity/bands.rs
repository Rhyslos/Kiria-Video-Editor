//! Third-octave bands from 160 Hz to 8 kHz, how much each matters to
//! understanding speech, and the power of a piece of sound in each.

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::ops::Range;
use std::sync::Arc;

/// How many bands.
pub const COUNT: usize = 18;

/// Each band's centre, Hz (the standard third-octave series).
pub const CENTRES: [f32; COUNT] = [
    160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1_000.0, 1_250.0, 1_600.0, 2_000.0, 2_500.0,
    3_150.0, 4_000.0, 5_000.0, 6_300.0, 8_000.0,
];

/// How much each band matters to following speech, adding up to 1: the
/// band-importance numbers for average speech of the third-octave method in
/// the published Speech Intelligibility Index standard (ANSI S3.5-1997,
/// Table 3), typed in from the standard.
pub const IMPORTANCE: [f32; COUNT] = [
    0.0083, 0.0095, 0.0150, 0.0289, 0.0440, 0.0578, 0.0653, 0.0711, 0.0818, 0.0844, 0.0882, 0.0898, 0.0868,
    0.0844, 0.0771, 0.0527, 0.0364, 0.0185,
];

/// The bands words are understood by: 315 Hz to 4 kHz.
pub const SPEECH: Range<usize> = 3..15;
/// The low middle, where sound turns muddy: 200 to 500 Hz.
pub const MUD: Range<usize> = 1..6;
/// The presence bands the low middle is weighed against: 1 to 4 kHz.
pub const PRESENCE: Range<usize> = 8..15;

/// Band `i`'s lower and upper edges, Hz.
pub fn edges(i: usize) -> (f32, f32) {
    let c = CENTRES[i.min(COUNT - 1)];
    let sixth = 2f32.powf(1.0 / 6.0);
    (c / sixth, c * sixth)
}

/// How many octaves bands `range` cover together.
pub fn octaves(range: &Range<usize>) -> f32 {
    range.len() as f32 / 3.0
}

/// Powers in dB (10 log10), with silence at -200 dB.
pub fn db(power: f32) -> f32 {
    if power > 1e-20 {
        10.0 * power.log10()
    } else {
        -200.0
    }
}

/// Samples per analysed window.
pub const WINDOW: usize = 2048;

/// Measures windows of [`WINDOW`] stereo samples: their mean-square power
/// in each band (a sine of peak 1 inside a band reads 0.5 there).
pub struct BandMeter {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// The bins of each band (none for a band above half the rate).
    bins: [Range<usize>; COUNT],
    scale: f32,
}

impl std::fmt::Debug for BandMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BandMeter({:?})", self.bins)
    }
}

impl BandMeter {
    pub fn new(rate: u32) -> BandMeter {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let window: Vec<f32> = (0..WINDOW)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / WINDOW as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        let mean_square = window.iter().map(|w| w * w).sum::<f32>() / WINDOW as f32;
        let bin_hz = rate as f32 / WINDOW as f32;
        let last = WINDOW / 2;
        let bins = std::array::from_fn(|i| {
            let (low, high) = edges(i);
            let first = ((low / bin_hz).ceil() as usize).clamp(1, last + 1);
            let end = ((high / bin_hz).ceil() as usize).clamp(first, last + 1);
            if first == end && first <= last {
                // Narrower than a bin: the bin nearest its centre.
                let near = ((CENTRES[i] / bin_hz).round() as usize).clamp(1, last);
                near..near + 1
            } else {
                first..end
            }
        });
        BandMeter {
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            input: vec![0.0; WINDOW],
            fft,
            window,
            bins,
            // Parseval for one side of the spectrum, undoing the window.
            scale: 2.0 / (WINDOW as f32 * WINDOW as f32 * mean_square),
        }
    }

    /// The power in each band of `l` and `r` (each [`WINDOW`] long), the
    /// mean of the two sides.
    pub fn measure(&mut self, l: &[f32], r: &[f32]) -> [f32; COUNT] {
        let mut out = [0f32; COUNT];
        for side in [l, r] {
            for ((i, x), w) in self.input.iter_mut().zip(side).zip(&self.window) {
                *i = x * w;
            }
            if self
                .fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
                .is_err()
            {
                // Only for buffers of the wrong length, which these never are.
                continue;
            }
            for (o, bins) in out.iter_mut().zip(&self.bins) {
                let sum: f32 = self
                    .spectrum
                    .get(bins.clone())
                    .map_or(0.0, |s| s.iter().map(|c| c.norm_sqr()).sum());
                *o += 0.5 * sum * self.scale;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_importance_adds_up_to_one() {
        let sum: f32 = IMPORTANCE.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
    }

    #[test]
    fn a_sine_lands_in_its_band_at_its_power() {
        let rate = 48_000;
        let mut meter = BandMeter::new(rate);
        for (i, freq) in [(3, 315.0f32), (12, 2_500.0), (8, 1_000.0)] {
            let s = audio::test_signal::sine(rate as f32, freq, 1.0, WINDOW);
            let p = meter.measure(&s, &s);
            assert!((db(p[i]) - db(0.5)).abs() < 1.0, "{freq} Hz: {} dB", db(p[i]));
            let others: f32 = p
                .iter()
                .enumerate()
                .filter(|(j, _)| j.abs_diff(i) > 1)
                .map(|(_, v)| v)
                .sum();
            assert!(db(others) < db(p[i]) - 30.0, "{freq} Hz leaks");
        }
    }
}
