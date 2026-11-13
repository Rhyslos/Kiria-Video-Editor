//! How a clip sounds: how loud, where it sits between the speakers, and how
//! it fades in and out.
//!
//! Volume and pan are whole numbers (thousandths of a decibel, millionths of
//! the way to one side), so two clips compare exactly and a file saved and
//! opened again says the same. Fades are exact [`Time`] lengths on the
//! timeline: a fade lasts as long on the timeline whatever the clip's speed.
//! Turning these into gains on samples is the mixer's work (`engine::mix`).

use crate::time::Time;
use crate::track_sound::{LinePoint, TrackSound};
use diag::AppError;
use imbl::Vector;
use serde::{Deserialize, Serialize};

/// A clip's sound settings. The default leaves the sound as it is: as loud
/// as recorded, where it was recorded, no fades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    /// Volume in thousandths of a decibel: 0 leaves it, [`Sound::LOUDEST`]
    /// is +12 dB, and [`Sound::SILENT`] (-60 dB, the bottom of the range)
    /// is no sound at all.
    pub gain_mdb: i32,
    /// Where it sits between the speakers, in millionths: -[`Sound::SIDE`]
    /// all left, 0 as recorded, [`Sound::SIDE`] all right.
    pub pan: i32,
    /// How long it takes to rise from silence at the clip's start.
    pub fade_in: Time,
    /// How long it takes to fall to silence at the clip's end.
    pub fade_out: Time,
}

impl Default for Sound {
    fn default() -> Sound {
        Sound::PLAIN
    }
}

impl Sound {
    /// The sound as it is.
    pub const PLAIN: Sound = Sound {
        gain_mdb: 0,
        pan: 0,
        fade_in: Time::ZERO,
        fade_out: Time::ZERO,
    };

    /// The bottom of the volume range: silent.
    pub const SILENT: i32 = -60_000;
    /// The top of the volume range: +12 dB.
    pub const LOUDEST: i32 = 12_000;
    /// All the way to one side.
    pub const SIDE: i32 = 1_000_000;

    /// Whether it is silent: the volume is at the bottom of its range.
    pub fn is_silent(&self) -> bool {
        self.gain_mdb <= Sound::SILENT
    }

    /// Whether the volume or the pan differ from the sound as recorded.
    pub fn is_mixed(&self) -> bool {
        (self.gain_mdb, self.pan) != (0, 0)
    }

    /// Whether it fades in or out.
    pub fn is_faded(&self) -> bool {
        self.fade_in > Time::ZERO || self.fade_out > Time::ZERO
    }

    /// The same settings with volume and pan brought inside their range and
    /// no fade shorter than nothing: what a slider dragged too far means.
    /// Fades are brought within a clip's length by [`Sound::fitted`].
    pub fn within_limits(self) -> Sound {
        Sound {
            gain_mdb: self.gain_mdb.clamp(Sound::SILENT, Sound::LOUDEST),
            pan: self.pan.clamp(-Sound::SIDE, Sound::SIDE),
            fade_in: self.fade_in.max(Time::ZERO),
            fade_out: self.fade_out.max(Time::ZERO),
        }
    }

    /// Whether both fades fit in a clip `length` long: neither shorter than
    /// nothing, and together no longer than the clip.
    pub fn fits(&self, length: Time) -> bool {
        !self.fade_in.is_negative() && !self.fade_out.is_negative() && self.fade_in + self.fade_out <= length
    }

    /// The same sound on a clip `length` long: the fade in at most the
    /// whole clip, and the fade out at most what the fade in leaves. A clip
    /// trimmed shorter than its fades keeps its fade in and loses the end of
    /// its fade out first.
    pub fn fitted(self, length: Time) -> Sound {
        let length = length.max(Time::ZERO);
        let fade_in = self.fade_in.clamp(Time::ZERO, length);
        Sound {
            fade_in,
            fade_out: self.fade_out.clamp(Time::ZERO, length - fade_in),
            ..self
        }
    }

    /// The same sound without its fade in: the right half of a cut.
    pub fn without_fade_in(self) -> Sound {
        Sound {
            fade_in: Time::ZERO,
            ..self
        }
    }

    /// The same sound without its fade out: the left half of a cut.
    pub fn without_fade_out(self) -> Sound {
        Sound {
            fade_out: Time::ZERO,
            ..self
        }
    }

    /// Refuses, explained, settings that can't be used on a clip `length`
    /// long.
    pub fn check(&self, length: Time) -> diag::Result<()> {
        if *self != self.within_limits() {
            return Err(AppError::new("That sound setting can't be used")
                .why("Volume goes from -60 dB (silent) to +12 dB, and pan from all left to all right.")
                .fix("Reset the clip's Audio in the inspector and set it again."));
        }
        if !self.fits(length) {
            return Err(AppError::new("Those fades are too long")
                .why("A clip's fade in and fade out together can't be longer than the clip.")
                .fix("Make one of the fades shorter, or the clip longer."));
        }
        Ok(())
    }
}

/// A clip's own volume and pan lines (format 17): points at moments of its
/// file (source time), like look keyframes, so they stay on the sound
/// they were drawn on through moves, trims and speed changes. Straight
/// between points and flat before the first and after the last; a cut
/// clip's halves both keep every point, so the line goes on across the
/// cut as it was.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipLines {
    /// Added to the clip's volume, thousandths of a dB (-60 to +12 dB).
    pub volume: Vector<LinePoint>,
    /// While it has points, the clip's pan (millionths, all left to all right).
    pub pan: Vector<LinePoint>,
}

impl ClipLines {
    /// Whether neither line has a point.
    pub fn is_empty(&self) -> bool {
        self.volume.is_empty() && self.pan.is_empty()
    }

    /// The volume line's value at `source` (a moment of the file), in
    /// thousandths of a dB; 0 without points.
    pub fn volume_at(&self, source: Time) -> i32 {
        line_value(&self.volume, source).unwrap_or(0)
    }

    /// The pan line's value at `source`, when it has points.
    pub fn pan_at(&self, source: Time) -> Option<i32> {
        line_value(&self.pan, source)
    }

    /// Refuses, explained, lines that can't be used.
    pub fn check(&self) -> diag::Result<()> {
        if crate::track_sound::check_line(&self.volume, TrackSound::LINE_QUIETEST, TrackSound::LINE_LOUDEST)
            && crate::track_sound::check_line(&self.pan, -Sound::SIDE, Sound::SIDE)
        {
            return Ok(());
        }
        Err(AppError::new("That line can't be used")
            .why(
                "A line's points are in time order, never two at one moment; volume goes from -60 to +12 dB and \
                 pan from all left to all right.",
            )
            .fix("Remove the line's points and draw it again."))
    }
}

/// A line's value at `at`: straight between points (worked out in whole
/// numbers, so preview and export agree), flat outside them.
pub fn line_value(line: &Vector<LinePoint>, at: Time) -> Option<i32> {
    let first = line.front()?;
    let last = line.back()?;
    if at <= first.at {
        return Some(first.value);
    }
    if at >= last.at {
        return Some(last.value);
    }
    let next = line.iter().position(|p| p.at > at)?;
    let (a, b) = (line.get(next - 1)?, line.get(next)?);
    let span = crate::ramp::flicks(b.at - a.at);
    if span <= 0 {
        return Some(b.value);
    }
    let into = crate::ramp::flicks(at - a.at);
    let delta = i128::from(b.value) - i128::from(a.value);
    let value = i128::from(a.value) + delta * into / span;
    Some(i32::try_from(value).unwrap_or(a.value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(seconds: i64, value: i32) -> LinePoint {
        LinePoint {
            at: Time::from_ratio(seconds, 1).unwrap(),
            value,
        }
    }

    #[test]
    fn a_clip_line_is_straight_between_points_and_flat_outside() {
        let lines = ClipLines {
            volume: [point(2, -6_000), point(4, 0)].into_iter().collect(),
            pan: Vector::new(),
        };
        let at = |s: i64, den: u64| lines.volume_at(Time::from_ratio(s, den).unwrap());
        assert_eq!(at(0, 1), -6_000);
        assert_eq!(at(3, 1), -3_000);
        assert_eq!(at(7, 2), -1_500);
        assert_eq!(at(9, 1), 0);
        assert_eq!(lines.pan_at(Time::ZERO), None, "no pan line: the clip's pan");
        assert_eq!(ClipLines::default().volume_at(Time::ZERO), 0);
        assert!(lines.check().is_ok());
        let backwards = ClipLines {
            volume: [point(4, 0), point(2, 0)].into_iter().collect(),
            pan: Vector::new(),
        };
        assert!(backwards.check().is_err());
        let wild = ClipLines {
            volume: Vector::new(),
            pan: [point(1, 2 * Sound::SIDE)].into_iter().collect(),
        };
        assert!(wild.check().is_err());
    }

    #[test]
    fn the_default_changes_nothing() {
        let sound = Sound::default();
        assert_eq!(sound, Sound::PLAIN);
        assert!(!sound.is_mixed() && !sound.is_faded() && !sound.is_silent());
        assert!(sound.check(Time::seconds(1)).is_ok());
        assert!(sound.fits(Time::ZERO));
    }

    #[test]
    fn values_out_of_range_are_brought_back_and_refused_as_they_are() {
        let wild = Sound {
            gain_mdb: 20_000,
            pan: -3 * Sound::SIDE,
            fade_in: Time::seconds(-1),
            ..Sound::PLAIN
        };
        assert!(wild.check(Time::seconds(10)).is_err());
        let tamed = wild.within_limits();
        assert_eq!(
            tamed,
            Sound {
                gain_mdb: Sound::LOUDEST,
                pan: -Sound::SIDE,
                ..Sound::PLAIN
            }
        );
        assert!(tamed.check(Time::seconds(10)).is_ok());
        assert!(
            Sound {
                gain_mdb: Sound::SILENT,
                ..Sound::PLAIN
            }
            .is_silent()
        );
    }

    #[test]
    fn fades_must_fit_the_clip_and_are_cut_to_fit() {
        let half = Time::from_ratio(1, 2).unwrap();
        let sound = Sound {
            fade_in: Time::seconds(2),
            fade_out: Time::seconds(3),
            ..Sound::PLAIN
        };
        assert!(sound.fits(Time::seconds(5)));
        assert!(!sound.fits(Time::seconds(4)));
        assert!(sound.check(Time::seconds(4)).is_err());
        // Shorter: the fade in stays, the fade out gives way.
        let cut = sound.fitted(Time::seconds(4));
        assert_eq!((cut.fade_in, cut.fade_out), (Time::seconds(2), Time::seconds(2)));
        let tiny = sound.fitted(half);
        assert_eq!((tiny.fade_in, tiny.fade_out), (half, Time::ZERO));
        assert_eq!(sound.fitted(Time::seconds(9)), sound, "room enough: unchanged");
        assert_eq!(sound.without_fade_in().fade_in, Time::ZERO);
        assert_eq!(sound.without_fade_out().fade_out, Time::ZERO);
    }

    #[test]
    fn a_file_without_some_values_reads_them_as_plain() {
        let sound: Sound = ron::from_str("(gain_mdb: -3000, fade_in: (1, 2))").unwrap();
        assert_eq!(
            sound,
            Sound {
                gain_mdb: -3_000,
                fade_in: Time::from_ratio(1, 2).unwrap(),
                ..Sound::PLAIN
            }
        );
    }
}
