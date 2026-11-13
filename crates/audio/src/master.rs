//! The master: the mix's volume, compressor, trim (export's normalising)
//! and true-peak limiter, and the meter (true peak; momentary, short-term and integrated loudness).
//!
//! The limiter always runs its delay, on or off (off, its ceiling is out
//! of reach), so the mix is always late by the same amount and turning it
//! on or off while playing never jumps.

use crate::BLOCK;
use crate::dsp::dynamics::Compressor;
use crate::dsp::limiter::Limiter;
use crate::dsp::loudness::Loudness;
use crate::dsp::volume_gain;
use crate::readout::MasterReadout;
use crate::settings::{MasterChange, MasterSettings};

#[derive(Debug)]
pub struct Master {
    settings: MasterSettings,
    compressor: Compressor,
    limiter: Limiter,
    meter: Loudness,
    /// The volume's gain as the last block ended.
    gain: f32,
}

impl Master {
    /// Made off the real-time thread: it allocates (the meter's histogram).
    pub fn new(rate: u32, settings: MasterSettings) -> Master {
        let mut limiter = Limiter::new(rate, settings.limiter.ceiling_db);
        if !settings.limiter.on {
            limiter.set_ceiling(f32::INFINITY);
        }
        Master {
            settings,
            compressor: Compressor::new(rate as f32, settings.compressor),
            limiter,
            meter: Loudness::new(rate),
            gain: volume_gain(settings.volume_db),
        }
    }

    pub fn settings(&self) -> &MasterSettings {
        &self.settings
    }

    /// The deepest the limiter has turned the mix down since made, dB (0
    /// when never).
    pub fn limited_db(&self) -> f32 {
        self.limiter.deepest_db()
    }

    /// How late the limiter makes the mix, in frames.
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }

    pub fn apply(&mut self, change: MasterChange) {
        self.settings.apply(change);
        match change {
            MasterChange::Volume(_) | MasterChange::Trim(_) => {}
            MasterChange::Compressor(c) => self.compressor.set(c),
            MasterChange::Limiter(l) => {
                self.limiter
                    .set_ceiling(if l.on { l.ceiling_db } else { f32::INFINITY })
            }
        }
    }

    /// Takes the summed tracks and writes the finished block, interleaved.
    pub fn process(
        &mut self,
        l: &mut [f32; BLOCK],
        r: &mut [f32; BLOCK],
        out: &mut [f32; 2 * BLOCK],
        readout: &MasterReadout,
    ) {
        let (from, to) = (self.gain, volume_gain(self.settings.volume_db));
        self.gain = to;
        let step = (to - from) / BLOCK as f32;
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let g = from + step * (i + 1) as f32;
            *a *= g;
            *b *= g;
        }
        self.compressor.process(l, r);
        if self.settings.trim_db != 0.0 {
            // Set before mixing (export), so it never needs a glide.
            let trim = volume_gain(self.settings.trim_db);
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                *a *= trim;
                *b *= trim;
            }
        }
        for (frame, (a, b)) in out.chunks_exact_mut(2).zip(l.iter().zip(r.iter())) {
            frame[0] = *a;
            frame[1] = *b;
        }
        self.limiter.process(out);
        if readout.take_reset() {
            self.meter.reset();
        }
        readout.put_levels(self.meter.feed(out));
        readout.put_reductions(self.compressor.reduction_db(), self.limiter.take_recent_db());
    }
}
