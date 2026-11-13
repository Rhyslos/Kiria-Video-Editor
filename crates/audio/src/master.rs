//! The master: the mix's volume, compressor and true-peak limiter, and the
//! meter (true peak; momentary, short-term and integrated loudness).
//!
//! The limiter always runs its delay, on or off (off, its ceiling is out
//! of reach), so the mix is always late by the same amount and turning it
//! on or off while playing never jumps.

use crate::BLOCK;
use crate::dsp::dynamics::Compressor;
use crate::dsp::from_db;
use crate::dsp::limiter::Limiter;
use crate::dsp::loudness::Loudness;
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
            gain: from_db(settings.volume_db),
        }
    }

    pub fn settings(&self) -> &MasterSettings {
        &self.settings
    }

    /// How late the limiter makes the mix, in frames.
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }

    pub fn apply(&mut self, change: MasterChange) {
        self.settings.apply(change);
        match change {
            MasterChange::Volume(_) => {}
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
        let (from, to) = (self.gain, from_db(self.settings.volume_db));
        self.gain = to;
        let step = (to - from) / BLOCK as f32;
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let g = from + step * (i + 1) as f32;
            *a *= g;
            *b *= g;
        }
        self.compressor.process(l, r);
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
