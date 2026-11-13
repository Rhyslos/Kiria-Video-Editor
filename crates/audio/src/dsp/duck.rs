//! Ducking: music and other tracks turned down while someone speaks.
//!
//! The key is the dialogue tracks' sound, after their own effects and
//! faders. [`Key`] listens to it in the speech band (250 Hz to a few kHz,
//! so a bass line or a rumble in the dialogue doesn't count) and works out
//! its level per sample, once per block for every ducked track. Each
//! ducked track's [`Ducker`] then decides per sample whether someone is
//! speaking (over its threshold, or was within its hold), moves towards
//! down or up at its own speeds, and turns its sound down by its depth.

use super::biquad::{Biquad, Coefs};
use super::{from_db, smoothing, to_db};

/// The speech band's level of the key, per sample.
#[derive(Debug, Clone)]
pub struct Key {
    band: [Biquad; 2],
    mean_square: f32,
    smoothing: f32,
}

impl Key {
    pub fn new(rate: f32) -> Key {
        Key {
            band: [
                Biquad::new(Coefs::high_pass(rate, 250.0, std::f32::consts::FRAC_1_SQRT_2)),
                Biquad::new(Coefs::band_pass(rate, 1_200.0, 0.5)),
            ],
            mean_square: 0.0,
            // A 10 ms window: quick enough to catch a word's start.
            smoothing: smoothing(rate, 10.0),
        }
    }

    /// Fills `level_db` (per sample, the speech band's RMS level in dBFS)
    /// from the key's two channels.
    pub fn listen(&mut self, l: &[f32], r: &[f32], level_db: &mut [f32]) {
        for ((a, b), out) in l.iter().zip(r).zip(level_db.iter_mut()) {
            let mut x = 0.5 * (a + b);
            for f in &mut self.band {
                x = f.tick(0, x);
            }
            self.mean_square = x * x + self.smoothing * (self.mean_square - x * x);
            *out = 0.5 * to_db(self.mean_square);
        }
    }
}

/// A ducked track's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuckSettings {
    pub on: bool,
    /// The key's speech level over which someone counts as speaking (dBFS).
    pub threshold_db: f32,
    /// How long it stays down after the speaking stops (ms), so it doesn't
    /// come up between words.
    pub hold_ms: f32,
    /// How fast it goes down (ms).
    pub down_ms: f32,
    /// How fast it comes back up (ms).
    pub up_ms: f32,
    /// How far down while someone speaks (dB, 0 or less).
    pub depth_db: f32,
}

impl DuckSettings {
    /// Off; ready to duck music under a voice when turned on.
    pub const OFF: DuckSettings = DuckSettings {
        on: false,
        threshold_db: -40.0,
        hold_ms: 300.0,
        down_ms: 40.0,
        up_ms: 400.0,
        depth_db: -12.0,
    };
}

#[derive(Debug, Clone)]
pub struct Ducker {
    settings: DuckSettings,
    rate: f32,
    hold: u32,
    down: f32,
    up: f32,
    /// Samples of hold left.
    held: u32,
    /// How much it is ducked now, 0 (not) to 1 (by the whole depth).
    amount: f32,
}

impl Ducker {
    pub fn new(rate: f32, settings: DuckSettings) -> Ducker {
        let mut d = Ducker {
            settings,
            rate,
            hold: 0,
            down: 0.0,
            up: 0.0,
            held: 0,
            amount: 0.0,
        };
        d.derive();
        d
    }

    fn derive(&mut self) {
        let s = self.settings;
        self.hold = (s.hold_ms.max(0.0) * 0.001 * self.rate) as u32;
        self.down = smoothing(self.rate, s.down_ms);
        self.up = smoothing(self.rate, s.up_ms);
    }

    pub fn settings(&self) -> &DuckSettings {
        &self.settings
    }

    pub fn set(&mut self, settings: DuckSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.derive();
        }
    }

    /// How far down it is now (dB, 0 or less), for the meter.
    pub fn ducked_db(&self) -> f32 {
        self.settings.depth_db * self.amount
    }

    /// Turns the block down by how much the key speaks (`key_db`, from
    /// [`Key::listen`]). Off, it comes back up at its up speed first, so
    /// turning it off never clicks.
    pub fn process(&mut self, key_db: &[f32], l: &mut [f32], r: &mut [f32]) {
        if !self.settings.on && self.amount == 0.0 {
            return;
        }
        let DuckSettings {
            on,
            threshold_db,
            depth_db,
            ..
        } = self.settings;
        let mut amount = self.amount;
        for ((a, b), level) in l.iter_mut().zip(r.iter_mut()).zip(key_db) {
            let speaking = on && *level > threshold_db;
            if speaking {
                self.held = self.hold;
            } else {
                self.held = self.held.saturating_sub(1);
            }
            let want = if speaking || (on && self.held > 0) {
                1.0
            } else {
                0.0
            };
            let c = if want > amount { self.down } else { self.up };
            amount = want + c * (amount - want);
            if !on && amount < 1e-4 {
                amount = 0.0;
            }
            let g = from_db(depth_db * amount);
            *a *= g;
            *b *= g;
        }
        self.amount = amount;
    }

    pub fn take_state(&mut self, old: &Ducker) {
        self.held = old.held;
        self.amount = old.amount;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signal::sine;

    const RATE: f32 = 48_000.0;

    /// The key's level for `n` samples of a 1 kHz tone at `amplitude`,
    /// then `quiet` samples of silence.
    fn key_level(amplitude: f32, n: usize, quiet: usize) -> Vec<f32> {
        let mut key = Key::new(RATE);
        let mut x = sine(RATE, 1_000.0, amplitude, n);
        x.extend(std::iter::repeat_n(0.0, quiet));
        let mut level = vec![0f32; x.len()];
        key.listen(&x, &x, &mut level);
        level
    }

    #[test]
    fn the_key_hears_speech_frequencies_and_not_rumble() {
        let tone = key_level(0.1, 9_600, 0);
        // A sine's RMS is 3 dB under its peak: -23 dBFS for 0.1.
        assert!((tone[9_000] + 23.0).abs() < 1.0, "{}", tone[9_000]);
        let mut key = Key::new(RATE);
        let rumble = sine(RATE, 50.0, 0.5, 9_600);
        let mut level = vec![0f32; rumble.len()];
        key.listen(&rumble, &rumble, &mut level);
        assert!(level[9_000] < -40.0, "{}", level[9_000]);
    }

    fn settings() -> DuckSettings {
        DuckSettings {
            on: true,
            ..DuckSettings::OFF
        }
    }

    #[test]
    fn it_goes_down_by_its_depth_holds_then_comes_back() {
        let rate = RATE as usize;
        // One second of speech-level tone, then two of silence.
        let level = key_level(0.1, rate, 2 * rate);
        let mut d = Ducker::new(RATE, settings());
        let mut l = vec![1f32; level.len()];
        let mut r = l.clone();
        for ((k, a), b) in level.chunks(256).zip(l.chunks_mut(256)).zip(r.chunks_mut(256)) {
            d.process(k, a, b);
        }
        let db = |i: usize| to_db(l[i]);
        // 40 ms down speed: 63 % of the way 40 ms after the key starts
        // (and a little for the key's own 10 ms).
        assert!(db(rate / 2) < -11.9, "fully down: {}", db(rate / 2));
        assert!(db(rate * 40 / 1000) < -6.0, "{}", db(rate * 40 / 1000));
        // Held down for 300 ms after the speaking stops.
        assert!(db(rate + rate / 4) < -11.5, "still held: {}", db(rate + rate / 4));
        // Then back up at the up speed: a second later, two thirds of the
        // way (400 ms is 63 %), nearly all of it by the end.
        let up_from = rate + rate * 3 / 10 + rate / 10;
        assert!(
            db(up_from + rate * 4 / 10) > -6.0,
            "{}",
            db(up_from + rate * 4 / 10)
        );
        assert!(db(3 * rate - 1) > -0.5, "{}", db(3 * rate - 1));
        let r_db = to_db(r[rate / 2]);
        assert_eq!(r_db, db(rate / 2), "both channels alike");
    }

    #[test]
    fn under_the_threshold_nothing_happens_and_off_comes_back_up() {
        let quiet = key_level(0.001, 48_000, 0);
        let mut d = Ducker::new(RATE, settings());
        let mut l = vec![1f32; quiet.len()];
        let mut r = l.clone();
        d.process(&quiet, &mut l, &mut r);
        assert!(l.iter().all(|v| (*v - 1.0).abs() < 1e-6));
        // Down, then turned off: it rises rather than jumping.
        let loud = key_level(0.1, 48_000, 0);
        d.process(&loud, &mut l, &mut r);
        assert!(d.ducked_db() < -11.0);
        d.set(DuckSettings::OFF);
        let mut l = vec![1f32; 256];
        let mut r = l.clone();
        d.process(&loud[..256], &mut l, &mut r);
        assert!(to_db(l[0]) < -11.0, "no jump");
        let n = 48_000 * 5;
        let mut l = vec![1f32; n];
        let mut r = l.clone();
        d.process(&vec![-100.0; n], &mut l, &mut r);
        assert_eq!(l[n - 1], 1.0, "all the way back, then nothing to do");
    }
}
