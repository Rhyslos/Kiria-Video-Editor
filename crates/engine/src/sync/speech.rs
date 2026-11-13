//! Made-up sound for the tests: something like speech (voiced syllables
//! with a pitch and vowel colours, short hiss for consonants, pauses), and
//! the same moment "recorded" by two different devices.

/// A small random number maker that gives the same numbers every run.
#[derive(Debug, Clone)]
pub struct Dice(u64);

impl Dice {
    pub fn new(seed: u64) -> Dice {
        Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// 0 to 1.
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }

    /// Noise with even power across frequencies, -1 to 1.
    pub fn white(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

/// Vowel colours: the first three resonances (Hz).
const VOWELS: [[f32; 3]; 5] = [
    [730.0, 1090.0, 2440.0],
    [270.0, 2290.0, 3010.0],
    [530.0, 1840.0, 2480.0],
    [570.0, 840.0, 2410.0],
    [300.0, 870.0, 2240.0],
];

/// `seconds` of speech-like sound at `rate`, peaking near 0.5.
pub fn speech(seed: u64, seconds: f32, rate: u32) -> Vec<f32> {
    let mut dice = Dice::new(seed);
    let total = (seconds * rate as f32) as usize;
    let mut out = vec![0.0f32; total];
    let r = rate as f32;
    let mut at = (dice.between(0.05, 0.3) * r) as usize;
    let mut phase = 0.0f32;
    while at < total {
        // A phrase of a few syllables, then a breath.
        let syllables = 2 + (dice.next() * 6.0) as usize;
        for _ in 0..syllables {
            if dice.next() < 0.4 {
                // A consonant: a short hiss.
                let len = (dice.between(0.02, 0.07) * r) as usize;
                let level = dice.between(0.05, 0.15);
                let mut last = 0.0;
                for i in 0..len.min(total.saturating_sub(at)) {
                    let w = dice.white();
                    let hiss = w - last;
                    last = w;
                    let shape = (std::f32::consts::PI * i as f32 / len as f32).sin();
                    out[at + i] += level * hiss * shape;
                }
                at += len;
            }
            let len = (dice.between(0.08, 0.26) * r) as usize;
            let pitch = dice.between(95.0, 220.0);
            let glide = dice.between(-0.25, 0.25);
            let vowel = VOWELS[(dice.next() * VOWELS.len() as f32) as usize % VOWELS.len()];
            let level = dice.between(0.15, 0.45);
            for i in 0..len.min(total.saturating_sub(at)) {
                let t = i as f32 / len as f32;
                let f0 = pitch * (1.0 + glide * t);
                phase += f0 / r;
                let mut v = 0.0;
                let mut h = 1.0;
                while h * f0 < 3800.0 && h < 30.0 {
                    let f = h * f0;
                    let colour: f32 = vowel
                        .iter()
                        .map(|formant| 1.0 / (1.0 + ((f - formant) / 90.0).powi(2)))
                        .sum();
                    v += colour / h.sqrt() * (std::f32::consts::TAU * h * phase).sin();
                    h += 1.0;
                }
                let attack = (t / 0.12).min(1.0);
                let release = ((1.0 - t) / 0.3).min(1.0);
                out[at + i] += level * 0.25 * v * attack * release;
            }
            at += len + (dice.between(0.01, 0.12) * r) as usize;
        }
        at += (dice.between(0.25, 1.2) * r) as usize;
    }
    out
}

/// A sound like a game's: rumble, a steady chord and loud bangs now and
/// then, none of it like speech.
pub fn game(seed: u64, seconds: f32, rate: u32) -> Vec<f32> {
    let mut dice = Dice::new(seed ^ 0xA5A5);
    let total = (seconds * rate as f32) as usize;
    let r = rate as f32;
    let mut rumble = 0.0f32;
    let mut bang = 0.0f32;
    (0..total)
        .map(|i| {
            let t = i as f32 / r;
            rumble = 0.995 * rumble + 0.02 * dice.white();
            if dice.next() < 0.8 / r {
                bang = dice.between(0.2, 0.5);
            }
            bang *= 0.9995;
            let chord = [110.0f32, 138.6, 164.8]
                .iter()
                .map(|f| (std::f32::consts::TAU * f * t).sin())
                .sum::<f32>();
            rumble + 0.03 * chord + bang * dice.white()
        })
        .collect()
}

/// How a device colours and spoils what it records.
#[derive(Debug, Clone, Copy)]
pub struct Device {
    /// How loud the shared sound comes out.
    pub gain: f32,
    /// Bass let through: a one-pole high-pass at this frequency (Hz).
    pub low_cut: f32,
    /// Treble let through: a one-pole low-pass at this frequency (Hz).
    pub high_cut: f32,
    /// Hiss of its own, as a level.
    pub hiss: f32,
    /// Plays slightly fast or slow (parts per million): its clock.
    pub drift_ppm: f32,
}

/// `shared` as `device` records it, with `own` (its own other sound) mixed
/// in; starting `delay` samples into `shared` (negative: silence first).
pub fn record(
    shared: &[f32],
    own: &[f32],
    device: Device,
    delay: i64,
    length: usize,
    rate: u32,
    seed: u64,
) -> Vec<f32> {
    let mut dice = Dice::new(seed);
    let r = rate as f32;
    let hp = (-std::f32::consts::TAU * device.low_cut / r).exp();
    let lp = 1.0 - (-std::f32::consts::TAU * device.high_cut / r).exp();
    let (mut hp_in, mut hp_out, mut lp_out) = (0.0f32, 0.0f32, 0.0f32);
    let step = 1.0 + f64::from(device.drift_ppm) * 1e-6;
    (0..length)
        .map(|i| {
            let place = delay as f64 + i as f64 * step;
            let k = place.floor();
            let frac = (place - k) as f32;
            let get = |j: f64| -> f32 {
                if j < 0.0 {
                    0.0
                } else {
                    shared.get(j as usize).copied().unwrap_or(0.0)
                }
            };
            let x = get(k) * (1.0 - frac) + get(k + 1.0) * frac;
            hp_out = hp * (hp_out + x - hp_in);
            hp_in = x;
            lp_out += lp * (hp_out - lp_out);
            device.gain * lp_out + own.get(i).copied().unwrap_or(0.0) + device.hiss * dice.white()
        })
        .collect()
}
