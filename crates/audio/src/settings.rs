//! Settings as plain values for the mixer, built from the project, and the
//! changes that travel to it one at a time.
//!
//! [`TrackSettings`] is everything about a track that is a number or a
//! switch; what needs memory (its lines, its noise) is built separately
//! ([`lines_from_project`], [`noise_from_project`]) and swapped in.
//! [`TrackChange`] is one section of it (the numbers that belong together
//! travel together); [`TrackSettings::changes_from`] says which sections
//! differ, so after an edit or an undo only those are sent.

use crate::dsp::bands;
use crate::dsp::cleanup::{CleanupSettings, NoiseProfile};
use crate::dsp::duck::DuckSettings;
use crate::dsp::dynamics::CompressorSettings;
use crate::dsp::filters::{BANDS, Band, FLAT, Shape};
use crate::dsp::level::LevelSettings;
use crate::dsp::lines::{Line, Lines};
use project::track_sound as p;

/// The high pass's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighPassSettings {
    pub on: bool,
    pub freq: f32,
}

/// The EQ's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqSettings {
    pub on: bool,
    pub bands: [Band; BANDS],
}

/// Everything about a track that is a number or a switch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    /// A dialogue track: its sound is what ducking listens to.
    pub key: bool,
    pub high_pass: HighPassSettings,
    pub eq: EqSettings,
    pub compressor: CompressorSettings,
    pub cleanup: CleanupSettings,
    pub duck: DuckSettings,
    pub level: LevelSettings,
    pub volume_db: f32,
    /// -1 all left to 1 all right.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for TrackSettings {
    /// Every effect off, at 0 dB, centred: the sound as it is.
    fn default() -> TrackSettings {
        TrackSettings {
            key: false,
            high_pass: HighPassSettings {
                on: false,
                freq: 80.0,
            },
            eq: EqSettings {
                on: false,
                bands: FLAT,
            },
            compressor: CompressorSettings::OFF,
            cleanup: CleanupSettings::OFF,
            duck: DuckSettings::OFF,
            level: LevelSettings::OFF,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// One section of a track's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackChange {
    Key(bool),
    HighPass(HighPassSettings),
    EqOn(bool),
    EqBand(u8, Band),
    Compressor(CompressorSettings),
    Cleanup(CleanupSettings),
    Duck(DuckSettings),
    Level(LevelSettings),
    Volume(f32),
    Pan(f32),
    Mute(bool),
    Solo(bool),
}

impl TrackSettings {
    /// Applies one change.
    pub fn apply(&mut self, change: TrackChange) {
        match change {
            TrackChange::Key(v) => self.key = v,
            TrackChange::HighPass(v) => self.high_pass = v,
            TrackChange::EqOn(v) => self.eq.on = v,
            TrackChange::EqBand(i, b) => {
                if let Some(band) = self.eq.bands.get_mut(usize::from(i)) {
                    *band = b;
                }
            }
            TrackChange::Compressor(v) => self.compressor = v,
            TrackChange::Cleanup(v) => self.cleanup = v,
            TrackChange::Duck(v) => self.duck = v,
            TrackChange::Level(v) => self.level = v,
            TrackChange::Volume(v) => self.volume_db = v,
            TrackChange::Pan(v) => self.pan = v,
            TrackChange::Mute(v) => self.mute = v,
            TrackChange::Solo(v) => self.solo = v,
        }
    }

    /// The changes that turn `old` into these settings, section by section.
    pub fn changes_from(&self, old: &TrackSettings) -> Vec<TrackChange> {
        let mut out = Vec::new();
        let mut differs = |same: bool, change: TrackChange| {
            if !same {
                out.push(change);
            }
        };
        differs(self.key == old.key, TrackChange::Key(self.key));
        differs(
            self.high_pass == old.high_pass,
            TrackChange::HighPass(self.high_pass),
        );
        differs(self.eq.on == old.eq.on, TrackChange::EqOn(self.eq.on));
        for (i, (new, was)) in self.eq.bands.iter().zip(&old.eq.bands).enumerate() {
            differs(new == was, TrackChange::EqBand(i as u8, *new));
        }
        differs(
            self.compressor == old.compressor,
            TrackChange::Compressor(self.compressor),
        );
        differs(self.cleanup == old.cleanup, TrackChange::Cleanup(self.cleanup));
        differs(self.duck == old.duck, TrackChange::Duck(self.duck));
        differs(self.level == old.level, TrackChange::Level(self.level));
        differs(
            self.volume_db == old.volume_db,
            TrackChange::Volume(self.volume_db),
        );
        differs(self.pan == old.pan, TrackChange::Pan(self.pan));
        differs(self.mute == old.mute, TrackChange::Mute(self.mute));
        differs(self.solo == old.solo, TrackChange::Solo(self.solo));
        out
    }

    /// A project track's settings.
    pub fn from_project(track: &project::Track) -> TrackSettings {
        let s = &track.sound;
        let eq_on = s.eq.on;
        TrackSettings {
            key: s.role == p::TrackRole::Dialogue,
            high_pass: HighPassSettings {
                on: s.high_pass.on,
                freq: s.high_pass.freq_hz as f32,
            },
            eq: EqSettings {
                on: eq_on,
                bands: s.eq.bands.map(|b| Band {
                    on: b.on,
                    shape: match b.shape {
                        p::BandShape::LowShelf => Shape::LowShelf,
                        p::BandShape::Peak => Shape::Peak,
                        p::BandShape::HighShelf => Shape::HighShelf,
                    },
                    freq: b.freq_hz as f32,
                    q: milli(b.q_milli as i32),
                    gain_db: milli(b.gain_mdb),
                }),
            },
            compressor: compressor(&s.compressor),
            cleanup: CleanupSettings {
                on: s.cleanup.on,
                threshold_db: milli(s.cleanup.threshold_mdb),
                reduction_db: milli(s.cleanup.reduction_mdb),
                release_ms: micro_to_ms(s.cleanup.release_us),
            },
            duck: DuckSettings {
                on: s.ducking.on,
                threshold_db: milli(s.ducking.threshold_mdb),
                hold_ms: micro_to_ms(s.ducking.hold_us),
                down_ms: micro_to_ms(s.ducking.down_us),
                up_ms: micro_to_ms(s.ducking.up_us),
                depth_db: milli(s.ducking.depth_mdb),
            },
            level: LevelSettings {
                on: s.loudness.on,
                target_lufs: milli(s.loudness.target_mlufs),
                max_boost_db: milli(s.loudness.max_boost_mdb as i32),
                max_cut_db: milli(s.loudness.max_cut_mdb as i32),
            },
            volume_db: track.gain_db,
            pan: s.pan as f32 / 1_000_000.0,
            mute: track.muted,
            solo: track.solo,
        }
    }
}

fn milli(v: i32) -> f32 {
    v as f32 / 1_000.0
}

fn micro_to_ms(v: u32) -> f32 {
    v as f32 / 1_000.0
}

fn compressor(c: &p::Compressor) -> CompressorSettings {
    CompressorSettings {
        on: c.on,
        threshold_db: milli(c.threshold_mdb),
        ratio: c.ratio_milli as f32 / 1_000.0,
        attack_ms: micro_to_ms(c.attack_us),
        release_ms: micro_to_ms(c.release_us),
        knee_db: milli(c.knee_mdb as i32),
        makeup_db: milli(c.makeup_mdb),
    }
}

/// The master's limiter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterSettings {
    pub on: bool,
    /// dBTP.
    pub ceiling_db: f32,
}

/// The master's settings: the mix's volume, compressor and limiter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasterSettings {
    pub volume_db: f32,
    pub compressor: CompressorSettings,
    /// A gain after the compressor and before the limiter: export's
    /// normalising. Not in the project; 0 while playing.
    pub trim_db: f32,
    pub limiter: LimiterSettings,
}

impl Default for MasterSettings {
    fn default() -> MasterSettings {
        MasterSettings {
            volume_db: 0.0,
            compressor: CompressorSettings::OFF,
            trim_db: 0.0,
            limiter: LimiterSettings {
                on: false,
                ceiling_db: -1.0,
            },
        }
    }
}

/// One section of the master's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MasterChange {
    Volume(f32),
    Compressor(CompressorSettings),
    Trim(f32),
    Limiter(LimiterSettings),
}

impl MasterSettings {
    pub fn apply(&mut self, change: MasterChange) {
        match change {
            MasterChange::Volume(v) => self.volume_db = v,
            MasterChange::Compressor(v) => self.compressor = v,
            MasterChange::Trim(v) => self.trim_db = v,
            MasterChange::Limiter(v) => self.limiter = v,
        }
    }

    /// The changes that turn `old` into these settings.
    pub fn changes_from(&self, old: &MasterSettings) -> Vec<MasterChange> {
        let mut out = Vec::new();
        if self.volume_db != old.volume_db {
            out.push(MasterChange::Volume(self.volume_db));
        }
        if self.compressor != old.compressor {
            out.push(MasterChange::Compressor(self.compressor));
        }
        if self.trim_db != old.trim_db {
            out.push(MasterChange::Trim(self.trim_db));
        }
        if self.limiter != old.limiter {
            out.push(MasterChange::Limiter(self.limiter));
        }
        out
    }

    /// A project timeline's master settings.
    pub fn from_project(sequence: &project::Sequence) -> MasterSettings {
        let m = &sequence.master_sound;
        MasterSettings {
            volume_db: sequence.master_gain_db,
            compressor: compressor(&m.compressor),
            trim_db: 0.0,
            limiter: LimiterSettings {
                on: m.limiter.on,
                ceiling_db: milli(m.limiter.ceiling_mdb),
            },
        }
    }
}

/// The sample at `rate` nearest a moment on the timeline (exact whole-number
/// arithmetic on the fraction, halves rounded up; before the start is 0).
pub fn sample_at(at: project::Time, rate: u32) -> u64 {
    let num = i128::from(at.num()) * i128::from(rate);
    let den = i128::from(at.den()).max(1);
    let rounded = (2 * num + den).div_euclid(2 * den);
    u64::try_from(rounded.max(0)).unwrap_or(u64::MAX)
}

/// A project track's volume and pan lines at `rate`. Allocates: built off
/// the real-time thread and swapped in.
pub fn lines_from_project(sound: &p::TrackSound, rate: u32) -> Box<Lines> {
    Box::new(Lines {
        volume_db: line(&sound.volume_line, rate, 1_000.0),
        pan: line(&sound.pan_line, rate, 1_000_000.0),
    })
}

fn line<'a>(points: impl IntoIterator<Item = &'a p::LinePoint>, rate: u32, scale: f32) -> Line {
    Line::new(
        points
            .into_iter()
            .map(|q| (sample_at(q.at, rate), q.value as f32 / scale))
            .collect(),
    )
}

/// A project noise profile as the clean-up's, `None` when there is none
/// (or it is not 64 bands).
pub fn noise_from_project(sound: &p::TrackSound) -> Option<NoiseProfile> {
    let levels = &sound.cleanup.profile.as_ref()?.levels_mdb;
    let mut out = [0f32; bands::COUNT];
    if levels.len() != out.len() {
        return None;
    }
    for (o, l) in out.iter_mut().zip(levels) {
        *o = milli(*l);
    }
    Some(NoiseProfile(out))
}

/// A learned noise profile as the project keeps it.
pub fn noise_to_project(profile: &NoiseProfile) -> p::NoiseProfile {
    p::NoiseProfile {
        levels_mdb: profile
            .0
            .iter()
            .map(|db| (db.clamp(-200.0, 60.0) * 1_000.0).round() as i32)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Project, SequenceSettings, Time};

    #[test]
    fn a_plain_project_track_is_every_effect_off() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active().expect("a timeline");
        let t = TrackSettings::from_project(&seq.audio_tracks[0]);
        assert_eq!(t, TrackSettings::default());
        assert_eq!(MasterSettings::from_project(seq), MasterSettings::default());
        assert!(lines_from_project(&seq.audio_tracks[0].sound, 48_000).is_empty());
        assert!(noise_from_project(&seq.audio_tracks[0].sound).is_none());
    }

    #[test]
    fn project_numbers_become_the_mixers() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active().expect("a timeline");
        let mut sound = project::TrackSound {
            role: project::TrackRole::Dialogue,
            pan: -500_000,
            ..Default::default()
        };
        sound.eq.on = true;
        sound.eq.bands[2].gain_mdb = -4_500;
        sound.eq.bands[2].q_milli = 2_500;
        sound.compressor.on = true;
        sound.ducking.hold_us = 250_000;
        sound.volume_line.push_back(p::LinePoint {
            at: Time::from_ratio(1, 3).expect("a third"),
            value: -6_000,
        });
        let learned = NoiseProfile([-80.5; bands::COUNT]);
        sound.cleanup.profile = Some(noise_to_project(&learned));
        let p = Command::SetTrackSound {
            sequence: seq.id,
            track: seq.audio_tracks[0].id,
            sound: Box::new(sound),
        }
        .apply(&p)
        .expect("allowed");
        let track = &p.active().expect("a timeline").audio_tracks[0];
        let t = TrackSettings::from_project(track);
        assert!(t.key && t.eq.on && t.compressor.on);
        assert_eq!(t.pan, -0.5);
        assert_eq!((t.eq.bands[2].gain_db, t.eq.bands[2].q), (-4.5, 2.5));
        assert_eq!(t.duck.hold_ms, 250.0);
        assert_eq!(noise_from_project(&track.sound), Some(learned));
        let mut lines = lines_from_project(&track.sound, 48_000);
        assert_eq!(
            lines.volume_db.value_at(16_000),
            Some(-6.0),
            "a third of a second is sample 16 000"
        );
        // Every difference is a change, and applying them gets there.
        let mut s = TrackSettings::default();
        let changes = t.changes_from(&s);
        assert!(changes.len() >= 5);
        for c in changes {
            s.apply(c);
        }
        assert_eq!(s, t);
        assert!(t.changes_from(&t).is_empty());
    }

    #[test]
    fn moments_land_on_the_nearest_sample() {
        let t = |n, d| Time::from_ratio(n, d).expect("a time");
        assert_eq!(sample_at(t(1, 1), 48_000), 48_000);
        assert_eq!(sample_at(t(1, 30), 44_100), 1_470);
        assert_eq!(sample_at(t(1001, 60_000), 48_000), 801, "800.8 rounds up");
        assert_eq!(sample_at(t(-1, 1), 48_000), 0);
    }
}
