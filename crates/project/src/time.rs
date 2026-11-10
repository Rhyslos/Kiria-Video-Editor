//! Exact time.
//!
//! Every time and length in a project is a fraction of seconds, never a float,
//! so frame positions never drift: 30000/1001 fps footage stays frame-accurate
//! after an hour of edits. Frame rates are fractions too.

use diag::AppError;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Neg, Sub};

/// Frames per second as an exact fraction, for example 60/1 or 30000/1001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "(u32, u32)", into = "(u32, u32)")]
pub struct Rate {
    num: u32,
    den: u32,
}

impl Rate {
    pub const FPS_24: Rate = Rate::whole(24);
    pub const FPS_25: Rate = Rate::whole(25);
    pub const FPS_30: Rate = Rate::whole(30);
    pub const FPS_50: Rate = Rate::whole(50);
    pub const FPS_60: Rate = Rate::whole(60);
    pub const FPS_120: Rate = Rate::whole(120);
    pub const FPS_144: Rate = Rate::whole(144);
    /// 23.976 fps.
    pub const NTSC_24: Rate = Rate {
        num: 24000,
        den: 1001,
    };
    /// 29.97 fps.
    pub const NTSC_30: Rate = Rate {
        num: 30000,
        den: 1001,
    };
    /// 59.94 fps.
    pub const NTSC_60: Rate = Rate {
        num: 60000,
        den: 1001,
    };

    /// A whole number of frames per second. Zero becomes 1.
    pub const fn whole(fps: u32) -> Rate {
        Rate {
            num: if fps == 0 { 1 } else { fps },
            den: 1,
        }
    }

    /// `num / den` frames per second, reduced.
    pub fn new(num: u32, den: u32) -> diag::Result<Rate> {
        if num == 0 || den == 0 {
            return Err(AppError::new("This frame rate is not valid")
                .why(format!("{num}/{den} frames per second is zero or undefined."))
                .fix("Pick a frame rate from the list, such as 30 or 60."));
        }
        let g = gcd(num as u128, den as u128) as u32;
        Ok(Rate {
            num: num / g,
            den: den / g,
        })
    }

    pub fn num(self) -> u32 {
        self.num
    }

    pub fn den(self) -> u32 {
        self.den
    }

    /// The nominal whole number of frames per second (60 for 59.94), as used in timecode.
    pub fn nominal_fps(self) -> u32 {
        self.num.div_ceil(self.den)
    }

    /// Approximate frames per second, for display only.
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// How long one frame lasts.
    pub fn frame_duration(self) -> Time {
        self.frame_start(1)
    }

    /// The time where frame number `frame` starts (frame 0 starts at zero).
    pub fn frame_start(self, frame: i64) -> Time {
        Time::from_i128(i128::from(frame) * i128::from(self.den), i128::from(self.num))
    }

    /// The frame showing at time `t` (rounded down).
    pub fn frame_at(self, t: Time) -> i64 {
        let n = i128::from(t.num) * i128::from(self.num);
        let d = i128::from(t.den) * i128::from(self.den);
        saturate(n.div_euclid(d))
    }

    /// The nearest frame boundary to `t`.
    pub fn snap(self, t: Time) -> Time {
        let n = i128::from(t.num) * i128::from(self.num);
        let d = i128::from(t.den) * i128::from(self.den);
        let nearest = (2 * n + d).div_euclid(2 * d);
        self.frame_start(saturate(nearest))
    }
}

impl TryFrom<(u32, u32)> for Rate {
    type Error = String;
    fn try_from((num, den): (u32, u32)) -> Result<Self, Self::Error> {
        Rate::new(num, den).map_err(|e| e.to_string())
    }
}

impl From<Rate> for (u32, u32) {
    fn from(rate: Rate) -> Self {
        (rate.num, rate.den)
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{} fps", self.num)
        } else {
            write!(f, "{:.3} fps", self.as_f64())
        }
    }
}

/// A point in time or a length, in exact seconds (`num / den`).
///
/// Always stored reduced, with a positive denominator, so equal times are
/// equal field by field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "(i64, u64)", into = "(i64, u64)")]
pub struct Time {
    num: i64,
    den: u64,
}

impl Time {
    pub const ZERO: Time = Time { num: 0, den: 1 };

    /// A whole number of seconds.
    pub const fn seconds(s: i64) -> Time {
        Time { num: s, den: 1 }
    }

    /// `num / den` seconds. `None` when `den` is zero.
    pub fn from_ratio(num: i64, den: u64) -> Option<Time> {
        (den != 0).then(|| Time::from_i128(i128::from(num), i128::from(den)))
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> u64 {
        self.den
    }

    pub fn is_negative(self) -> bool {
        self.num < 0
    }

    /// Approximate seconds, for display and drawing only.
    pub fn as_secs_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// This time repeated `times` times.
    pub fn times(self, times: i64) -> Time {
        Time::from_i128(i128::from(self.num) * i128::from(times), i128::from(self.den))
    }

    /// `HH:MM:SS:FF` at the given rate (non-drop-frame). Negative times get a minus sign.
    pub fn timecode(self, rate: Rate) -> String {
        let frame = rate.frame_at(self);
        let sign = if frame < 0 { "-" } else { "" };
        let frame = frame.unsigned_abs();
        let fps = u64::from(rate.nominal_fps());
        let (ff, total_seconds) = (frame % fps, frame / fps);
        let (ss, total_minutes) = (total_seconds % 60, total_seconds / 60);
        let (mm, hh) = (total_minutes % 60, total_minutes / 60);
        format!("{sign}{hh:02}:{mm:02}:{ss:02}:{ff:02}")
    }

    /// Builds a reduced time from a wide fraction. Values too large for a
    /// project (millions of years) are clamped rather than wrapped.
    fn from_i128(num: i128, den: i128) -> Time {
        debug_assert!(den != 0);
        let (mut num, mut den) = if den < 0 { (-num, -den) } else { (num, den) };
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()) as i128;
        if g > 1 {
            num /= g;
            den /= g;
        }
        Time {
            num: saturate(num),
            den: u64::try_from(den).unwrap_or(u64::MAX),
        }
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        let a = i128::from(self.num) * i128::from(other.den);
        let b = i128::from(other.num) * i128::from(self.den);
        a.cmp(&b)
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        let num = i128::from(self.num) * i128::from(rhs.den) + i128::from(rhs.num) * i128::from(self.den);
        Time::from_i128(num, i128::from(self.den) * i128::from(rhs.den))
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        self + (-rhs)
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time {
            num: self.num.saturating_neg(),
            den: self.den,
        }
    }
}

impl TryFrom<(i64, u64)> for Time {
    type Error = String;
    fn try_from((num, den): (i64, u64)) -> Result<Self, Self::Error> {
        Time::from_ratio(num, den).ok_or_else(|| "a time with a zero denominator".to_owned())
    }
}

impl From<Time> for (i64, u64) {
    fn from(t: Time) -> Self {
        (t.num, t.den)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} s", self.as_secs_f64())
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

fn saturate(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_reduce_and_reject_zero() {
        assert_eq!(Rate::new(120, 2).unwrap(), Rate::FPS_60);
        assert!(Rate::new(0, 1).is_err());
        assert!(Rate::new(30, 0).is_err());
        assert_eq!(Rate::NTSC_60.nominal_fps(), 60);
    }

    #[test]
    fn ntsc_frames_stay_exact_after_an_hour() {
        let rate = Rate::NTSC_30;
        let hour_of_frames = 30 * 60 * 60;
        let mut t = Time::ZERO;
        for _ in 0..hour_of_frames {
            t = t + rate.frame_duration();
        }
        assert_eq!(rate.frame_at(t), hour_of_frames);
        assert_eq!(t, rate.frame_start(hour_of_frames));
    }

    #[test]
    fn frame_at_rounds_down_and_snap_rounds_to_nearest() {
        let rate = Rate::FPS_60;
        let almost_one = Time::from_ratio(119, 7200).unwrap(); // just under 1/60 s
        assert_eq!(rate.frame_at(almost_one), 0);
        assert_eq!(rate.snap(almost_one), rate.frame_start(1));
        assert_eq!(rate.frame_at(Time::from_ratio(-1, 120).unwrap()), -1);
    }

    #[test]
    fn mixed_rates_add_exactly() {
        let a = Rate::FPS_144.frame_start(144);
        let b = Rate::NTSC_24.frame_start(24);
        assert_eq!(a + b - b, a);
        assert_eq!(a, Time::seconds(1));
    }

    #[test]
    fn ordering_compares_values_not_fields() {
        let half = Time::from_ratio(1, 2).unwrap();
        let third = Time::from_ratio(1, 3).unwrap();
        assert!(third < half);
        assert_eq!(Time::from_ratio(2, 4).unwrap(), half);
        assert!(-half < Time::ZERO);
    }

    #[test]
    fn timecode_counts_hours_minutes_seconds_frames() {
        let rate = Rate::FPS_60;
        let t = Time::seconds(3_723) + rate.frame_start(5); // 1 h 2 min 3 s + 5 frames
        assert_eq!(t.timecode(rate), "01:02:03:05");
        assert_eq!(Time::ZERO.timecode(rate), "00:00:00:00");
    }

    #[test]
    fn zero_denominators_are_refused_when_reading_files() {
        assert!(Time::try_from((1, 0)).is_err());
        assert!(Rate::try_from((60, 0)).is_err());
    }
}
