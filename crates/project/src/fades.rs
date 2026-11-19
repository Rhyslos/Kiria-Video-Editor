//! Fades on a clip's picture (format 32), and the shape of every fade.
//!
//! A picture fades in from see-through at the clip's start and out to
//! see-through at its end, over exact [`Time`] lengths on the timeline,
//! like a sound's fades ([`crate::Sound`]). Each fade has a curve, in
//! millionths from -[`CURVE_ONE`] to [`CURVE_ONE`]: 0 is the plain fade
//! (a straight line for a picture, a quarter sine for sound), above it the
//! fade comes up sooner and below it later. The engine turns a fade into
//! a factor with [`level`], the same for preview and export; the timeline
//! draws it with the same formula.

use crate::time::Time;
use diag::AppError;
use serde::{Deserialize, Serialize};

/// A curve all the way up: the fade comes up as soon as it can.
pub const CURVE_ONE: i32 = 1_000_000;

/// What a fade's curve bends: a straight line (pictures) or a quarter sine
/// (sound, so two crossing fades keep their power).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Straight,
    Sine,
}

/// How far up a fade is `share` of the way along it (0 at its silent or
/// see-through end, 1 where the clip is whole), with `curve` in millionths.
/// The plain fade is raised to the power 4^(-curve): a curve of one comes
/// up as the fourth root of the plain fade, minus one as its fourth power.
pub fn level(share: f64, curve: i32, base: Base) -> f64 {
    let share = share.clamp(0.0, 1.0);
    let plain = match base {
        Base::Straight => share,
        Base::Sine => (share * std::f64::consts::FRAC_PI_2).sin(),
    };
    let bend = f64::from(curve.clamp(-CURVE_ONE, CURVE_ONE)) / f64::from(CURVE_ONE);
    plain.powf(4f64.powf(-bend))
}

/// How far up both fades of a clip `length` long are `from_start` into it
/// (seconds on the timeline): the fade in's level times the fade out's.
pub fn both(from_start: f64, length: f64, fade_in: (f64, i32), fade_out: (f64, i32), base: Base) -> f64 {
    let mut factor = 1.0;
    let (in_length, in_curve) = fade_in;
    if in_length > 0.0 && from_start < in_length {
        factor *= level(from_start / in_length, in_curve, base);
    }
    let (out_length, out_curve) = fade_out;
    let left = length - from_start;
    if out_length > 0.0 && left < out_length {
        factor *= level(left / out_length, out_curve, base);
    }
    factor
}

/// A picture's fades. The default has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct PictureFades {
    /// How long it takes to come in from see-through at the clip's start.
    pub fade_in: Time,
    /// How long it takes to go to see-through at the clip's end.
    pub fade_out: Time,
    /// The fade in's curve, in millionths (0 straight).
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub fade_in_curve: i32,
    /// The fade out's curve, in millionths (0 straight).
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub fade_out_curve: i32,
}

impl Default for PictureFades {
    fn default() -> PictureFades {
        PictureFades::NONE
    }
}

impl PictureFades {
    /// No fades.
    pub const NONE: PictureFades = PictureFades {
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
        fade_in_curve: 0,
        fade_out_curve: 0,
    };

    /// Whether it fades in or out.
    pub fn is_faded(&self) -> bool {
        self.fade_in > Time::ZERO || self.fade_out > Time::ZERO
    }

    /// How much of the picture shows `from_start` into a clip `length`
    /// long (seconds on the timeline): 1 outside the fades.
    pub fn level(&self, from_start: f64, length: f64) -> f64 {
        if !self.is_faded() {
            return 1.0;
        }
        both(
            from_start,
            length,
            (self.fade_in.as_secs_f64(), self.fade_in_curve),
            (self.fade_out.as_secs_f64(), self.fade_out_curve),
            Base::Straight,
        )
    }

    /// Whether both fades fit in a clip `length` long.
    pub fn fits(&self, length: Time) -> bool {
        !self.fade_in.is_negative() && !self.fade_out.is_negative() && self.fade_in + self.fade_out <= length
    }

    /// The same fades on a clip `length` long: the fade in at most the
    /// whole clip and the fade out at most what that leaves (as a sound's,
    /// [`crate::Sound::fitted`]); curves brought inside their range.
    pub fn fitted(self, length: Time) -> PictureFades {
        let length = length.max(Time::ZERO);
        let fade_in = self.fade_in.clamp(Time::ZERO, length);
        PictureFades {
            fade_in,
            fade_out: self.fade_out.clamp(Time::ZERO, length - fade_in),
            fade_in_curve: self.fade_in_curve.clamp(-CURVE_ONE, CURVE_ONE),
            fade_out_curve: self.fade_out_curve.clamp(-CURVE_ONE, CURVE_ONE),
        }
    }

    /// Without its fade in: the right half of a cut.
    pub fn without_fade_in(self) -> PictureFades {
        PictureFades {
            fade_in: Time::ZERO,
            fade_in_curve: 0,
            ..self
        }
    }

    /// Without its fade out: the left half of a cut.
    pub fn without_fade_out(self) -> PictureFades {
        PictureFades {
            fade_out: Time::ZERO,
            fade_out_curve: 0,
            ..self
        }
    }

    /// Refuses, explained, fades that can't be used on a clip `length` long.
    pub fn check(&self, length: Time) -> diag::Result<()> {
        if !self.fits(length) || *self != self.fitted(length) {
            return Err(AppError::new("Those fades can't be used")
                .why("A clip's fade in and fade out together can't be longer than the clip, and a curve goes from -1 to 1.")
                .fix("Make one of the fades shorter, or the clip longer."));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_fade_is_straight_for_a_picture_and_a_quarter_sine_for_sound() {
        assert_eq!(level(0.0, 0, Base::Straight), 0.0);
        assert_eq!(level(1.0, 0, Base::Straight), 1.0);
        assert!((level(0.25, 0, Base::Straight) - 0.25).abs() < 1e-12);
        assert!((level(0.5, 0, Base::Sine) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        // Out of range: the ends, and curves held to ±1.
        assert_eq!(level(2.0, 0, Base::Sine), 1.0);
        assert_eq!(
            level(0.5, 5 * CURVE_ONE, Base::Straight),
            level(0.5, CURVE_ONE, Base::Straight)
        );
    }

    #[test]
    fn a_curve_up_comes_in_sooner_and_a_curve_down_later() {
        let plain = level(0.5, 0, Base::Straight);
        let up = level(0.5, CURVE_ONE, Base::Straight);
        let down = level(0.5, -CURVE_ONE, Base::Straight);
        assert!((up - 0.5f64.powf(0.25)).abs() < 1e-12, "{up}");
        assert!((down - 0.0625).abs() < 1e-12, "{down}");
        assert!(down < plain && plain < up);
        // Both ends stay where they are whatever the curve.
        for curve in [-CURVE_ONE, -300_000, 0, 700_000, CURVE_ONE] {
            assert_eq!(level(0.0, curve, Base::Sine), 0.0);
            assert_eq!(level(1.0, curve, Base::Sine), 1.0);
        }
    }

    #[test]
    fn a_picture_fades_in_from_its_start_and_out_to_its_end() {
        let fades = PictureFades {
            fade_in: Time::seconds(1),
            fade_out: Time::seconds(2),
            ..PictureFades::NONE
        };
        assert_eq!(fades.level(0.0, 10.0), 0.0);
        assert!((fades.level(0.5, 10.0) - 0.5).abs() < 1e-12);
        assert_eq!(fades.level(5.0, 10.0), 1.0);
        assert!((fades.level(9.0, 10.0) - 0.5).abs() < 1e-12);
        assert_eq!(fades.level(10.0, 10.0), 0.0);
        assert_eq!(PictureFades::NONE.level(0.0, 10.0), 1.0);
    }

    #[test]
    fn fades_fit_their_clip_and_a_cut_keeps_each_on_its_own_side() {
        let fades = PictureFades {
            fade_in: Time::seconds(3),
            fade_out: Time::seconds(4),
            fade_in_curve: 200_000,
            fade_out_curve: -2 * CURVE_ONE,
        };
        assert!(fades.fits(Time::seconds(7)) && !fades.fits(Time::seconds(6)));
        let fitted = fades.fitted(Time::seconds(5));
        assert_eq!(
            (fitted.fade_in, fitted.fade_out),
            (Time::seconds(3), Time::seconds(2))
        );
        assert_eq!(fitted.fade_out_curve, -CURVE_ONE);
        assert!(fades.check(Time::seconds(9)).is_err(), "a curve out of range");
        assert!(fitted.check(Time::seconds(5)).is_ok());
        assert_eq!(fades.without_fade_in().fade_in_curve, 0);
        assert_eq!(fades.without_fade_out().fade_out, Time::ZERO);
    }
}
