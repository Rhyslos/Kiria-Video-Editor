//! A sound clip's own EQ and clean-up (`project::clip_fx`), run by the
//! mixer on the clip's samples as they are read, before its volume, pan,
//! lines and fades.
//!
//! Each clip being played keeps its effects' state here from piece to
//! piece, so playback and export, which read the same pieces, make the
//! same sound. The clean-up delays what goes through it by
//! [`audio::dsp::cleanup::LATENCY`] samples, so a clip with it on is read
//! that far ahead and comes out in place; after a jump (or once it is
//! turned on) that much is read first and thrown away, filling the gate.

use audio::dsp::cleanup::{LATENCY, NoiseProfile, SpectralGate};
use audio::dsp::filters::Eq;
use audio::settings::{TrackSettings, noise_from_project};
use media::audio::CHANNELS;
use project::clip_fx::ClipFx;
use project::{Clip, ClipId};

/// How many clips keep their effects' state.
const KEPT: usize = 32;

/// A clip's effects and where they are.
pub(crate) struct Chain {
    clip: ClipId,
    fx: ClipFx,
    eq: Eq,
    eq_on: bool,
    gate: Option<SpectralGate>,
    /// The timeline sample (at the mixer's rate) the next sample out is.
    next: i64,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Chain {
    fn new(rate: u32, clip: &Clip, next: i64) -> Chain {
        let settings = TrackSettings::from_sound(&project::TrackSound {
            eq: clip.fx.eq,
            cleanup: clip.fx.cleanup.clone(),
            ..Default::default()
        });
        let gate = clip.fx.cleanup.on.then(|| {
            let noise = noise_of(&clip.fx).unwrap_or(NoiseProfile::SILENT);
            SpectralGate::new(rate, settings.cleanup, noise.bins(rate))
        });
        Chain {
            clip: clip.id,
            fx: clip.fx.clone(),
            eq: Eq::new(rate as f32, settings.eq.bands),
            eq_on: settings.eq.on,
            gate,
            next,
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    /// How far ahead its clip is read: the clean-up's delay, when it is on.
    pub(crate) fn latency(&self) -> usize {
        if self.gate.is_some() { LATENCY } else { 0 }
    }

    /// Runs interleaved stereo `samples` through the EQ and the clean-up.
    pub(crate) fn process(&mut self, samples: &mut [f32]) {
        let frames = samples.len() / CHANNELS;
        self.left.resize(frames, 0.0);
        self.right.resize(frames, 0.0);
        for (i, frame) in samples.chunks_exact(CHANNELS).enumerate() {
            self.left[i] = frame[0];
            self.right[i] = frame[1];
        }
        if self.eq_on {
            self.eq.process(&mut self.left, &mut self.right);
        }
        if let Some(gate) = &mut self.gate {
            gate.process(&mut self.left, &mut self.right);
        }
        for (i, frame) in samples.chunks_exact_mut(CHANNELS).enumerate() {
            frame[0] = self.left[i];
            frame[1] = self.right[i];
        }
    }

    /// Takes in a change of its clip's settings: the EQ's and the clean-up's
    /// numbers carry on; turning the clean-up on or off, or a new noise,
    /// starts it afresh (`false`).
    fn follow(&mut self, clip: &Clip) -> bool {
        if self.fx == clip.fx {
            return true;
        }
        let cleanup_same =
            self.fx.cleanup.on == clip.fx.cleanup.on && self.fx.cleanup.profile == clip.fx.cleanup.profile;
        if !cleanup_same {
            return false;
        }
        let settings = TrackSettings::from_sound(&project::TrackSound {
            eq: clip.fx.eq,
            cleanup: clip.fx.cleanup.clone(),
            ..Default::default()
        });
        for (i, band) in settings.eq.bands.iter().enumerate() {
            self.eq.set_band(i, *band);
        }
        self.eq_on = settings.eq.on;
        if let Some(gate) = &mut self.gate {
            gate.set(settings.cleanup);
        }
        self.fx = clip.fx.clone();
        true
    }
}

fn noise_of(fx: &ClipFx) -> Option<NoiseProfile> {
    noise_from_project(&project::TrackSound {
        cleanup: fx.cleanup.clone(),
        ..Default::default()
    })
}

/// The chains of the clips being played, most recent first.
#[derive(Default)]
pub(crate) struct Chains {
    kept: Vec<Chain>,
}

impl Chains {
    /// `clip`'s chain for the samples from timeline sample `at` on, taken
    /// out (put it back with [`Chains::put`]), and whether it is fresh:
    /// new, after a jump, or started afresh by a change, so its delay must
    /// be filled first.
    pub(crate) fn take(&mut self, rate: u32, clip: &Clip, at: i64) -> (Chain, bool) {
        if let Some(i) = self.kept.iter().position(|c| c.clip == clip.id) {
            let mut chain = self.kept.remove(i);
            if chain.next == at && chain.follow(clip) {
                return (chain, false);
            }
        }
        (Chain::new(rate, clip, at), true)
    }

    /// Keeps `chain`, whose next sample out is `next`.
    pub(crate) fn put(&mut self, mut chain: Chain, next: i64) {
        chain.next = next;
        self.kept.insert(0, chain);
        self.kept.truncate(KEPT);
    }
}

impl std::fmt::Debug for Chains {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chains").field("kept", &self.kept.len()).finish()
    }
}
