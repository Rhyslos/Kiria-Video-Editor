//! Made-up sound for the tests: a small random number generator and a
//! speech-like signal (from the audio engine spike).

use crate::dsp::biquad::{Biquad, Coefs};

/// A small, fast, repeatable random number generator (xorshift).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in [-1, 1).
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

/// `n` samples of something like speech: a buzz at a wandering pitch
/// through three moving resonances (now and then a hiss instead), in
/// syllables of about a quarter second and phrases of 1.5 to 3 s with
/// pauses between. Also says, per sample, whether it is speaking.
pub fn speech(rate: f32, n: usize, seed: u64) -> (Vec<f32>, Vec<bool>) {
    let mut rng = Rng::new(seed);
    let mut out = vec![0f32; n];
    let mut talking = vec![false; n];
    let mut formants = [Biquad::default(); 3];
    let mut at = (0.2 * rate) as usize;
    let mut phase = 0f32;
    while at < n {
        let end = (at + ((1.5 + 1.5 * rng.unit()) * rate) as usize).min(n);
        let mut syllable = at;
        while syllable < end {
            let len = ((0.15 + 0.15 * rng.unit()) * rate) as usize;
            let resonances = [
                300.0 + 500.0 * rng.unit(),
                900.0 + 1_500.0 * rng.unit(),
                2_300.0 + 800.0 * rng.unit(),
            ];
            for (f, freq) in formants.iter_mut().zip(resonances) {
                f.coefs = Coefs::band_pass(rate, freq, 6.0);
            }
            let pitch = 110.0 + 90.0 * rng.unit();
            let hissed = rng.unit() < 0.2;
            for i in syllable..(syllable + len).min(end) {
                let t = (i - syllable) as f32 / len as f32;
                let envelope = (std::f32::consts::PI * t).sin().powf(0.7);
                phase += pitch / rate;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                let x = if hissed {
                    rng.signed() * 0.3
                } else if phase < 0.05 {
                    1.0
                } else {
                    -0.05
                };
                let y: f32 = formants.iter_mut().map(|f| f.tick(0, x)).sum();
                out[i] = y * envelope * 0.6;
                talking[i] = true;
            }
            syllable += len;
        }
        at = end + ((0.3 + 0.7 * rng.unit()) * rate) as usize;
    }
    (out, talking)
}

/// `n` samples of a sine at `freq` and peak `amplitude`.
pub fn sine(rate: f32, freq: f32, amplitude: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin())
        .collect()
}
