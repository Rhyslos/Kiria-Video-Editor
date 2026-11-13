//! The one-click fixes: each is an ordinary, undoable
//! [`project::Command::SetTrackSound`] on one track, changing only the
//! effect it names.
//!
//! - **Lower under the dialogue:** ducking on, keyed by the dialogue tracks
//!   (the mixer's own key), [`DUCK_DEPTH_MDB`] down, with the effect's own
//!   hold and speeds; already ducking that deep, it goes [`DEEPER_MDB`]
//!   further, to at most [`DEEPEST_DUCK_MDB`].
//! - **Cut around 300 Hz:** an EQ dip of [`CUT_MDB`] at 300 Hz, Q 1.
//! - **Make room for the voice:** an EQ dip of [`ROOM_MDB`] at 2.5 kHz, Q 1.
//!
//! An EQ dip goes in a peak band already within half an octave of its
//! frequency and not boosting (made deeper there, to at most
//! [`DEEPEST_DIP_MDB`]), else in a peak band that is flat (0 dB, on or off), moved there. With
//! the EQ off, bands set before stay as they were but switched off, so
//! only the dip is new in what is heard.
//!
//! A fix already made on its track says so ([`Fix::applied_mdb`]: the cut
//! or ducking there now) and offers to go further; one already as far as
//! it goes is not offered ([`Fix::useful`]). A fix may not clear a finding
//! by itself (three tracks piling up are still three after one is cut),
//! so the finding stays, its button saying what is done.

use diag::AppError;
use project::track_sound::{BandShape, EqBand, TrackRole, TrackSound};
use project::{Command, Project, SequenceId, TrackId};

/// How far ducking turns a track down under dialogue, thousandths of a dB.
pub const DUCK_DEPTH_MDB: i32 = -10_000;
/// How much deeper a track that already ducks that far goes.
pub const DEEPER_MDB: i32 = -6_000;
/// The deepest the fix ducks.
pub const DEEPEST_DUCK_MDB: i32 = -24_000;
/// The cut around 300 Hz.
pub const CUT_HZ: u32 = 300;
pub const CUT_MDB: i32 = -3_500;
/// The dip that makes room for the voice.
pub const ROOM_HZ: u32 = 2_500;
pub const ROOM_MDB: i32 = -2_500;
/// Both dips are this wide (thousandths: a Q of 1).
pub const DIP_Q_MILLI: u32 = 1_000;
/// The deepest a dip goes when the fix is used again.
pub const DEEPEST_DIP_MDB: i32 = -9_000;

/// What a fix does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Ducking on: the track turned down while the dialogue speaks. The
    /// threshold comes from how loud the dialogue was found to be.
    LowerUnderDialogue { threshold_mdb: i32 },
    /// A gentle EQ cut around 300 Hz.
    CutLowMiddle,
    /// A gentle EQ dip around 2.5 kHz, where words are understood.
    MakeRoomForVoice,
}

/// One fix on offer for a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub sequence: SequenceId,
    pub track: TrackId,
    pub remedy: Remedy,
    /// What the button says: "Lower the music under the dialogue".
    pub label: String,
    /// The same fix already on the track: its cut, or how far it ducks
    /// now, thousandths of a dB. Pressing again goes further.
    pub applied_mdb: Option<i32>,
}

fn gone() -> AppError {
    AppError::new("That fix can't be used now")
        .why("The track it was for is not in the timeline any more.")
        .fix("Check clarity again.")
}

impl Fix {
    pub fn new(sequence: SequenceId, track: &project::Track, remedy: Remedy) -> Fix {
        let what = called(track);
        let applied_mdb = applied(&track.sound, remedy);
        let db = |mdb: i32| format!("{:.1}", f64::from(mdb) / 1_000.0).replace('-', "\u{2212}");
        let label = match (remedy, applied_mdb) {
            (Remedy::LowerUnderDialogue { .. }, None) => format!("Lower {what} under the dialogue"),
            (Remedy::LowerUnderDialogue { .. }, Some(d)) => {
                format!(
                    "Lower {what} further under the dialogue (it ducks {} dB now)",
                    db(d)
                )
            }
            (Remedy::CutLowMiddle, None) => format!("Cut {what} around {CUT_HZ} Hz"),
            (Remedy::CutLowMiddle, Some(d)) => {
                format!(
                    "Cut {what} deeper around {CUT_HZ} Hz (cut {} dB there now)",
                    db(d)
                )
            }
            (Remedy::MakeRoomForVoice, None) => format!("Make room for the voice in {what}"),
            (Remedy::MakeRoomForVoice, Some(d)) => {
                format!("Make more room for the voice in {what} ({} dB there now)", db(d))
            }
        };
        Fix {
            sequence,
            track: track.id,
            remedy,
            label,
            applied_mdb,
        }
    }

    /// Whether pressing it changes anything: not when the cut or the
    /// ducking is already as deep as the fix goes.
    pub fn useful(sound: &TrackSound, remedy: Remedy) -> bool {
        match (remedy, applied(sound, remedy)) {
            (_, None) => true,
            (Remedy::LowerUnderDialogue { .. }, Some(d)) => d > DEEPEST_DUCK_MDB,
            (_, Some(d)) => d > DEEPEST_DIP_MDB,
        }
    }

    /// The track's sound with the fix in it.
    pub fn sound(&self, sound: &TrackSound) -> diag::Result<TrackSound> {
        let mut out = sound.clone();
        match self.remedy {
            Remedy::LowerUnderDialogue { threshold_mdb } => {
                let d = &mut out.ducking;
                d.depth_mdb = if d.on && d.depth_mdb <= DUCK_DEPTH_MDB {
                    (d.depth_mdb + DEEPER_MDB).max(DEEPEST_DUCK_MDB)
                } else {
                    DUCK_DEPTH_MDB
                };
                if !d.on {
                    let fresh = project::track_sound::Ducking::default();
                    d.hold_us = fresh.hold_us;
                    d.down_us = fresh.down_us;
                    d.up_us = fresh.up_us;
                    d.threshold_mdb = threshold_mdb;
                }
                d.on = true;
            }
            Remedy::CutLowMiddle => dip(&mut out, CUT_HZ, CUT_MDB)?,
            Remedy::MakeRoomForVoice => dip(&mut out, ROOM_HZ, ROOM_MDB)?,
        }
        out.check()?;
        Ok(out)
    }

    /// The undoable edit that makes the fix, on the project as it is now.
    pub fn apply(&self, project: &Project) -> diag::Result<Command> {
        let track = project
            .sequence(self.sequence)
            .and_then(|s| s.audio_tracks.iter().find(|t| t.id == self.track))
            .ok_or_else(gone)?;
        Ok(Command::SetTrackSound {
            sequence: self.sequence,
            track: self.track,
            sound: Box::new(self.sound(&track.sound)?),
        })
    }
}

/// The fix already on the track, as it is now (thousandths of a dB):
/// ducking that is on, or a dip of its EQ near the fix's frequency.
fn applied(sound: &TrackSound, remedy: Remedy) -> Option<i32> {
    let dip_near = |hz: u32| {
        sound.eq.on.then_some(())?;
        sound
            .eq
            .bands
            .iter()
            .filter(|b| b.on && b.shape == BandShape::Peak && b.gain_mdb < 0)
            .filter(|b| (b.freq_hz.max(1) as f32 / hz as f32).log2().abs() <= 0.5)
            .map(|b| b.gain_mdb)
            .min()
    };
    match remedy {
        Remedy::LowerUnderDialogue { .. } => sound.ducking.on.then_some(sound.ducking.depth_mdb),
        Remedy::CutLowMiddle => dip_near(CUT_HZ),
        Remedy::MakeRoomForVoice => dip_near(ROOM_HZ),
    }
}

/// Whether an EQ dip at `hz` fits in this sound's EQ.
pub fn dip_fits(sound: &TrackSound, hz: u32) -> bool {
    let mut s = sound.clone();
    dip(&mut s, hz, -1_000).is_ok()
}

/// Puts a dip of `gain_mdb` at `hz` (Q 1) into the EQ: see the module notes.
fn dip(sound: &mut TrackSound, hz: u32, gain_mdb: i32) -> diag::Result<()> {
    let eq = &mut sound.eq;
    if !eq.on {
        for b in &mut eq.bands {
            if b.gain_mdb != 0 {
                b.on = false;
            }
        }
        eq.on = true;
    }
    let octaves = |b: &EqBand| (b.freq_hz.max(1) as f32 / hz as f32).log2().abs();
    let peaks = || {
        eq.bands
            .iter()
            .enumerate()
            .filter(|(_, b)| b.shape == BandShape::Peak)
    };
    let near = peaks()
        .filter(|(_, b)| b.on && b.gain_mdb < 0 && octaves(b) <= 0.5)
        .min_by(|a, b| octaves(a.1).total_cmp(&octaves(b.1)))
        .map(|(i, _)| i);
    let flat = || {
        peaks()
            .filter(|(_, b)| b.gain_mdb == 0)
            .min_by(|a, b| octaves(a.1).total_cmp(&octaves(b.1)))
            .map(|(i, _)| i)
    };
    match near.or_else(flat) {
        Some(i) => {
            let b = &mut eq.bands[i];
            if b.on && b.gain_mdb < 0 {
                b.gain_mdb = (b.gain_mdb + gain_mdb).max(DEEPEST_DIP_MDB);
            } else {
                *b = EqBand {
                    on: true,
                    shape: BandShape::Peak,
                    freq_hz: hz,
                    q_milli: DIP_Q_MILLI,
                    gain_mdb,
                };
            }
            Ok(())
        }
        None => Err(AppError::new("That fix can't be used on this track")
            .why("Every band of its EQ is in use.")
            .fix("Set one of its EQ bands flat on the Audio page, then try the fix again.")),
    }
}

/// How a track is named in a sentence: "the music on A2", or "A2" for a
/// track without a role.
pub fn called(track: &project::Track) -> String {
    let name = &track.name;
    match track.sound.role {
        TrackRole::Music => format!("the music on {name}"),
        TrackRole::Effects => format!("the sound effects on {name}"),
        TrackRole::Ambience => format!("the background sound on {name}"),
        TrackRole::Dialogue => format!("the voice on {name}"),
        TrackRole::Other => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(remedy: Remedy) -> Fix {
        Fix {
            sequence: SequenceId(0),
            track: TrackId(0),
            remedy,
            label: String::new(),
            applied_mdb: None,
        }
    }

    fn track(sound: TrackSound) -> project::Track {
        let mut p = project::Project::new_automatic("T");
        let mut t = p.new_track(project::TrackKind::Audio, "Music");
        t.sound = sound;
        t
    }

    /// Once made, a fix says so and offers to go further; as far as it
    /// goes, it is no longer offered.
    #[test]
    fn a_fix_already_made_says_so_until_it_goes_no_further() {
        let mut sound = TrackSound::default();
        let first = Fix::new(SequenceId(0), &track(sound.clone()), Remedy::CutLowMiddle);
        assert_eq!(first.applied_mdb, None);
        assert_eq!(first.label, "Cut Music around 300 Hz");
        for _ in 0..2 {
            sound = first.sound(&sound).unwrap();
        }
        let again = Fix::new(SequenceId(0), &track(sound.clone()), Remedy::CutLowMiddle);
        assert_eq!(again.applied_mdb, Some(-7_000));
        assert!(
            again.label.contains("deeper") && again.label.contains("\u{2212}7.0 dB"),
            "{}",
            again.label
        );
        assert!(Fix::useful(&sound, Remedy::CutLowMiddle));
        sound = first.sound(&sound).unwrap();
        assert_eq!(applied(&sound, Remedy::CutLowMiddle), Some(DEEPEST_DIP_MDB));
        assert!(!Fix::useful(&sound, Remedy::CutLowMiddle), "as deep as it goes");
        assert!(
            Fix::useful(&sound, Remedy::MakeRoomForVoice),
            "another fix still is"
        );
    }

    #[test]
    fn a_cut_goes_into_a_flat_peak_band_and_deepens_when_used_again() {
        let plain = TrackSound::default();
        let once = fix(Remedy::CutLowMiddle).sound(&plain).unwrap();
        assert!(once.eq.on);
        let cut: Vec<_> = once.eq.bands.iter().filter(|b| b.gain_mdb != 0).collect();
        assert_eq!(cut.len(), 1);
        assert_eq!(
            (cut[0].freq_hz, cut[0].gain_mdb, cut[0].q_milli),
            (300, -3_500, 1_000)
        );
        let twice = fix(Remedy::CutLowMiddle).sound(&once).unwrap();
        let cut: Vec<_> = twice.eq.bands.iter().filter(|b| b.gain_mdb != 0).collect();
        assert_eq!((cut.len(), cut[0].gain_mdb), (1, -7_000));
        let room = fix(Remedy::MakeRoomForVoice).sound(&twice).unwrap();
        assert!(
            room.eq
                .bands
                .iter()
                .any(|b| b.freq_hz == 2_500 && b.gain_mdb == -2_500)
        );
        assert_eq!(room.eq.bands.iter().filter(|b| b.gain_mdb != 0).count(), 2);
    }

    #[test]
    fn bands_set_while_the_eq_was_off_stay_but_are_switched_off() {
        let mut sound = TrackSound::default();
        sound.eq.bands[1].gain_mdb = 6_000;
        let fixed = fix(Remedy::CutLowMiddle).sound(&sound).unwrap();
        assert!(fixed.eq.on);
        assert!(!fixed.eq.bands[1].on && fixed.eq.bands[1].gain_mdb == 6_000);
    }

    #[test]
    fn a_full_eq_is_refused_explained() {
        let mut sound = TrackSound::default();
        sound.eq.on = true;
        for b in &mut sound.eq.bands {
            b.gain_mdb = 2_000;
        }
        assert!(!dip_fits(&sound, CUT_HZ));
        assert!(fix(Remedy::CutLowMiddle).sound(&sound).is_err());
    }

    #[test]
    fn ducking_goes_on_then_deeper() {
        let lower = fix(Remedy::LowerUnderDialogue {
            threshold_mdb: -35_000,
        });
        let once = lower.sound(&TrackSound::default()).unwrap();
        assert!(once.ducking.on);
        assert_eq!(
            (once.ducking.depth_mdb, once.ducking.threshold_mdb),
            (-10_000, -35_000)
        );
        let twice = lower.sound(&once).unwrap();
        assert_eq!(twice.ducking.depth_mdb, -16_000);
    }
}
