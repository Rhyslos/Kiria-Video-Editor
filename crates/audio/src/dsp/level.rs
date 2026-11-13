//! The Loudness tool on a track: keeps it near a loudness, slowly, the way
//! a person riding a fader would. Quiet stretches come up, loud ones go
//! down, within limits; pauses (under -50 LUFS) leave the gain where it
//! was, so the noise floor never comes up between words.
//!
//! The track's sound is K-weighted (as loudness is measured) and its
//! energy smoothed over about 0.4 s; every 64 samples the gain moves a
//! little towards what would bring that to the target (about two thirds of
//! the way in 1.5 s), and between those steps it glides sample by sample.
//! A quicker measure (50 ms) notices a pause at once: when it falls under
//! -50 LUFS, or 15 dB under the slow one, the gain is held.

use super::biquad::{Biquad, Coefs};
use super::{from_db, smoothing};

/// The Loudness tool's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelSettings {
    pub on: bool,
    /// The loudness to keep the track at (LUFS).
    pub target_lufs: f32,
    /// At most this much up (dB).
    pub max_boost_db: f32,
    /// At most this much down (dB).
    pub max_cut_db: f32,
}

impl LevelSettings {
    pub const OFF: LevelSettings = LevelSettings {
        on: false,
        target_lufs: -18.0,
        max_boost_db: 12.0,
        max_cut_db: 12.0,
    };
}

/// Quieter than this counts as a pause: the gain is held.
const PAUSE_LUFS: f32 = -50.0;
/// The quick measure this far under the slow one is a pause too.
const DROP_DB: f32 = 15.0;
/// Samples between steps of the gain.
const STEP: usize = 64;

#[derive(Debug, Clone)]
pub struct Leveller {
    settings: LevelSettings,
    weighting: [Biquad; 2],
    energy: f32,
    energy_smoothing: f32,
    quick: f32,
    quick_smoothing: f32,
    gain_smoothing: f32,
    gain_db: f32,
    /// The linear gains at the start and end of the current step.
    from: f32,
    to: f32,
    /// Samples into the current step.
    into_step: usize,
}

impl Leveller {
    pub fn new(rate: f32, settings: LevelSettings) -> Leveller {
        let (shelf, high_pass) = Coefs::k_weighting(rate);
        Leveller {
            settings,
            weighting: [Biquad::new(shelf), Biquad::new(high_pass)],
            energy: 0.0,
            energy_smoothing: smoothing(rate, 400.0),
            quick: 0.0,
            quick_smoothing: smoothing(rate, 50.0),
            gain_smoothing: smoothing(rate / STEP as f32, 1_500.0),
            gain_db: 0.0,
            from: 1.0,
            to: 1.0,
            into_step: 0,
        }
    }

    pub fn settings(&self) -> &LevelSettings {
        &self.settings
    }

    pub fn set(&mut self, settings: LevelSettings) {
        self.settings = settings;
    }

    /// The gain it applies now (dB).
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let on = self.settings.on;
        if !on && self.gain_db == 0.0 {
            return;
        }
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            if self.into_step == 0 {
                self.from = self.to;
                self.step();
                self.to = from_db(self.gain_db);
            }
            if on {
                let [shelf, high_pass] = &mut self.weighting;
                let ka = high_pass.tick(0, shelf.tick(0, *a));
                let kb = high_pass.tick(1, shelf.tick(1, *b));
                let e = ka * ka + kb * kb;
                self.energy = e + self.energy_smoothing * (self.energy - e);
                self.quick = e + self.quick_smoothing * (self.quick - e);
            }
            self.into_step += 1;
            let g = self.from + (self.to - self.from) * (self.into_step as f32 / STEP as f32);
            if self.into_step == STEP {
                self.into_step = 0;
            }
            *a *= g;
            *b *= g;
        }
    }

    /// One step of the gain towards what the target wants (towards 0 dB
    /// when off).
    fn step(&mut self) {
        let s = self.settings;
        let want = if !s.on {
            0.0
        } else {
            let lufs = |e: f32| -0.691 + 10.0 * (e + 1e-12).log10();
            let (loudness, now) = (lufs(self.energy), lufs(self.quick));
            if now < PAUSE_LUFS || now < loudness - DROP_DB {
                return;
            }
            (s.target_lufs - loudness).clamp(-s.max_cut_db.max(0.0), s.max_boost_db.max(0.0))
        };
        self.gain_db = want + self.gain_smoothing * (self.gain_db - want);
        if !s.on && self.gain_db.abs() < 0.01 {
            self.gain_db = 0.0;
        }
    }

    pub fn take_state(&mut self, old: &Leveller) {
        self.weighting[0].take_state(&old.weighting[0]);
        self.weighting[1].take_state(&old.weighting[1]);
        self.energy = old.energy;
        self.quick = old.quick;
        self.gain_db = old.gain_db;
        self.from = old.from;
        self.to = old.to;
        self.into_step = old.into_step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signal::sine;

    const RATE: f32 = 48_000.0;

    fn run(lev: &mut Leveller, amplitude: f32, seconds: usize) -> Vec<f32> {
        let mut l = sine(RATE, 1_000.0, amplitude, RATE as usize * seconds);
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            lev.process(a, b);
        }
        l
    }

    #[test]
    fn a_quiet_track_comes_up_to_the_target_within_its_limits() {
        let on = LevelSettings {
            on: true,
            ..LevelSettings::OFF
        };
        // 1 kHz at -30 dBFS peak in both channels: about -30 LUFS. Target -18.
        let mut lev = Leveller::new(RATE, on);
        run(&mut lev, from_db(-30.0), 12);
        assert!((lev.gain_db() - 12.0).abs() < 0.3, "{}", lev.gain_db());
        // A loud one goes down, but no further than its cut.
        let mut lev = Leveller::new(RATE, on);
        run(&mut lev, from_db(-3.0), 12);
        assert!((lev.gain_db() + 12.0).abs() < 0.3, "{}", lev.gain_db());
        let mut lev = Leveller::new(RATE, on);
        run(&mut lev, from_db(-24.0), 12);
        assert!((lev.gain_db() - 6.0).abs() < 0.3, "{}", lev.gain_db());
        // Silence holds it.
        run(&mut lev, 0.0, 3);
        assert!((lev.gain_db() - 6.0).abs() < 0.3, "{}", lev.gain_db());
    }

    #[test]
    fn off_it_does_nothing_and_turning_off_glides_back() {
        let mut lev = Leveller::new(RATE, LevelSettings::OFF);
        let out = run(&mut lev, 0.5, 1);
        assert_eq!(out, sine(RATE, 1_000.0, 0.5, RATE as usize));
        lev.set(LevelSettings {
            on: true,
            ..LevelSettings::OFF
        });
        run(&mut lev, from_db(-30.0), 10);
        lev.set(LevelSettings::OFF);
        let out = run(&mut lev, 0.01, 1);
        let step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(step < 0.05, "glides: {step}");
        run(&mut lev, 0.01, 20);
        assert_eq!(lev.gain_db(), 0.0);
    }
}
