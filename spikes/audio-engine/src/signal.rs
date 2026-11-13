//! The synthetic input every design gets: eight 10 s stereo signals, two
//! of each kind, which the 32 tracks read from at different offsets.
//!
//! - speech-like: a buzz at a wandering pitch through three moving
//!   resonances, in syllables of about a quarter second, phrases of 1.5 to
//!   3 s with pauses, over hiss at -50 dBFS and a 50 Hz hum;
//! - music: chords of four notes with harmonics, changing every half
//!   second, and a kick drum on every beat;
//! - effects: bursts of noise dying away at random moments, some up near
//!   full scale;
//! - ambience: steady rumbling noise at about -30 dBFS.

use crate::dsp::biquad::{Biquad, Coefs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Dialogue,
    Music,
    Effects,
    Ambience,
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    pub fn s(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
}

pub struct Signal {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

pub const RATE: f32 = 48_000.0;

pub fn hiss(len: usize, level: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.s() * level).collect()
}

/// The speech-like part alone (no hiss or hum), and where it speaks.
pub fn speech(len: usize, seed: u64) -> (Vec<f32>, Vec<bool>) {
    let mut rng = Rng::new(seed);
    let mut out = vec![0f32; len];
    let mut talking = vec![false; len];
    let mut formants = [Biquad::default(); 3];
    let mut at = (0.2 * RATE) as usize;
    let mut phase = 0f32;
    while at < len {
        let phrase = ((1.5 + 1.5 * rng.f()) * RATE) as usize;
        let end = (at + phrase).min(len);
        let mut syl_start = at;
        while syl_start < end {
            let syl = ((0.15 + 0.15 * rng.f()) * RATE) as usize;
            let f1 = 300.0 + 500.0 * rng.f();
            let f2 = 900.0 + 1500.0 * rng.f();
            let f3 = 2300.0 + 800.0 * rng.f();
            for (f, fr) in formants.iter_mut().zip([f1, f2, f3]) {
                f.c = Coefs::bandpass(RATE, fr, 6.0);
            }
            let pitch = 110.0 + 90.0 * rng.f();
            let fricative = rng.f() < 0.2;
            for i in syl_start..(syl_start + syl).min(end) {
                let t = (i - syl_start) as f32 / syl as f32;
                let env = (std::f32::consts::PI * t).sin().powf(0.7);
                phase += pitch / RATE;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                let x = if fricative {
                    rng.s() * 0.3
                } else {
                    if phase < 0.05 { 1.0 } else { -0.05 }
                };
                let mut y = 0.0;
                for f in &mut formants {
                    y += f.tick(0, x);
                }
                out[i] = y * env * 0.6;
                talking[i] = true;
            }
            syl_start += syl;
        }
        at = end + ((0.3 + 0.7 * rng.f()) * RATE) as usize;
    }
    (out, talking)
}

pub fn make(role: Role, len: usize, seed: u64) -> Signal {
    let mut rng = Rng::new(seed ^ 0xABCD);
    match role {
        Role::Dialogue => {
            let (s, _) = speech(len, seed);
            let h = hiss(len, 0.003, seed + 1);
            let h2 = hiss(len, 0.003, seed + 2);
            let hum = |i: usize| 0.01 * (2.0 * std::f32::consts::PI * 50.0 * i as f32 / RATE).sin();
            Signal {
                l: (0..len).map(|i| s[i] + h[i] + hum(i)).collect(),
                r: (0..len).map(|i| s[i] * 0.9 + h2[i] + hum(i)).collect(),
            }
        }
        Role::Music => {
            let (mut l, mut r) = (vec![0f32; len], vec![0f32; len]);
            let beat = (0.5 * RATE) as usize;
            for start in (0..len).step_by(beat) {
                let root = 110.0 * 2f32.powf((rng.next() % 12) as f32 / 12.0);
                let notes = [1.0, 1.26, 1.5, 2.0].map(|k| root * k);
                for i in start..(start + beat).min(len) {
                    let t = (i - start) as f32 / RATE;
                    let ti = i as f32 / RATE;
                    let mut x = 0.0;
                    for (n, f) in notes.iter().enumerate() {
                        for h in 1..4 {
                            x += (2.0 * std::f32::consts::PI * f * h as f32 * ti + n as f32).sin() * 0.05
                                / h as f32;
                        }
                    }
                    let kick = (2.0 * std::f32::consts::PI * (60.0 + 80.0 * (-t * 30.0).exp()) * t).sin()
                        * (-t * 8.0).exp()
                        * 0.5;
                    l[i] = x + kick;
                    r[i] = x * 0.95 + kick;
                }
            }
            Signal { l, r }
        }
        Role::Effects => {
            let (mut l, mut r) = (vec![0f32; len], vec![0f32; len]);
            let mut at = 0;
            while at < len {
                at += ((0.2 + 0.8 * rng.f()) * RATE) as usize;
                let loud = if rng.f() < 0.3 { 0.95 } else { 0.3 };
                let decay = 5.0 + 20.0 * rng.f();
                for i in at..(at + (0.6 * RATE) as usize).min(len) {
                    let t = (i - at) as f32 / RATE;
                    let e = loud * (-t * decay).exp();
                    l[i] += rng.s() * e;
                    r[i] += rng.s() * e;
                }
            }
            Signal { l, r }
        }
        Role::Ambience => {
            let mut lp = [
                Biquad::new(Coefs::highpass(RATE, 20.0, 0.7)),
                Biquad::new(Coefs::bandpass(RATE, 300.0, 0.4)),
            ];
            let mut l = hiss(len, 0.2, seed + 3);
            let mut r = hiss(len, 0.2, seed + 4);
            for f in &mut lp {
                f.process(0, &mut l);
                f.process(1, &mut r);
            }
            Signal { l, r }
        }
    }
}

/// Track `t` of 32: 8 of each kind, in that order.
pub fn role_of(t: usize) -> Role {
    [Role::Dialogue, Role::Music, Role::Effects, Role::Ambience][t / 8 % 4]
}

/// The eight signals, two of each kind (index = kind * 2 + variant).
pub fn all(len: usize) -> Vec<Signal> {
    [Role::Dialogue, Role::Music, Role::Effects, Role::Ambience]
        .iter()
        .flat_map(|&role| (0..2).map(move |v| make(role, len, 17 + v as u64 * 101 + role as u64 * 7)))
        .collect()
}
