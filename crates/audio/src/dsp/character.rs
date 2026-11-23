//! Distortion: a sound pushed into a soft clip (Drive), or squeezed
//! through the narrow band of a small speaker first (Radio, Phone).
//!
//! The clip is `tanh`, so it saturates smoothly; the gain into it is the
//! drive, and the level out is brought back down by as much as the clip
//! lifted a quiet sound, so turning the drive up changes the character
//! more than the loudness. Mix blends it with the sound as it came.

use super::biquad::{Biquad, Coefs};
use super::from_db;

/// Which kind of distortion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The whole sound, soft-clipped.
    Drive,
    /// Kept to 500 Hz to 4 kHz, then clipped: a small radio.
    Radio,
    /// Kept to 400 Hz to 3 kHz, steeper, mono, clipped harder: a phone
    /// line.
    Phone,
}

impl Style {
    /// The band it keeps, if any: low and high edge in hertz.
    fn band(self) -> Option<(f32, f32)> {
        match self {
            Style::Drive => None,
            Style::Radio => Some((500.0, 4_000.0)),
            Style::Phone => Some((400.0, 3_000.0)),
        }
    }
}

/// Its settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistortSettings {
    pub on: bool,
    pub style: Style,
    /// Gain into the clip: 0 to 30 dB.
    pub drive_db: f32,
    /// 0 (the sound as it came) to 1 (only the distorted sound).
    pub mix: f32,
}

impl DistortSettings {
    pub const OFF: DistortSettings = DistortSettings {
        on: false,
        style: Style::Drive,
        drive_db: 12.0,
        mix: 1.0,
    };
}

/// The effect. Its filters (two high passes and two low passes, each 12 dB
/// an octave, so 24 together) are set for the style.
#[derive(Debug)]
pub struct Distort {
    settings: DistortSettings,
    rate: f32,
    low_cut: [Biquad; 2],
    high_cut: [Biquad; 2],
    dry: Vec<f32>,
}

/// The longest block it is handed.
const MOST: usize = crate::BLOCK;

impl Distort {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: f32, settings: DistortSettings) -> Distort {
        let mut d = Distort {
            settings,
            rate,
            low_cut: [Biquad::default(); 2],
            high_cut: [Biquad::default(); 2],
            dry: vec![0.0; 2 * MOST],
        };
        d.set(settings);
        d
    }

    /// Changes its settings. Never allocates.
    pub fn set(&mut self, settings: DistortSettings) {
        self.settings = settings;
        let (low, high) = match settings.style.band() {
            Some((low, high)) => (
                Coefs::high_pass(self.rate, low, 0.707),
                Coefs::low_pass(self.rate, high, 0.707),
            ),
            None => (Coefs::THROUGH, Coefs::THROUGH),
        };
        for f in &mut self.low_cut {
            f.coefs = low;
        }
        for f in &mut self.high_cut {
            f.coefs = high;
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = self.settings;
        if !s.on {
            return;
        }
        let n = l.len().min(r.len()).min(MOST);
        let (dry_l, dry_r) = self.dry.split_at_mut(MOST);
        dry_l[..n].copy_from_slice(&l[..n]);
        dry_r[..n].copy_from_slice(&r[..n]);
        if s.style == Style::Phone {
            for i in 0..n {
                let m = 0.5 * (l[i] + r[i]);
                l[i] = m;
                r[i] = m;
            }
        }
        for f in self.low_cut.iter_mut().chain(self.high_cut.iter_mut()) {
            f.process(0, &mut l[..n]);
            f.process(1, &mut r[..n]);
        }
        let hard = if s.style == Style::Phone { 6.0 } else { 0.0 };
        let gain = from_db(s.drive_db.clamp(0.0, 30.0) + hard);
        // A quiet sound comes out as loud as it went in.
        let back = 1.0 / gain;
        let mix = s.mix.clamp(0.0, 1.0);
        for (out, dry) in [(&mut l[..n], &dry_l[..n]), (&mut r[..n], &dry_r[..n])] {
            for (o, d) in out.iter_mut().zip(dry) {
                let wet = (*o * gain).tanh() * back;
                *o = *d + (wet - *d) * mix;
            }
        }
    }

    /// Carries the filters' memories over from the one this replaces.
    pub fn take_state(&mut self, old: &Distort) {
        for (new, was) in self.low_cut.iter_mut().zip(&old.low_cut) {
            new.take_state(was);
        }
        for (new, was) in self.high_cut.iter_mut().zip(&old.high_cut) {
            new.take_state(was);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::biquad::tests::{RATE, measured_db};

    fn on(style: Style, drive_db: f32) -> DistortSettings {
        DistortSettings {
            on: true,
            style,
            drive_db,
            mix: 1.0,
        }
    }

    fn run(d: &mut Distort, block: &mut [f32]) {
        for chunk in block.chunks_mut(MOST) {
            let mut right = chunk.to_vec();
            d.process(chunk, &mut right);
        }
    }

    #[test]
    fn off_leaves_the_sound_alone() {
        let mut d = Distort::new(RATE, DistortSettings::OFF);
        let mut l = [0.5, -0.25, 0.9];
        let mut r = l;
        d.process(&mut l, &mut r);
        assert_eq!(l, [0.5, -0.25, 0.9]);
    }

    #[test]
    fn drive_rounds_loud_peaks_and_keeps_quiet_sound() {
        let mut d = Distort::new(RATE, on(Style::Drive, 24.0));
        let mut l = [0.001, 1.0];
        let mut r = l;
        d.process(&mut l, &mut r);
        assert!((l[0] - 0.001).abs() < 0.0001, "quiet as it came: {}", l[0]);
        assert!(l[1] < 0.1, "a loud peak flattened: {}", l[1]);
    }

    #[test]
    fn radio_and_phone_keep_to_a_small_speakers_band() {
        let quiet = |style| {
            let mut d = Distort::new(RATE, on(style, 0.0));
            move |block: &mut [f32]| {
                // Quiet, so the clip does nothing: only the filters show.
                for s in block.iter_mut() {
                    *s *= 0.01;
                }
                run(&mut d, block);
                for s in block.iter_mut() {
                    *s *= 100.0;
                }
            }
        };
        assert!(measured_db(quiet(Style::Radio), 1_500.0) > -2.0);
        assert!(measured_db(quiet(Style::Radio), 100.0) < -20.0);
        assert!(measured_db(quiet(Style::Phone), 8_000.0) < -20.0);
        assert!(
            measured_db(quiet(Style::Drive), 100.0) > -0.5,
            "drive keeps the whole sound"
        );
    }
}
