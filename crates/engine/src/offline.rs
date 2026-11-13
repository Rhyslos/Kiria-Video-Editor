//! The timeline's sound mixed without a sound card, for export: the same
//! feeder and the same real-time mixer (`crates/audio`) as playback, run
//! block after block on the caller's thread, so an export sounds exactly as
//! playback does from the same place, track effects and master included.
//!
//! The mixer's fixed delay ([`audio::Mixer::latency`]) is mixed ahead and
//! thrown away, so the first sample out is the stretch's first sample.

use crate::feeder::{self, Feeder, PIECE};
use audio::settings::{MasterSettings, sample_at};
use audio::{BLOCK, Controller, Mixer};
use media::audio::CHANNELS;
use project::{Project, SequenceId, Time};

pub struct OfflineMix {
    mixer: Mixer,
    controller: Controller,
    feeder: Feeder,
    block: Box<[f32; 2 * BLOCK]>,
    /// Frames of `block` already given out.
    used: usize,
    /// Frames of the mixer's delay still to throw away.
    ahead: usize,
}

impl std::fmt::Debug for OfflineMix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OfflineMix({:?})", self.feeder)
    }
}

impl OfflineMix {
    /// Mixes `sequence` from `start` at `rate`, through a master with
    /// `master`'s settings (the timeline's own, or export's with its
    /// normalising and limiter).
    pub fn new(
        project: &Project,
        sequence: SequenceId,
        rate: u32,
        start: Time,
        master: MasterSettings,
    ) -> diag::Result<OfflineMix> {
        let slots = project.sequence(sequence).map_or(64, feeder::slots_for);
        let (mut mixer, mut controller, _analyser) = audio::mixer(rate, slots, master);
        let at = sample_at(start, rate);
        let feeder = Feeder::new(
            project.clone(),
            sequence,
            &mut controller,
            (slots, PIECE),
            (at, start.as_secs_f64()),
            true,
        )?;
        controller.locate(at)?;
        mixer.idle();
        let ahead = mixer.latency();
        Ok(OfflineMix {
            mixer,
            controller,
            feeder,
            block: Box::new([0.0; 2 * BLOCK]),
            used: BLOCK,
            ahead,
        })
    }

    /// The next `frames` frames, interleaved stereo, into `out` (resized).
    pub fn take(&mut self, frames: usize, out: &mut Vec<f32>) {
        out.clear();
        while out.len() < frames * CHANNELS {
            if self.used == BLOCK {
                self.next_block();
                continue;
            }
            let want = frames - out.len() / CHANNELS;
            let n = want.min(BLOCK - self.used);
            out.extend_from_slice(&self.block[self.used * CHANNELS..(self.used + n) * CHANNELS]);
            self.used += n;
        }
    }

    /// Mixes the next block, throwing away what is still the mixer's delay.
    fn next_block(&mut self) {
        self.feeder.fill(&|| false);
        self.mixer.process(&mut self.block);
        self.controller.collect();
        let skip = self.ahead.min(BLOCK);
        self.ahead -= skip;
        self.used = skip;
    }

    /// The deepest the master's limiter turned the sound down so far, dB.
    pub fn limited_db(&self) -> f32 {
        self.mixer.limited_db()
    }
}
