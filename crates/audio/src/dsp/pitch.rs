//! Pitch shift: the sound higher or lower without changing its speed.
//!
//! Two read heads move through a short delay line at the new speed, each
//! faded in and out on a Hann window half a window apart, so one is always
//! sounding while the other jumps back. Where a head comes in again is
//! chosen to line up with what the other is playing (the place within a
//! few milliseconds whose sound matches best), so the two add up instead
//! of cancelling and a tone keeps its new pitch rather than splitting.
//! The window is 40 ms; the sound comes out about half of it late, which
//! is part of the effect (the track's fixed delay is the clean-up's).
//!
//! Its line is made with the effect, off the real-time thread, so changing
//! a setting never allocates.

use std::f32::consts::PI;

/// The pitch shift's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchSettings {
    pub on: bool,
    /// How far: -12 to 12 semitones (an octave down to an octave up).
    pub semitones: f32,
    /// How much of the shifted sound is heard: 0 to 1 (the rest is the
    /// sound as it came).
    pub mix: f32,
}

impl PitchSettings {
    pub const OFF: PitchSettings = PitchSettings {
        on: false,
        semitones: 0.0,
        mix: 1.0,
    };

    /// How much faster the heads read than the sound is written. Two
    /// octaves either way at most: a track's shift is kept to one by its
    /// settings, a clip's kept pitch goes as far as its speed (25 % to 400 %).
    pub fn ratio(&self) -> f32 {
        2f32.powf(self.semitones.clamp(-24.0, 24.0) / 12.0)
    }
}

/// The window each head is faded over, in seconds.
pub const WINDOW: f32 = 0.04;

/// How far behind the sound it is given the shifted sound comes out, on
/// average (samples at `rate`): where its heads read from, half way through
/// their window, past the room kept for lining up.
pub fn latency(rate: f32) -> usize {
    let window = (rate * WINDOW).max(16.0);
    (1.0 + (window / 4.0).floor() + window / 2.0).round() as usize
}

/// A read head: where it is in its window (0 to 1) and how far behind the
/// newest sample its window starts.
#[derive(Debug, Clone, Copy)]
struct Head {
    phase: f32,
    base: f32,
}

#[derive(Debug)]
pub struct Pitch {
    settings: PitchSettings,
    lines: [Vec<f32>; 2],
    at: usize,
    /// The window in samples.
    window: f32,
    /// How far either way a head may come in from its usual place, and how
    /// much sound is compared to choose: samples.
    search: usize,
    compare: usize,
    heads: [Head; 2],
}

impl Pitch {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: f32, settings: PitchSettings) -> Pitch {
        let window = (rate * WINDOW).max(16.0);
        let search = (window / 4.0) as usize;
        let compare = (window / 4.0) as usize;
        // Room for a window, the search both ways, the sound compared and a
        // little.
        let len = window as usize + 2 * search + compare + 8;
        Pitch {
            settings,
            lines: [vec![0.0; len], vec![0.0; len]],
            at: 0,
            window,
            search,
            compare,
            heads: Pitch::fresh(search),
        }
    }

    fn fresh(search: usize) -> [Head; 2] {
        let base = 1.0 + search as f32;
        [Head { phase: 0.0, base }, Head { phase: 0.5, base }]
    }

    /// Changes its settings. Never allocates; turned on again, it starts
    /// from silence rather than old sound.
    pub fn set(&mut self, settings: PitchSettings) {
        if settings.on && !self.settings.on {
            for line in &mut self.lines {
                line.fill(0.0);
            }
            self.heads = Pitch::fresh(self.search);
        }
        self.settings = settings;
    }

    /// Both channels at `back` samples behind the newest, between samples.
    fn read(&self, back: f32) -> [f32; 2] {
        let len = self.lines[0].len();
        let pos = (self.at as f32 - back).rem_euclid(len as f32);
        let i = pos.floor() as usize % len;
        let next = (i + 1) % len;
        let f = pos - pos.floor();
        [0, 1].map(|c| {
            let line = &self.lines[c];
            line[i] + (line[next] - line[i]) * f
        })
    }

    /// The mono sound `back + i` samples behind the newest.
    fn mono(&self, back: usize) -> f32 {
        let len = self.lines[0].len();
        let i = (self.at + len * 2 - back % len) % len;
        self.lines[0][i] + self.lines[1][i]
    }

    /// Where head `k` comes in again: its usual place moved by the shift
    /// (within the search) whose sound best matches what the other head
    /// plays now, compared a few samples apart and then to the sample.
    fn come_in(&mut self, k: usize) {
        let other = self.heads[1 - k];
        let now_back = (other.base + other.phase * self.window).round() as usize;
        let usual = 1 + self.search;
        let start_phase = self.heads[k].phase;
        let at_phase = (start_phase * self.window).round() as usize;
        let score = |shift: usize| {
            let back = usual + shift - self.search + at_phase;
            (0..self.compare)
                .step_by(4)
                .map(|i| self.mono(now_back + i) * self.mono(back + i))
                .sum::<f32>()
        };
        let best = (0..=2 * self.search)
            .step_by(2)
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap_or(self.search);
        let fine = (best.saturating_sub(1)..=(best + 1).min(2 * self.search))
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap_or(best);
        self.heads[k].base = (usual + fine - self.search) as f32;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = self.settings;
        if !s.on {
            return;
        }
        let len = self.lines[0].len();
        // The heads fall behind (lower) or catch up (higher) by this much
        // a sample, as a share of the window.
        let step = (1.0 - s.ratio()) / self.window;
        let mix = s.mix.clamp(0.0, 1.0);
        for (dl, dr) in l.iter_mut().zip(r.iter_mut()) {
            self.lines[0][self.at] = *dl;
            self.lines[1][self.at] = *dr;
            let mut wet = [0.0f32; 2];
            for head in self.heads {
                // A Hann window: the two heads' weights add up to one.
                let weight = (PI * head.phase).sin().powi(2);
                let sound = self.read(head.base + head.phase * self.window);
                wet[0] += weight * sound[0];
                wet[1] += weight * sound[1];
            }
            self.at = (self.at + 1) % len;
            for k in 0..2 {
                let phase = self.heads[k].phase + step;
                self.heads[k].phase = phase.rem_euclid(1.0);
                // Wrapped round: silent now, it comes in afresh.
                if !(0.0..1.0).contains(&phase) {
                    self.come_in(k);
                }
            }
            *dl += (wet[0] - *dl) * mix;
            *dr += (wet[1] - *dr) * mix;
        }
    }

    /// Carries the sound under way over from the one this replaces.
    pub fn take_state(&mut self, old: &Pitch) {
        if self.lines[0].len() == old.lines[0].len() {
            for (new, was) in self.lines.iter_mut().zip(&old.lines) {
                new.copy_from_slice(was);
            }
            self.at = old.at;
            self.heads = old.heads;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (2.0 * PI * hz * i as f32 / RATE).sin() * 0.5)
            .collect()
    }

    /// Where its sound is centred in frequency near `about` (a quarter
    /// either side), over the last half: a shifter's grains split a tone
    /// into lines a grain's rate apart around its new pitch, so their
    /// middle is what is heard.
    fn pitch_of(samples: &[f32], about: f32) -> f32 {
        let half = &samples[samples.len() / 2..];
        let power = |hz: f32| {
            let w = 2.0 * PI * hz / RATE;
            let (mut re, mut im) = (0f64, 0f64);
            for (i, s) in half.iter().enumerate() {
                let (sin, cos) = (w * i as f32).sin_cos();
                re += f64::from(s * cos);
                im += f64::from(s * sin);
            }
            re * re + im * im
        };
        let (mut sum, mut weight) = (0f64, 0f64);
        for hz in (about * 0.75) as u32..(about * 1.25) as u32 {
            let p = power(hz as f32);
            sum += p * f64::from(hz);
            weight += p;
        }
        (sum / weight.max(1e-12)) as f32
    }

    fn shifted(semitones: f32, hz: f32) -> Vec<f32> {
        let mut pitch = Pitch::new(
            RATE,
            PitchSettings {
                on: true,
                semitones,
                mix: 1.0,
            },
        );
        let mut l = tone(hz, RATE as usize);
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            pitch.process(a, b);
        }
        l
    }

    #[test]
    fn an_octave_up_doubles_the_pitch_and_down_halves_it() {
        let up = pitch_of(&shifted(12.0, 220.0), 440.0);
        assert!((up - 440.0).abs() < 2.0, "{up}");
        let down = pitch_of(&shifted(-12.0, 440.0), 220.0);
        assert!((down - 220.0).abs() < 2.0, "{down}");
        let fifth = pitch_of(&shifted(7.0, 200.0), 300.0);
        assert!((fifth - 299.7).abs() < 2.0, "{fifth}");
    }

    #[test]
    fn no_shift_keeps_the_pitch_and_the_level() {
        let same = shifted(0.0, 300.0);
        assert!((pitch_of(&same, 300.0) - 300.0).abs() < 2.0);
        let peak = same[24_000..].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.02, "{peak}");
    }

    #[test]
    fn off_or_dry_leaves_the_sound_alone() {
        let mut pitch = Pitch::new(RATE, PitchSettings::OFF);
        let mut l = tone(440.0, 1024);
        let mut r = l.clone();
        let before = l.clone();
        pitch.process(&mut l, &mut r);
        assert_eq!(l, before);
        pitch.set(PitchSettings {
            on: true,
            semitones: 5.0,
            mix: 0.0,
        });
        pitch.process(&mut l, &mut r);
        assert_eq!(l, before);
    }

    #[test]
    fn its_level_holds_while_shifted() {
        let up = shifted(5.0, 330.0);
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let level = rms(&up[24_000..]);
        // A tone of 0.5 is 0.354 RMS; two heads mixing can dip a little.
        assert!(level > 0.25 && level < 0.45, "{level}");
    }
}
