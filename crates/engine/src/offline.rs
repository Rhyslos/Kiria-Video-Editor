//! The timeline's sound mixed without a sound card, for export: the same
//! feeder and the same real-time mixer (`crates/audio`) as playback, run
//! block after block on the caller's thread, so an export sounds exactly as
//! playback does from the same place, track effects and master included.
//!
//! The mixer's fixed delay ([`audio::Mixer::latency`]) is mixed ahead and
//! thrown away, so the first sample out is the stretch's first sample. The
//! thread treats very small numbers as zero, as the callback does.
//!
//! A nested timeline's sound is made the same way ([`OfflineMix::nested`]):
//! by the feeder of the track its clip is on, on the feeder's thread, so it
//! sounds exactly as that timeline exported on its own.

use crate::feeder::{self, Feeder, MixShape, PIECE};
use audio::settings::{MasterSettings, sample_at};
use audio::{BLOCK, Controller, Mixer};
use media::audio::CHANNELS;
use project::{Project, SequenceId, Time};

pub struct OfflineMix {
    mixer: Mixer,
    controller: Controller,
    feeder: Feeder,
    /// The master's settings as last sent.
    master: MasterSettings,
    /// The timeline's mix as last taken, for a nested timeline following
    /// edits ([`OfflineMix::follow`]).
    shape: Option<MixShape>,
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
        let at = sample_at(start, rate);
        OfflineMix::build(project, sequence, rate, (at, start.as_secs_f64()), master, 0)
    }

    /// As [`OfflineMix::new`], for measuring: the same sound, without the
    /// meters of its tracks and master (nobody watches them, and the
    /// measuring has a meter of its own).
    pub fn measuring(
        project: &Project,
        sequence: SequenceId,
        rate: u32,
        start: Time,
        master: MasterSettings,
    ) -> diag::Result<OfflineMix> {
        let mut mix = OfflineMix::new(project, sequence, rate, start, master)?;
        mix.mixer.without_meters();
        Ok(mix)
    }

    /// A nested timeline's sound, `depth` timelines deep, from its sample
    /// `at` at `rate` (the rate that plays it at its clip's speed): the
    /// timeline as it sounds opened, its own master included.
    pub(crate) fn nested(
        project: &Project,
        sequence: SequenceId,
        rate: u32,
        at: u64,
        depth: usize,
    ) -> diag::Result<OfflineMix> {
        let master = project
            .sequence(sequence)
            .map_or(MasterSettings::default(), MasterSettings::from_project);
        let secs = at as f64 / f64::from(rate.max(1));
        let mut mix = OfflineMix::build(project, sequence, rate, (at, secs), master, depth)?;
        mix.shape = MixShape::of(project, sequence);
        Ok(mix)
    }

    fn build(
        project: &Project,
        sequence: SequenceId,
        rate: u32,
        (at, at_secs): (u64, f64),
        master: MasterSettings,
        depth: usize,
    ) -> diag::Result<OfflineMix> {
        let slots = project.sequence(sequence).map_or(64, feeder::slots_for);
        let (mut mixer, mut controller, _analyser) = audio::mixer(rate, slots, master);
        let feeder = Feeder::at_depth(
            project.clone(),
            sequence,
            &mut controller,
            (slots, PIECE),
            (at, at_secs),
            true,
            depth,
        )?;
        controller.locate(at)?;
        mixer.idle();
        let ahead = mixer.latency();
        Ok(OfflineMix {
            mixer,
            controller,
            feeder,
            master,
            shape: None,
            block: Box::new([0.0; 2 * BLOCK]),
            used: BLOCK,
            ahead,
        })
    }

    /// The next `frames` frames, interleaved stereo, into `out` (resized).
    pub fn take(&mut self, frames: usize, out: &mut Vec<f32>) {
        out.clear();
        out.resize(frames * CHANNELS, 0.0);
        self.fill(None, out);
    }

    /// The next frames into all of `out` (interleaved stereo), the clips
    /// taken from `project` when given (see [`Feeder::fill_from`]).
    pub(crate) fn fill(&mut self, project: Option<&Project>, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        let mut done = 0;
        while done < frames {
            if self.used == BLOCK {
                self.next_block(project);
                continue;
            }
            let n = (frames - done).min(BLOCK - self.used);
            out[done * CHANNELS..(done + n) * CHANNELS]
                .copy_from_slice(&self.block[self.used * CHANNELS..(self.used + n) * CHANNELS]);
            self.used += n;
            done += n;
        }
    }

    /// Takes the project as it is now, when its mix changed since last
    /// taken: track and master settings travel as commands, tracks come and
    /// go (see [`Feeder::update`]). Cheap otherwise: a comparison.
    pub(crate) fn follow(&mut self, project: &Project) {
        let sequence = self.feeder.sequence();
        if self.shape.as_ref().is_some_and(|s| s.matches(project, sequence)) {
            return;
        }
        let Some(seq) = project.sequence(sequence) else {
            return;
        };
        self.shape = MixShape::of(project, sequence);
        let master = MasterSettings::from_project(seq);
        for change in master.changes_from(&self.master) {
            if let Err(err) = self.controller.change_master(change) {
                tracing::warn!("{}", err.report());
            }
        }
        self.master = master;
        if let Err(err) = self.feeder.update(project.clone(), &mut self.controller) {
            tracing::warn!("{}", err.report());
        }
    }

    /// Mixes the next block, throwing away what is still the mixer's delay.
    /// Very small numbers count as zero on this thread, as in the sound
    /// card's callback: effects dying away in silence would otherwise get
    /// many times slower (and export would differ from playback there).
    fn next_block(&mut self, project: Option<&Project>) {
        os::float::denormals_as_zero();
        self.feeder.fill_from(project, &|| false);
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
