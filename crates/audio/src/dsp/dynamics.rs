//! The compressor, on each track and on the master: it turns the sound
//! down by a share of how far it goes over a threshold, so loud and quiet
//! moments come closer together.
//!
//! Feed-forward, both channels linked (the louder one decides, so the
//! stereo picture never moves), peak detection, a soft knee, and the gain
//! smoothed in decibels: it goes down at the attack speed and comes back
//! at the release speed.

use super::{from_db, smoothing, to_db};

/// A compressor's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorSettings {
    pub on: bool,
    /// Where it starts to act, dBFS.
    pub threshold_db: f32,
    /// How much of what goes over is kept: 4 means 4 dB over comes out 1 dB over.
    pub ratio: f32,
    /// How fast it turns down (ms).
    pub attack_ms: f32,
    /// How fast it lets go (ms).
    pub release_ms: f32,
    /// How wide the bend at the threshold is (dB); 0 is a sharp corner.
    pub knee_db: f32,
    /// Gain added after, to make up for what was taken (dB).
    pub makeup_db: f32,
}

impl CompressorSettings {
    /// Off; the numbers are the Voice preset's, ready for when it is turned on.
    pub const OFF: CompressorSettings = CompressorSettings {
        on: false,
        ..Preset::Voice.settings()
    };

    /// The gain change, in dB (0 or less), for a level in dBFS: the static
    /// curve, before the attack and release. The page draws it.
    pub fn curve(&self, level_db: f32) -> f32 {
        let over = level_db - self.threshold_db;
        let slope = 1.0 / self.ratio.max(1.0) - 1.0;
        let knee = self.knee_db.max(0.0);
        if 2.0 * over <= -knee {
            0.0
        } else if 2.0 * over >= knee {
            slope * over
        } else {
            // Inside the knee: a parabola joining the two straight parts.
            let into = over + knee / 2.0;
            slope * into * into / (2.0 * knee)
        }
    }
}

/// Plain starting points, named for what they are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Barely noticeable: takes the edge off the loudest moments.
    Gentle,
    /// Keeps a voice even, so quiet words are heard and shouts don't jump out.
    Voice,
    /// Holds music or game sound together without squashing its punch.
    Music,
    /// Strong and even: everything at about the same level.
    Strong,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Gentle, Preset::Voice, Preset::Music, Preset::Strong];

    /// The preset's settings, turned on.
    pub const fn settings(self) -> CompressorSettings {
        let (threshold_db, ratio, attack_ms, release_ms, knee_db, makeup_db) = match self {
            Preset::Gentle => (-18.0, 1.5, 20.0, 250.0, 6.0, 1.0),
            Preset::Voice => (-24.0, 3.0, 5.0, 120.0, 6.0, 4.0),
            Preset::Music => (-20.0, 2.0, 15.0, 200.0, 8.0, 2.0),
            Preset::Strong => (-30.0, 6.0, 2.0, 80.0, 4.0, 8.0),
        };
        CompressorSettings {
            on: true,
            threshold_db,
            ratio,
            attack_ms,
            release_ms,
            knee_db,
            makeup_db,
        }
    }

    /// Its name on the page.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Gentle => "Gentle",
            Preset::Voice => "Voice",
            Preset::Music => "Music",
            Preset::Strong => "Strong",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Compressor {
    settings: CompressorSettings,
    rate: f32,
    attack: f32,
    release: f32,
    /// The gain change now, dB (0 or less).
    gain_db: f32,
    /// The deepest gain change in the last block (dB), for the meter.
    deepest_db: f32,
}

impl Compressor {
    pub fn new(rate: f32, settings: CompressorSettings) -> Compressor {
        Compressor {
            settings,
            rate,
            attack: smoothing(rate, settings.attack_ms),
            release: smoothing(rate, settings.release_ms),
            gain_db: 0.0,
            deepest_db: 0.0,
        }
    }

    pub fn settings(&self) -> &CompressorSettings {
        &self.settings
    }

    pub fn set(&mut self, settings: CompressorSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.attack = smoothing(self.rate, settings.attack_ms);
            self.release = smoothing(self.rate, settings.release_ms);
            if !settings.on {
                self.gain_db = 0.0;
                self.deepest_db = 0.0;
            }
        }
    }

    /// How far it turned the sound down in the last block, in dB (0 or
    /// less, before the makeup gain): the gain-reduction meter.
    pub fn reduction_db(&self) -> f32 {
        self.deepest_db
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.settings.on {
            return;
        }
        let makeup = self.settings.makeup_db;
        let (attack, release) = (self.attack, self.release);
        let mut gain = self.gain_db;
        let mut deepest = 0f32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let want = self.settings.curve(to_db(a.abs().max(b.abs())));
            let c = if want < gain { attack } else { release };
            gain = want + c * (gain - want);
            deepest = deepest.min(gain);
            let g = from_db(gain + makeup);
            *a *= g;
            *b *= g;
        }
        self.gain_db = gain;
        self.deepest_db = deepest;
    }

    pub fn take_state(&mut self, old: &Compressor) {
        self.gain_db = old.gain_db;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn settings(ratio: f32, knee_db: f32) -> CompressorSettings {
        CompressorSettings {
            on: true,
            threshold_db: -20.0,
            ratio,
            attack_ms: 1.0,
            release_ms: 50.0,
            knee_db,
            makeup_db: 0.0,
        }
    }

    #[test]
    fn the_curve_keeps_a_share_of_what_goes_over() {
        let s = settings(4.0, 0.0);
        assert_eq!(s.curve(-30.0), 0.0);
        assert_eq!(s.curve(-20.0), 0.0);
        // 14 dB over at 4:1 comes out 3.5 dB over: 10.5 dB down.
        assert!((s.curve(-6.0) + 10.5).abs() < 1e-4);
        // A knee bends it smoothly: halfway in, a quarter of the full change.
        let soft = settings(4.0, 10.0);
        assert_eq!(soft.curve(-25.0), 0.0);
        assert!(
            (soft.curve(-15.0) - s.curve(-15.0)).abs() < 1e-4,
            "past the knee it's the same"
        );
        let mid = soft.curve(-20.0);
        assert!((mid - 0.25 * s.curve(-15.0)).abs() < 1e-4, "{mid}");
        assert!(mid < 0.0 && mid > s.curve(-15.0));
    }

    fn square(level_db: f32, n: usize) -> Vec<f32> {
        let amp = from_db(level_db);
        (0..n).map(|i| if i / 50 % 2 == 0 { amp } else { -amp }).collect()
    }

    #[test]
    fn a_steady_tone_settles_at_the_curve_and_the_meter_says_so() {
        let mut c = Compressor::new(RATE, settings(4.0, 0.0));
        let mut l = square(-6.0, 24_000);
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            c.process(a, b);
        }
        assert!((c.reduction_db() + 10.5).abs() < 0.1, "{}", c.reduction_db());
        let out = to_db(l[23_999].abs());
        assert!((out + 16.5).abs() < 0.1, "{out}");
    }

    #[test]
    fn it_turns_down_at_the_attack_speed_and_comes_back_at_the_release() {
        let s = CompressorSettings {
            attack_ms: 10.0,
            release_ms: 100.0,
            ..settings(4.0, 0.0)
        };
        let mut c = Compressor::new(RATE, s);
        let mut l = square(-6.0, 480);
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        // After one attack time, 63 % of the way to -10.5 dB.
        assert!((c.gain_db + 0.632 * 10.5).abs() < 0.1, "{}", c.gain_db);
        let mut l = square(-6.0, 48_000);
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        let mut quiet = vec![0f32; 4_800];
        let mut quiet_r = quiet.clone();
        c.process(&mut quiet, &mut quiet_r);
        // One release time after it went quiet, 37 % of the way down is left.
        assert!((c.gain_db + 0.368 * 10.5).abs() < 0.1, "{}", c.gain_db);
    }

    #[test]
    fn off_and_presets() {
        let mut c = Compressor::new(RATE, CompressorSettings::OFF);
        let mut l = square(0.0, 256);
        let before = l.clone();
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        assert_eq!(l, before);
        for p in Preset::ALL {
            let s = p.settings();
            assert!(s.on && s.ratio >= 1.0 && s.threshold_db < 0.0, "{}", p.name());
            assert!(s.curve(0.0) < 0.0);
        }
    }
}
