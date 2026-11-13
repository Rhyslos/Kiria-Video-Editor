//! Ducking: music turned down while someone speaks.
//!
//! One detector listens to the key (the dialogue tracks' sound, after
//! their own effects and faders) in the speech band, and says per sample
//! how much speech there is (0 to 1), with a hold so the music doesn't
//! come up between words. Each ducked track turns itself down by its own
//! depth times that amount.

use super::biquad::{Biquad, Coefs};
use super::dynamics::{coef, from_db, to_db};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeySettings {
    pub threshold_db: f32,
    pub hold_ms: f32,
    /// How fast the music goes down / comes back (ms).
    pub down_ms: f32,
    pub up_ms: f32,
}

#[derive(Debug, Clone)]
pub struct KeyDetector {
    pub s: KeySettings,
    rate: f32,
    band: [Biquad; 2],
    rms: f32,
    rms_c: f32,
    held: usize,
    hold: usize,
    act: f32,
    down: f32,
    up: f32,
}

impl KeyDetector {
    pub fn new(rate: f32, s: KeySettings) -> KeyDetector {
        let mut k = KeyDetector {
            s,
            rate,
            band: [
                Biquad::new(Coefs::highpass(rate, 250.0, 0.707)),
                Biquad::new(Coefs::bandpass(rate, 1200.0, 0.5)),
            ],
            rms: 0.0,
            rms_c: coef(rate, 10.0),
            held: 0,
            hold: 0,
            act: 0.0,
            down: 0.0,
            up: 0.0,
        };
        k.derive();
        k
    }

    fn derive(&mut self) {
        self.hold = (self.s.hold_ms * 0.001 * self.rate) as usize;
        self.down = coef(self.rate, self.s.down_ms);
        self.up = coef(self.rate, self.s.up_ms);
    }

    /// Fills `activity` (one value per sample) from the key's two channels.
    pub fn detect(&mut self, l: &[f32], r: &[f32], activity: &mut [f32]) {
        for ((a, b), out) in l.iter().zip(r).zip(activity.iter_mut()) {
            let mut x = 0.5 * (a + b);
            x = self.band[0].tick(0, x);
            x = self.band[1].tick(0, x);
            self.rms = x * x + self.rms_c * (self.rms - x * x);
            let speaking = 0.5 * to_db(self.rms) > self.s.threshold_db;
            if speaking {
                self.held = self.hold;
            } else {
                self.held = self.held.saturating_sub(1);
            }
            let want = if speaking || self.held > 0 { 1.0 } else { 0.0 };
            let c = if want > self.act { self.down } else { self.up };
            self.act = want + c * (self.act - want);
            *out = self.act;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ducker {
    /// How far down at full speech (dB, ≤ 0).
    pub depth_db: f32,
}

impl Ducker {
    pub fn process(&mut self, activity: &[f32], l: &mut [f32], r: &mut [f32]) {
        let d = self.depth_db;
        for ((a, b), act) in l.iter_mut().zip(r.iter_mut()).zip(activity) {
            let g = from_db(d * act);
            *a *= g;
            *b *= g;
        }
    }
}
