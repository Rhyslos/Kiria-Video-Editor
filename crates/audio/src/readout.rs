//! What the mixer measures, for the interface to read without locks: each
//! track's meter and how far its effects are turning it down, and the
//! master's loudness, true peak and gain reduction.
//!
//! Every value is an `f32` kept in an atomic. Peaks are held until the
//! interface takes them ([`TrackReadout::take_peak`]), so a peak between
//! two frames of the interface is never missed; the rest are the latest.

use crate::dsp::loudness::Measured;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

fn put(slot: &AtomicU32, v: f32) {
    slot.store(v.to_bits(), Ordering::Relaxed);
}

fn get(slot: &AtomicU32) -> f32 {
    f32::from_bits(slot.load(Ordering::Relaxed))
}

fn silent() -> AtomicU32 {
    AtomicU32::new(f32::NEG_INFINITY.to_bits())
}

fn zero() -> AtomicU32 {
    AtomicU32::new(0f32.to_bits())
}

/// One track's meter and effects, as last measured.
#[derive(Debug)]
pub struct TrackReadout {
    /// The highest sample of each channel since the interface last took it
    /// (linear, so the highest is kept with an atomic maximum: the bits of
    /// non-negative floats order as the floats do).
    peak: [AtomicU32; 2],
    momentary: AtomicU32,
    short_term: AtomicU32,
    compressor_db: AtomicU32,
    cleanup_db: AtomicU32,
    ducked_db: AtomicU32,
    level_db: AtomicU32,
}

impl Default for TrackReadout {
    fn default() -> TrackReadout {
        TrackReadout {
            peak: [zero(), zero()],
            momentary: silent(),
            short_term: silent(),
            compressor_db: zero(),
            cleanup_db: zero(),
            ducked_db: zero(),
            level_db: zero(),
        }
    }
}

/// One look at a track's readout. Silence reads `-inf`; reductions are dB,
/// 0 or less (the Loudness tool's gain can be either way).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackLevels {
    pub peak_db: [f32; 2],
    pub momentary: f32,
    pub short_term: f32,
    pub compressor_db: f32,
    pub cleanup_db: f32,
    pub ducked_db: f32,
    pub level_db: f32,
}

impl TrackReadout {
    /// The values now, taking the held peaks (they start again from silence).
    pub fn take(&self) -> TrackLevels {
        TrackLevels {
            peak_db: self.take_peak(),
            momentary: get(&self.momentary),
            short_term: get(&self.short_term),
            compressor_db: get(&self.compressor_db),
            cleanup_db: get(&self.cleanup_db),
            ducked_db: get(&self.ducked_db),
            level_db: get(&self.level_db),
        }
    }

    /// The highest sample of each channel since last taken, dBFS.
    pub fn take_peak(&self) -> [f32; 2] {
        self.peak
            .each_ref()
            .map(|p| crate::dsp::loudness::db(f32::from_bits(p.swap(0, Ordering::Relaxed))))
    }

    pub(crate) fn put_peak(&self, peak: [f32; 2]) {
        for (slot, p) in self.peak.iter().zip(peak) {
            slot.fetch_max(p.abs().to_bits(), Ordering::Relaxed);
        }
    }

    pub(crate) fn put_loudness(&self, momentary: f32, short_term: f32) {
        put(&self.momentary, momentary);
        put(&self.short_term, short_term);
    }

    pub(crate) fn put_effects(&self, compressor_db: f32, cleanup_db: f32, ducked_db: f32, level_db: f32) {
        put(&self.compressor_db, compressor_db);
        put(&self.cleanup_db, cleanup_db);
        put(&self.ducked_db, ducked_db);
        put(&self.level_db, level_db);
    }
}

/// The master's meter: everything [`Measured`] holds, plus how far the
/// compressor and the limiter turned it down in the last block.
#[derive(Debug)]
pub struct MasterReadout {
    values: [AtomicU32; 8],
    compressor_db: AtomicU32,
    limiter_db: AtomicU32,
    /// The interface asked for the integrated loudness and the highest true
    /// peak to start again.
    reset: AtomicBool,
}

impl Default for MasterReadout {
    fn default() -> MasterReadout {
        let r = MasterReadout {
            values: std::array::from_fn(|_| silent()),
            compressor_db: zero(),
            limiter_db: zero(),
            reset: AtomicBool::new(false),
        };
        r.put_levels(Measured::SILENT);
        r
    }
}

impl MasterReadout {
    /// The loudness and peaks now.
    pub fn levels(&self) -> Measured {
        let v = |i: usize| get(&self.values[i]);
        Measured {
            peak_db: [v(0), v(1)],
            true_peak_db: [v(2), v(3)],
            true_peak_max: v(4),
            momentary: v(5),
            short_term: v(6),
            integrated: v(7),
        }
    }

    /// How far the master's compressor turned the mix down in the last block (dB).
    pub fn compressor_db(&self) -> f32 {
        get(&self.compressor_db)
    }

    /// How far the limiter turned the mix down in the last block (dB).
    pub fn limiter_db(&self) -> f32 {
        get(&self.limiter_db)
    }

    /// Starts the integrated loudness and the highest true peak again, from
    /// the next block (they read silent at once).
    pub fn reset(&self) {
        self.reset.store(true, Ordering::Relaxed);
        put(&self.values[4], f32::NEG_INFINITY);
        put(&self.values[7], f32::NEG_INFINITY);
    }

    pub(crate) fn take_reset(&self) -> bool {
        self.reset.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn put_levels(&self, l: Measured) {
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
        for (slot, v) in self.values.iter().zip(all) {
            put(slot, v);
        }
    }

    pub(crate) fn put_reductions(&self, compressor_db: f32, limiter_db: f32) {
        put(&self.compressor_db, compressor_db);
        put(&self.limiter_db, limiter_db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_are_held_until_taken() {
        let r = TrackReadout::default();
        assert_eq!(r.take_peak(), [f32::NEG_INFINITY; 2]);
        r.put_peak([0.5, -0.25]);
        r.put_peak([0.1, 0.1]);
        let [l, rr] = r.take_peak();
        assert!((l + 6.02).abs() < 0.01 && (rr + 12.04).abs() < 0.01);
        assert_eq!(r.take_peak(), [f32::NEG_INFINITY; 2], "taken");
    }

    #[test]
    fn the_master_starts_silent_and_resets() {
        let m = MasterReadout::default();
        assert_eq!(m.levels(), Measured::SILENT);
        m.put_levels(Measured {
            integrated: -14.0,
            true_peak_max: -1.0,
            ..Measured::SILENT
        });
        m.reset();
        assert_eq!(m.levels().integrated, f32::NEG_INFINITY);
        assert!(m.take_reset() && !m.take_reset());
    }
}
