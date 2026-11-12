//! Measuring the master output while it plays (see [`crate::loudness`]
//! for what is measured and how): the peak and true peak of each channel,
//! the loudness over the last 0.4 s (momentary) and 3 s (short-term), and
//! the integrated loudness and highest true peak since playback started
//! or since the interface asked to start them again.
//!
//! The audio thread feeds every mixed block in; the interface reads the
//! latest values through [`Reading`] without locking, and asks for a fresh
//! start the same way.

use crate::loudness::{Loudness, Measured};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// One look at the meter. Silence reads as `-inf`.
pub type Levels = Measured;

/// The newest values, shared with the interface.
#[derive(Debug, Default)]
pub struct Reading {
    values: [AtomicU32; 8],
    /// The interface asked for the integrated loudness and the highest
    /// true peak to start again.
    reset: AtomicBool,
}

impl Reading {
    pub fn new() -> Arc<Reading> {
        let r = Arc::new(Reading::default());
        r.store(Levels::SILENT);
        r
    }

    pub fn levels(&self) -> Levels {
        let get = |i: usize| f32::from_bits(self.values[i].load(Ordering::Relaxed));
        Levels {
            peak_db: [get(0), get(1)],
            true_peak_db: [get(2), get(3)],
            true_peak_max: get(4),
            momentary: get(5),
            short_term: get(6),
            integrated: get(7),
        }
    }

    /// Asks the meter to start the integrated loudness and the highest
    /// true peak again, from the next block.
    pub fn reset(&self) {
        self.reset.store(true, Ordering::Relaxed);
        let silent = Levels::SILENT;
        self.values[4].store(silent.true_peak_max.to_bits(), Ordering::Relaxed);
        self.values[7].store(silent.integrated.to_bits(), Ordering::Relaxed);
    }

    fn store(&self, l: Levels) {
        let all = [
            l.peak_db[0],
            l.peak_db[1],
            l.true_peak_db[0],
            l.true_peak_db[1],
            l.true_peak_max,
            l.momentary,
            l.short_term,
            l.integrated,
        ];
        for (slot, value) in self.values.iter().zip(all) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Measures interleaved stereo as it is fed in and publishes the values.
#[derive(Debug)]
pub struct Meter {
    loudness: Loudness,
    reading: Arc<Reading>,
}

impl Meter {
    /// Made before the audio starts: it allocates (the integrated
    /// loudness's histogram); feeding never does.
    pub fn new(rate: u32, reading: Arc<Reading>) -> Meter {
        Meter {
            loudness: Loudness::new(rate),
            reading,
        }
    }

    /// Takes in one mixed block (interleaved left, right) and publishes the
    /// new levels.
    pub fn feed(&mut self, samples: &[f32]) {
        if self.reading.reset.swap(false, Ordering::Relaxed) {
            self.loudness.reset();
        }
        let levels = self.loudness.feed(samples);
        self.reading.store(levels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, amplitude: f32, seconds: f64) -> Vec<f32> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amplitude
                    * (2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / f64::from(rate)).sin() as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn the_interface_reads_what_was_measured_and_can_start_it_again() {
        let reading = Reading::new();
        let mut meter = Meter::new(48_000, Arc::clone(&reading));
        for block in sine(48_000, 0.1, 3.5).chunks(2048) {
            meter.feed(block);
        }
        let l = reading.levels();
        assert!((l.momentary + 20.0).abs() < 0.3, "{l:?}");
        assert!((l.short_term + 20.0).abs() < 0.3, "{l:?}");
        assert!((l.integrated + 20.0).abs() < 0.3, "{l:?}");
        assert!((l.peak_db[0] + 20.0).abs() < 0.1);
        assert!((l.true_peak_max + 20.0).abs() < 0.2);
        reading.reset();
        assert_eq!(reading.levels().integrated, f32::NEG_INFINITY, "at once");
        meter.feed(&vec![0.0; 9_600]);
        assert_eq!(
            reading.levels().integrated,
            f32::NEG_INFINITY,
            "only silence since"
        );
    }

    #[test]
    fn silence_reads_minus_infinity() {
        let reading = Reading::new();
        let mut meter = Meter::new(48_000, Arc::clone(&reading));
        meter.feed(&vec![0.0; 48_000]);
        let l = reading.levels();
        assert_eq!(l.momentary, f32::NEG_INFINITY);
        assert_eq!(l.peak_db[0], f32::NEG_INFINITY);
        assert_eq!(l.true_peak_max, f32::NEG_INFINITY);
    }
}
