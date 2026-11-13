//! A track's strip: every effect of the Audio page, always there, each on
//! or off, run in the page's order:
//!
//! high pass → clean-up → EQ (the analyser listens here) → compressor →
//! Loudness → ducking → volume, pan and lines (mute and solo) → meter.
//!
//! Turning an effect on or off is a flag, never an allocation, so it
//! travels as a plain command. The clean-up always runs its delay, so
//! every track is late by the same [`crate::dsp::cleanup::LATENCY`].

use crate::analyser::Tap;
use crate::dsp::cleanup::{BinNoise, SpectralGate};
use crate::dsp::duck::Ducker;
use crate::dsp::dynamics::Compressor;
use crate::dsp::filters::{Eq, HighPass};
use crate::dsp::level::Leveller;
use crate::dsp::lines::{Fader, Lines};
use crate::meter::TrackMeter;
use crate::readout::TrackReadout;
use crate::settings::{TrackChange, TrackSettings};

#[derive(Debug)]
pub struct Strip {
    settings: TrackSettings,
    high_pass: HighPass,
    cleanup: SpectralGate,
    eq: Eq,
    compressor: Compressor,
    level: Leveller,
    duck: Ducker,
    fader: Fader,
    meter: TrackMeter,
}

impl Strip {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: u32, settings: TrackSettings, lines: Box<Lines>, noise: Box<BinNoise>) -> Strip {
        let r = rate as f32;
        let s = settings;
        Strip {
            settings,
            high_pass: HighPass::new(r, s.high_pass.freq),
            cleanup: SpectralGate::new(rate, s.cleanup, noise),
            eq: Eq::new(r, s.eq.bands),
            compressor: Compressor::new(r, s.compressor),
            level: Leveller::new(r, s.level),
            duck: Ducker::new(r, s.duck),
            fader: Fader::new(s.volume_db, s.pan, !s.mute, lines),
            meter: TrackMeter::new(rate),
        }
    }

    pub fn settings(&self) -> &TrackSettings {
        &self.settings
    }

    /// Applies one change; only what it touches is worked out again. Never
    /// allocates.
    pub fn apply(&mut self, change: TrackChange) {
        self.settings.apply(change);
        match change {
            TrackChange::HighPass(h) => self.high_pass.set(h.freq),
            TrackChange::EqBand(i, b) => self.eq.set_band(usize::from(i), b),
            TrackChange::Compressor(c) => self.compressor.set(c),
            TrackChange::Cleanup(c) => self.cleanup.set(c),
            TrackChange::Duck(d) => self.duck.set(d),
            TrackChange::Level(l) => self.level.set(l),
            TrackChange::Volume(v) => self.fader.volume_db = v,
            TrackChange::Pan(p) => self.fader.pan = p,
            TrackChange::Key(_) | TrackChange::EqOn(_) | TrackChange::Mute(_) | TrackChange::Solo(_) => {}
        }
    }

    /// Puts in new lines; the old come back in `lines`.
    pub fn swap_lines(&mut self, lines: &mut Box<Lines>) {
        self.fader.swap_lines(lines);
    }

    /// Puts in new noise for the clean-up; the old comes back in `noise`.
    pub fn swap_noise(&mut self, noise: &mut Box<BinNoise>) {
        self.cleanup.swap_noise(noise);
    }

    /// Whether it plays a part in the ducking key.
    pub fn is_key(&self) -> bool {
        self.settings.key
    }

    /// Runs the block starting `pos` samples from the timeline's start.
    /// `key_db` is the ducking key's level per sample (unused on key tracks);
    /// `audible` says whether mute and solo let it be heard; `tap`, when
    /// given, gets the sound after the EQ.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        pos: u64,
        key_db: &[f32],
        audible: bool,
        tap: Option<&mut Tap>,
        l: &mut [f32],
        r: &mut [f32],
        readout: &TrackReadout,
    ) {
        let s = &self.settings;
        if s.high_pass.on {
            self.high_pass.process(l, r);
        }
        self.cleanup.process(l, r);
        if s.eq.on {
            self.eq.process(l, r);
        }
        if let Some(tap) = tap {
            tap.send(l, r);
        }
        self.compressor.process(l, r);
        self.level.process(l, r);
        if !s.key {
            self.duck.process(key_db, l, r);
        }
        self.fader.audible = audible;
        self.fader.process(pos, l, r);
        self.meter.process(l, r, readout);
        readout.put_effects(
            self.compressor.reduction_db(),
            self.cleanup.reduction_db(),
            self.duck.ducked_db(),
            self.level.gain_db(),
        );
    }

    /// Carries everything that runs on from block to block (filter
    /// memories, envelopes, the clean-up's windows, the meter) over from
    /// the strip this one replaces, so swapping in a new one never clicks.
    pub fn take_state(&mut self, old: &Strip) {
        self.high_pass.take_state(&old.high_pass);
        self.cleanup.take_state(&old.cleanup);
        self.eq.take_state(&old.eq);
        self.compressor.take_state(&old.compressor);
        self.level.take_state(&old.level);
        self.duck.take_state(&old.duck);
        self.fader.take_state(&old.fader);
        self.meter.take_state(&old.meter);
    }
}
