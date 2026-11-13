//! A true-peak limiter for the master: keeps the sound's true peak (the
//! highest level between samples too, as `loudness` measures it) under a
//! ceiling, by turning it down just before each peak and back up gently
//! after.
//!
//! How it works, per stereo frame:
//!
//! 1. The true peak at the frame is found as the meter finds it: both
//!    channels four times oversampled ([`crate::loudness::TruePeak`]).
//! 2. The gain that peak needs (`ceiling / peak`, at most 1) is taken, and
//!    lets go slowly afterwards (the release, 80 ms).
//! 3. The lowest gain over the next stretch (the look-ahead, 1.5 ms plus
//!    the oversampler's own delay) is held, then averaged over the
//!    look-ahead: the gain glides down in time for each peak, never steps.
//! 4. The sound comes out delayed by [`Limiter::latency`] frames, times
//!    that gain.
//!
//! Holding the lowest gain a little longer than it is averaged over keeps
//! the gain fully down for a few frames around each peak, so the
//! oversampled signal between them is turned down too. Nothing is
//! allocated after [`Limiter::new`]; any block length gives the same sound.

use crate::loudness::{OVERSAMPLE, TAPS, TruePeak, interpolator};
use std::collections::VecDeque;

/// Frames either side of a peak that are fully turned down as well: the
/// oversampler's reach.
const AROUND: usize = TAPS / 2;
/// How far ahead the gain starts to come down, in seconds.
const LOOK_AHEAD: f64 = 0.0015;
/// How quickly the gain comes back up after a peak, in seconds (to about
/// two thirds of the way).
const RELEASE: f64 = 0.08;
/// Aimed a little under the ceiling, for what still slips between frames.
const MARGIN_DB: f32 = 0.1;

#[derive(Debug, Clone)]
pub struct Limiter {
    /// The ceiling aimed at, linear.
    ceiling: f32,
    phases: [[f32; TAPS]; OVERSAMPLE],
    detectors: [TruePeak; 2],
    /// Per frame, how far the gain comes back up towards 1.
    release: f32,
    /// The gain after the release, last frame.
    released: f32,
    /// The lowest gain over the last `hold` frames: (frame, gain), gains
    /// rising from front to back.
    lowest: VecDeque<(u64, f32)>,
    hold: usize,
    /// The held gains of the last `average.len()` frames, and their sum.
    average: Vec<f32>,
    sum: f64,
    /// The sound waiting to come out, interleaved, a ring of `latency` frames.
    delay: Vec<f32>,
    latency: usize,
    /// Frames taken in so far.
    frame: u64,
    /// The lowest gain applied so far.
    deepest: f32,
}

impl Limiter {
    /// For stereo at `rate`, keeping the true peak under `ceiling_db` (dBTP).
    pub fn new(rate: u32, ceiling_db: f32) -> Limiter {
        let rate = f64::from(rate.max(8_000));
        let ramp = ((rate * LOOK_AHEAD).round() as usize).max(4);
        let hold = ramp + 2 * AROUND;
        // A peak is found `AROUND` frames after the frame it is at (the
        // oversampler's delay); the gain is fully down `ramp - 1` frames
        // after it is found, for `2 * AROUND + 1` frames: centred there.
        let latency = AROUND + ramp - 1 + AROUND;
        Limiter {
            ceiling: 10f32.powf((ceiling_db - MARGIN_DB) / 20.0),
            phases: interpolator(),
            detectors: [TruePeak::new(), TruePeak::new()],
            release: (1.0 - (-1.0 / (rate * RELEASE)).exp()) as f32,
            released: 1.0,
            lowest: VecDeque::with_capacity(hold + 1),
            hold,
            average: vec![1.0; ramp],
            sum: ramp as f64,
            delay: vec![0.0; latency * 2],
            latency,
            frame: 0,
            deepest: 1.0,
        }
    }

    /// How many frames late the sound comes out.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// The deepest the sound was turned down so far, in dB (0 when never).
    pub fn deepest_db(&self) -> f32 {
        20.0 * self.deepest.log10()
    }

    /// Limits interleaved stereo in place: what comes out is what went in
    /// [`Limiter::latency`] frames earlier, turned down where it peaks.
    pub fn process(&mut self, samples: &mut [f32]) {
        for pair in samples.chunks_exact_mut(2) {
            let gain = self.gain_for(pair[0], pair[1]);
            let slot = (self.frame as usize % self.latency) * 2;
            let (left, right) = (self.delay[slot], self.delay[slot + 1]);
            self.delay[slot] = pair[0];
            self.delay[slot + 1] = pair[1];
            pair[0] = left * gain;
            pair[1] = right * gain;
            self.frame += 1;
        }
    }

    /// Takes in one frame and gives the gain for the frame coming out.
    fn gain_for(&mut self, left: f32, right: f32) -> f32 {
        let peak = self.detectors[0]
            .feed(left, &self.phases)
            .max(self.detectors[1].feed(right, &self.phases));
        let needed = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        self.released = needed.min(self.released + (1.0 - self.released) * self.release);
        // The lowest over the hold: drop what is no lower than the new
        // one, and what has fallen out of the window.
        while self.lowest.back().is_some_and(|&(_, g)| g >= self.released) {
            self.lowest.pop_back();
        }
        self.lowest.push_back((self.frame, self.released));
        while self
            .lowest
            .front()
            .is_some_and(|&(at, _)| at + self.hold as u64 <= self.frame)
        {
            self.lowest.pop_front();
        }
        let held = self.lowest.front().map_or(1.0, |&(_, g)| g);
        let slot = self.frame as usize % self.average.len();
        self.sum += f64::from(held) - f64::from(self.average[slot]);
        self.average[slot] = held;
        // Recounted now and then, so rounding never builds up.
        if slot == 0 {
            self.sum = self.average.iter().map(|&g| f64::from(g)).sum();
        }
        let gain = ((self.sum / self.average.len() as f64) as f32).min(1.0);
        self.deepest = self.deepest.min(gain);
        gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::Loudness;

    const RATE: u32 = 48_000;

    fn sine(freq: f64, amplitude: f64, phase: f64, seconds: f64) -> Vec<f32> {
        let n = (f64::from(RATE) * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (amplitude
                    * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(RATE) + phase).sin())
                    as f32;
                [v, v]
            })
            .collect()
    }

    /// Through a limiter at -1 dBTP in blocks of `block` frames, with the
    /// delay taken out: the sound as it comes out, and the limiter.
    fn limited(input: &[f32], block: usize) -> (Vec<f32>, Limiter) {
        let mut limiter = Limiter::new(RATE, -1.0);
        let latency = limiter.latency();
        let mut out = input.to_vec();
        out.extend(std::iter::repeat_n(0.0, latency * 2));
        for chunk in out.chunks_mut(block * 2) {
            limiter.process(chunk);
        }
        (out[latency * 2..].to_vec(), limiter)
    }

    fn true_peak(samples: &[f32]) -> f32 {
        let mut meter = Loudness::new(RATE);
        meter.feed(samples);
        meter.measured().true_peak_max
    }

    #[test]
    fn quiet_sound_comes_out_as_it_went_in() {
        let input = sine(997.0, 0.5, 0.0, 0.5);
        let (out, limiter) = limited(&input, 512);
        assert_eq!(out, input);
        assert_eq!(limiter.deepest_db(), 0.0);
    }

    #[test]
    fn loud_sound_is_kept_under_the_ceiling() {
        // Twice full scale, a tone at a quarter of the rate whose samples
        // all miss its peaks (the true peak is 3 dB over them), a click, and
        // noise far over the top.
        let mut noise_state = 12345u32;
        let mut noise = || {
            noise_state = noise_state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (noise_state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let mut click = vec![0.0f32; 9600];
        click[4800] = 1.5;
        click[4801] = -1.5;
        let loud: Vec<f32> = (0..9600).map(|_| noise() * 3.0).collect();
        for input in [
            sine(997.0, 2.0, 0.0, 0.5),
            sine(12_000.0, 1.0, std::f64::consts::FRAC_PI_4, 0.5),
            click,
            loud,
        ] {
            let before = true_peak(&input);
            let (out, limiter) = limited(&input, 333);
            let after = true_peak(&out);
            assert!(after <= -1.0, "true peak {before:.2} became {after:.2} dBTP");
            assert!(limiter.deepest_db() < 0.0);
        }
    }

    #[test]
    fn a_steady_loud_tone_settles_just_under_the_ceiling() {
        let input = sine(997.0, 2.0, 0.0, 1.0);
        let (out, _) = limited(&input, 1024);
        // The last half second: steady at the ceiling, give or take the margin.
        let peak = out[RATE as usize..].iter().fold(0f32, |m, s| m.max(s.abs()));
        let db = 20.0 * peak.log10();
        assert!((-1.3..=-1.0).contains(&db), "{db} dB");
    }

    #[test]
    fn the_gain_comes_back_after_a_peak() {
        let mut input = sine(997.0, 2.0, 0.0, 0.05);
        let quiet = sine(997.0, 0.25, 0.0, 1.0);
        input.extend(&quiet);
        let (out, _) = limited(&input, 256);
        // Half a second after the loud part, the quiet tone is as it was.
        let start = (RATE as usize / 20 + RATE as usize / 2) * 2;
        let peak = out[start..].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.25).abs() < 0.0025, "{peak}");
    }

    #[test]
    fn any_block_length_gives_the_same_sound() {
        let input = sine(440.0, 1.8, 0.3, 0.3);
        let (one, _) = limited(&input, 1);
        let (many, _) = limited(&input, 4096);
        assert_eq!(one, many);
    }
}
