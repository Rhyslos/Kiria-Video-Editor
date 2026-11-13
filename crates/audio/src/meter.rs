//! A track's meter: its sample peaks, and its momentary (0.4 s) and
//! short-term (3 s) loudness, K-weighted as the loudness standard says.
//! Cheaper than the master's meter: no true peak and no integrated
//! loudness, which a track's strip doesn't show.

use crate::dsp::biquad::{Biquad, Coefs};
use crate::readout::TrackReadout;

/// 100 ms blocks kept: enough for the 3 s short-term window.
const BLOCKS: usize = 30;
/// 100 ms blocks in the momentary window.
const MOMENTARY: usize = 4;

#[derive(Debug, Clone)]
pub struct TrackMeter {
    weighting: [Biquad; 2],
    filling: f64,
    filled: usize,
    block_len: usize,
    blocks: [f64; BLOCKS],
    done: usize,
}

fn lufs(mean_square: f64) -> f32 {
    if mean_square <= 0.0 {
        f32::NEG_INFINITY
    } else {
        (-0.691 + 10.0 * mean_square.log10()) as f32
    }
}

impl TrackMeter {
    pub fn new(rate: u32) -> TrackMeter {
        let (shelf, high_pass) = Coefs::k_weighting(rate as f32);
        TrackMeter {
            weighting: [Biquad::new(shelf), Biquad::new(high_pass)],
            filling: 0.0,
            filled: 0,
            block_len: (rate / 10).max(1) as usize,
            blocks: [0.0; BLOCKS],
            done: 0,
        }
    }

    fn mean_of_last(&self, n: usize) -> Option<f64> {
        (self.done >= n).then(|| {
            (0..n)
                .map(|b| self.blocks[(self.done - 1 - b) % BLOCKS])
                .sum::<f64>()
                / n as f64
        })
    }

    /// Measures a block and publishes the results.
    pub fn process(&mut self, l: &[f32], r: &[f32], readout: &TrackReadout) {
        let mut peak = [0f32; 2];
        for (&a, &b) in l.iter().zip(r) {
            peak = [peak[0].max(a.abs()), peak[1].max(b.abs())];
            let [shelf, high_pass] = &mut self.weighting;
            let ka = high_pass.tick(0, shelf.tick(0, a));
            let kb = high_pass.tick(1, shelf.tick(1, b));
            self.filling += f64::from(ka * ka + kb * kb);
            self.filled += 1;
            if self.filled == self.block_len {
                self.blocks[self.done % BLOCKS] = self.filling / self.block_len as f64;
                self.done += 1;
                self.filling = 0.0;
                self.filled = 0;
            }
        }
        readout.put_peak(peak);
        let momentary = self.mean_of_last(MOMENTARY).map_or(f32::NEG_INFINITY, lufs);
        let short = if self.done >= MOMENTARY {
            self.mean_of_last(self.done.min(BLOCKS))
                .map_or(f32::NEG_INFINITY, lufs)
        } else {
            f32::NEG_INFINITY
        };
        readout.put_loudness(momentary, short);
    }

    pub fn take_state(&mut self, old: &TrackMeter) {
        self.weighting[0].take_state(&old.weighting[0]);
        self.weighting[1].take_state(&old.weighting[1]);
        self.filling = old.filling;
        self.filled = old.filled;
        self.blocks = old.blocks;
        self.done = old.done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signal::sine;

    #[test]
    fn a_reference_tone_reads_as_the_standard_says() {
        // 1 kHz at -23 dBFS in both channels: -23 LUFS.
        let tone = sine(48_000.0, 1_000.0, 10f32.powf(-23.0 / 20.0), 48_000 * 4);
        let mut meter = TrackMeter::new(48_000);
        let readout = TrackReadout::default();
        for block in tone.chunks(256) {
            meter.process(block, block, &readout);
        }
        let l = readout.take();
        assert!((l.momentary + 23.0).abs() < 0.1, "{l:?}");
        assert!((l.short_term + 23.0).abs() < 0.1, "{l:?}");
        assert!((l.peak_db[0] + 23.0).abs() < 0.05, "{l:?}");
    }
}
