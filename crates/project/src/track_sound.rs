//! How a track and a timeline's mix sound (format 16): the effects of the
//! Audio page, per audio track, and the master's compressor and limiter.
//!
//! Everything is a whole number (thousandths of a decibel, whole hertz,
//! thousandths for Q and ratio, microseconds for speeds), as clip sound
//! is, so settings compare exactly and a file saved and opened again says
//! the same. The defaults have every effect off, so a project from before
//! format 16 sounds as it did. Turning these into sound is `audio`'s work.
//!
//! A track's volume and pan lines are kept in timeline time (a line is
//! about the mix, not the recording). The volume line is added to the
//! track's volume; while the pan line has points, it is the pan.

use crate::time::Time;
use diag::AppError;
use imbl::Vector;
use serde::{Deserialize, Serialize};

/// What part a track plays in the mix. Ducking listens to dialogue tracks;
/// the Audio page's filter shows tracks by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TrackRole {
    #[default]
    Other,
    Dialogue,
    Music,
    Effects,
    Ambience,
}

/// The high pass: takes out rumble below a frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct HighPass {
    pub on: bool,
    pub freq_hz: u32,
}

impl Default for HighPass {
    fn default() -> HighPass {
        HighPass {
            on: false,
            freq_hz: 80,
        }
    }
}

/// The shape of an EQ band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BandShape {
    LowShelf,
    Peak,
    HighShelf,
}

/// One band of the EQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EqBand {
    pub on: bool,
    pub shape: BandShape,
    pub freq_hz: u32,
    /// How narrow a peak is, in thousandths (1000 is a Q of 1).
    pub q_milli: u32,
    pub gain_mdb: i32,
}

/// How many bands the EQ has.
pub const EQ_BANDS: usize = 6;

/// The parametric EQ: six bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Eq {
    pub on: bool,
    pub bands: [EqBand; EQ_BANDS],
}

impl Default for Eq {
    /// Off and flat: a low shelf, four peaks and a high shelf at 0 dB.
    fn default() -> Eq {
        let band = |shape, freq_hz| EqBand {
            on: true,
            shape,
            freq_hz,
            q_milli: 1_000,
            gain_mdb: 0,
        };
        Eq {
            on: false,
            bands: [
                band(BandShape::LowShelf, 100),
                band(BandShape::Peak, 250),
                band(BandShape::Peak, 1_000),
                band(BandShape::Peak, 3_000),
                band(BandShape::Peak, 6_000),
                band(BandShape::HighShelf, 10_000),
            ],
        }
    }
}

/// A compressor (a track's or the master's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Compressor {
    pub on: bool,
    pub threshold_mdb: i32,
    /// Thousandths: 4000 is 4:1.
    pub ratio_milli: u32,
    pub attack_us: u32,
    pub release_us: u32,
    pub knee_mdb: u32,
    pub makeup_mdb: i32,
}

impl Default for Compressor {
    /// Off, at the Voice preset's numbers.
    fn default() -> Compressor {
        Compressor {
            on: false,
            threshold_mdb: -24_000,
            ratio_milli: 3_000,
            attack_us: 5_000,
            release_us: 120_000,
            knee_mdb: 6_000,
            makeup_mdb: 4_000,
        }
    }
}

/// A learned noise profile: the noise's level in 64 bands by frequency
/// (thousandths of a dB, on the clean-up's own scale), so it is small and
/// works at any rate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoiseProfile {
    pub levels_mdb: Vec<i32>,
}

/// How many bands a noise profile has.
pub const NOISE_BANDS: usize = 64;

/// Noise clean-up (a spectral gate).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Cleanup {
    pub on: bool,
    /// How far above the noise a frequency must be to pass.
    pub threshold_mdb: i32,
    /// How far down the noise goes (0 or less).
    pub reduction_mdb: i32,
    pub release_us: u32,
    /// The noise, once learned; until then the clean-up lets everything through.
    pub profile: Option<NoiseProfile>,
}

impl Default for Cleanup {
    fn default() -> Cleanup {
        Cleanup {
            on: false,
            threshold_mdb: 6_000,
            reduction_mdb: -18_000,
            release_us: 120_000,
            profile: None,
        }
    }
}

/// Ducking: the track turned down while a dialogue track speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Ducking {
    pub on: bool,
    /// The dialogue's speech level over which someone counts as speaking.
    pub threshold_mdb: i32,
    pub hold_us: u32,
    pub down_us: u32,
    pub up_us: u32,
    /// How far down (0 or less).
    pub depth_mdb: i32,
}

impl Default for Ducking {
    fn default() -> Ducking {
        Ducking {
            on: false,
            threshold_mdb: -40_000,
            hold_us: 300_000,
            down_us: 40_000,
            up_us: 400_000,
            depth_mdb: -12_000,
        }
    }
}

/// The Loudness tool: keeps the track near a loudness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Loudness {
    pub on: bool,
    /// Thousandths of a LUFS.
    pub target_mlufs: i32,
    pub max_boost_mdb: u32,
    pub max_cut_mdb: u32,
}

impl Default for Loudness {
    fn default() -> Loudness {
        Loudness {
            on: false,
            target_mlufs: -18_000,
            max_boost_mdb: 12_000,
            max_cut_mdb: 12_000,
        }
    }
}

/// A point of a volume or pan line: a moment on the timeline and a value
/// (thousandths of a dB for volume, millionths for pan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinePoint {
    pub at: Time,
    pub value: i32,
}

/// How an audio track sounds besides its volume, mute and solo.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackSound {
    pub role: TrackRole,
    /// Millionths: -1 000 000 all left, 1 000 000 all right.
    pub pan: i32,
    pub high_pass: HighPass,
    pub eq: Eq,
    pub compressor: Compressor,
    pub cleanup: Cleanup,
    pub ducking: Ducking,
    pub loudness: Loudness,
    /// Added to the track's volume, thousandths of a dB.
    pub volume_line: Vector<LinePoint>,
    /// While it has points, the pan (millionths).
    pub pan_line: Vector<LinePoint>,
}

/// The master's true-peak limiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Limiter {
    pub on: bool,
    /// The highest true peak let out, thousandths of a dBTP.
    pub ceiling_mdb: i32,
}

impl Default for Limiter {
    fn default() -> Limiter {
        Limiter {
            on: false,
            ceiling_mdb: -1_000,
        }
    }
}

/// The timeline's mix after its tracks: a compressor and a true-peak
/// limiter (its volume is the timeline's `master_gain_db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterSound {
    pub compressor: Compressor,
    pub limiter: Limiter,
}

/// All the way to one side, for pan.
const SIDE: i32 = 1_000_000;

fn within<T: PartialOrd>(value: T, low: T, high: T) -> bool {
    low <= value && value <= high
}

fn refused(what: &str) -> AppError {
    AppError::new("That sound setting can't be used")
        .why(what)
        .fix("Reset the effect on the Audio page and set it again.")
}

impl Compressor {
    fn check(&self) -> diag::Result<()> {
        let ok = within(self.threshold_mdb, -60_000, 0)
            && within(self.ratio_milli, 1_000, 20_000)
            && within(self.attack_us, 100, 200_000)
            && within(self.release_us, 10_000, 2_000_000)
            && within(self.knee_mdb, 0, 24_000)
            && within(self.makeup_mdb, 0, 24_000);
        ok.then_some(()).ok_or_else(|| {
            refused(
                "A compressor's threshold goes from -60 to 0 dB, its ratio from 1:1 to 20:1, attack from 0.1 to \
                 200 ms, release from 10 ms to 2 s, knee and makeup from 0 to 24 dB.",
            )
        })
    }
}

/// Points in time order, never two at one moment, none before the start,
/// values from `low` to `high`.
fn check_line(line: &Vector<LinePoint>, low: i32, high: i32) -> bool {
    line.iter()
        .all(|p| !p.at.is_negative() && within(p.value, low, high))
        && line.iter().zip(line.iter().skip(1)).all(|(a, b)| a.at < b.at)
}

impl TrackSound {
    /// The bottom of a volume line: -60 dB.
    pub const LINE_QUIETEST: i32 = -60_000;
    /// The top of a volume line: +12 dB.
    pub const LINE_LOUDEST: i32 = 12_000;

    /// Whether every effect is off and nothing moves: the track sounds as
    /// its volume, mute and solo alone say.
    pub fn is_plain(&self) -> bool {
        let TrackSound { role, .. } = self;
        *self
            == TrackSound {
                role: *role,
                ..TrackSound::default()
            }
    }

    /// Refuses, explained, settings that can't be used.
    pub fn check(&self) -> diag::Result<()> {
        if !within(self.pan, -SIDE, SIDE) {
            return Err(refused("Pan goes from all left to all right."));
        }
        if !within(self.high_pass.freq_hz, 20, 500) {
            return Err(refused("The high pass goes from 20 to 500 Hz."));
        }
        let band_ok = |b: &EqBand| {
            within(b.freq_hz, 20, 20_000)
                && within(b.q_milli, 100, 20_000)
                && within(b.gain_mdb, -24_000, 24_000)
        };
        if !self.eq.bands.iter().all(band_ok) {
            return Err(refused(
                "An EQ band goes from 20 Hz to 20 kHz, its width (Q) from 0.1 to 20 and its gain from -24 to +24 dB.",
            ));
        }
        self.compressor.check()?;
        let c = &self.cleanup;
        let profile_ok = c.profile.as_ref().is_none_or(|p| {
            p.levels_mdb.len() == NOISE_BANDS && p.levels_mdb.iter().all(|l| within(*l, -200_000, 60_000))
        });
        if !(within(c.threshold_mdb, 0, 24_000)
            && within(c.reduction_mdb, -60_000, 0)
            && within(c.release_us, 10_000, 2_000_000)
            && profile_ok)
        {
            return Err(refused(
                "Clean-up's threshold goes from 0 to 24 dB over the noise, its reduction from 0 to -60 dB, its \
                 release from 10 ms to 2 s, and a noise profile has 64 bands.",
            ));
        }
        let d = &self.ducking;
        if !(within(d.threshold_mdb, -80_000, 0)
            && within(d.hold_us, 0, 2_000_000)
            && within(d.down_us, 1_000, 2_000_000)
            && within(d.up_us, 10_000, 5_000_000)
            && within(d.depth_mdb, -60_000, 0))
        {
            return Err(refused(
                "Ducking's threshold goes from -80 to 0 dB, hold up to 2 s, down from 1 ms to 2 s, up from 10 ms \
                 to 5 s, and depth from 0 to -60 dB.",
            ));
        }
        let l = &self.loudness;
        if !(within(l.target_mlufs, -40_000, -5_000)
            && within(l.max_boost_mdb, 0, 24_000)
            && within(l.max_cut_mdb, 0, 24_000))
        {
            return Err(refused(
                "The loudness aimed for goes from -40 to -5 LUFS, and it moves the track up or down by at most 24 dB.",
            ));
        }
        if !check_line(
            &self.volume_line,
            TrackSound::LINE_QUIETEST,
            TrackSound::LINE_LOUDEST,
        ) || !check_line(&self.pan_line, -SIDE, SIDE)
        {
            return Err(refused(
                "A line's points are in time order from the timeline's start, never two at one moment; volume goes \
                 from -60 to +12 dB and pan from all left to all right.",
            ));
        }
        Ok(())
    }
}

impl MasterSound {
    /// Refuses, explained, settings that can't be used.
    pub fn check(&self) -> diag::Result<()> {
        self.compressor.check()?;
        if !within(self.limiter.ceiling_mdb, -12_000, 0) {
            return Err(refused("The limiter's ceiling goes from -12 to 0 dBTP."));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_off_and_allowed() {
        let s = TrackSound::default();
        assert!(s.is_plain());
        assert!(s.check().is_ok());
        assert!(!s.high_pass.on && !s.eq.on && !s.compressor.on && !s.cleanup.on);
        assert!(!s.ducking.on && !s.loudness.on);
        assert!(MasterSound::default().check().is_ok());
        assert!(!MasterSound::default().limiter.on);
        let dialogue = TrackSound {
            role: TrackRole::Dialogue,
            ..TrackSound::default()
        };
        assert!(dialogue.is_plain(), "a role alone changes no sound");
    }

    #[test]
    fn values_out_of_range_are_refused() {
        let mut s = TrackSound::default();
        s.eq.bands[2].gain_mdb = 30_000;
        assert!(s.check().is_err());
        let mut s = TrackSound::default();
        s.compressor.ratio_milli = 500;
        assert!(s.check().is_err());
        let mut s = TrackSound::default();
        s.cleanup.profile = Some(NoiseProfile {
            levels_mdb: vec![0; 10],
        });
        assert!(s.check().is_err());
        s.cleanup.profile = Some(NoiseProfile {
            levels_mdb: vec![-90_000; NOISE_BANDS],
        });
        assert!(s.check().is_ok());
        let mut m = MasterSound::default();
        m.limiter.ceiling_mdb = 1_000;
        assert!(m.check().is_err());
    }

    #[test]
    fn lines_must_be_in_order_and_in_range() {
        let point = |s, value| LinePoint {
            at: Time::seconds(s),
            value,
        };
        let mut s = TrackSound {
            volume_line: [point(0, 0), point(2, -6_000)].into_iter().collect(),
            ..TrackSound::default()
        };
        assert!(s.check().is_ok());
        assert!(!s.is_plain());
        s.volume_line.push_back(point(2, 0));
        assert!(s.check().is_err(), "two at one moment");
        s.volume_line = [point(-1, 0)].into_iter().collect();
        assert!(s.check().is_err(), "before the start");
        s.volume_line = Vector::new();
        s.pan_line = [point(1, 2 * SIDE)].into_iter().collect();
        assert!(s.check().is_err());
    }

    #[test]
    fn a_file_without_some_values_reads_them_as_off() {
        let s: TrackSound = ron::from_str("(role: Dialogue, eq: (on: true))").expect("reads");
        assert_eq!(s.role, TrackRole::Dialogue);
        assert!(s.eq.on);
        assert_eq!(s.eq.bands, Eq::default().bands);
        assert_eq!(s.compressor, Compressor::default());
    }
}
