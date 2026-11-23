//! Recording a voice-over straight onto an audio track.
//!
//! Shift+R (or "Record a voice-over here" in an audio track's menu) opens
//! the microphone ([`engine::record`]); once it gives sound, a count-in
//! shows its level (Settings › Audio › Count-in), then the timeline plays
//! from the playhead and the take is kept from that moment, so it lines up
//! with what was heard. The track recorded on, and the earlier takes' tracks
//! under it, are muted while it plays, so they aren't heard over the new
//! take. Shift+R again, play/pause, Stop on the banner or the end of the
//! timeline stops it: the file is finished, checked like any import (on a
//! worker) and placed as one undo step (`Command::PlaceRecording`): on the
//! track where that stretch is free, else the first free one under it, else
//! a new track of the same name, so takes over the same stretch are all
//! kept, to choose from.
//!
//! Which track: the one asked for (the track's menu), else the audio target
//! track, else a track named "Voice-over", made (an ordinary undoable
//! "add track") when there is none. Takes are WAV files in a folder beside
//! the project ("<project> voice-overs"), numbered ("Voice-over 3.wav");
//! they are recordings like any other, never removed with the project.

use project::{Project, Sequence, SequenceId, Time, TrackId, TrackKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// The name of the track voice-overs go on when no track is chosen.
pub const TRACK_NAME: &str = "Voice-over";

/// Where a take goes, worked out when recording starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// This audio track.
    Track(TrackId),
    /// A track named [`TRACK_NAME`] is made first.
    Make,
}

/// The track a take goes on: `asked` (a track's menu), else the audio
/// target, else the track named [`TRACK_NAME`], else one to make. A track
/// asked for or targeted that is gone, or isn't an audio track, counts as
/// not given.
pub fn pick(seq: &Sequence, asked: Option<TrackId>, target: Option<u64>) -> Pick {
    let audio = |id: TrackId| seq.audio_tracks.iter().any(|t| t.id == id);
    asked
        .filter(|id| audio(*id))
        .or_else(|| target.map(TrackId).filter(|id| audio(*id)))
        .or_else(|| {
            seq.audio_tracks
                .iter()
                .find(|t| t.name == TRACK_NAME)
                .map(|t| t.id)
        })
        .map_or(Pick::Make, Pick::Track)
}

/// The track named [`TRACK_NAME`] that was just made: the last audio
/// track with that name.
pub fn made_track(seq: &Sequence) -> Option<TrackId> {
    seq.audio_tracks
        .iter()
        .rev()
        .find(|t| t.kind == TrackKind::Audio && t.name == TRACK_NAME)
        .map(|t| t.id)
}

/// The folder takes of the project at `project_file` are kept in: beside
/// it, named after it.
pub fn folder(project_file: &Path) -> PathBuf {
    let stem = project_file
        .file_stem()
        .map_or_else(|| "Project".to_owned(), |s| s.to_string_lossy().into_owned());
    let parent = project_file.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem} voice-overs"))
}

/// A take's file in `folder`: "Voice-over 1.wav", then the first number
/// not taken, so a take never replaces another.
pub fn take_path(folder: &Path) -> PathBuf {
    (1u32..)
        .map(|n| folder.join(format!("Voice-over {n}.wav")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| folder.join("Voice-over.wav"))
}

/// The project as playback should hear it while recording on `track`: that
/// track muted, and the tracks of the same name right under it (where
/// earlier takes over the same stretch went), so earlier takes aren't heard
/// over the new one. `None` when they are all muted already.
pub fn muted_while_recording(project: &Project, sequence: SequenceId, track: TrackId) -> Option<Project> {
    let seq = project.sequences.iter().find(|s| s.id == sequence)?;
    let index = seq.audio_tracks.iter().position(|t| t.id == track)?;
    let name = &seq.audio_tracks[index].name;
    let takes = std::iter::once(&seq.audio_tracks[index]).chain(
        seq.audio_tracks
            .iter()
            .skip(index + 1)
            .take_while(|t| t.name == *name),
    );
    let mutes: Vec<project::Command> = takes
        .filter(|t| !t.muted)
        .map(|t| project::Command::SetTrackMix {
            sequence,
            track: t.id,
            muted: true,
            solo: t.solo,
            gain_db: t.gain_db,
        })
        .collect();
    if mutes.is_empty() {
        return None;
    }
    project::Command::Steps(mutes).apply(project).ok()
}

/// The microphones the system offers, asked for once on a thread of its
/// own (the system can take a while to answer).
#[derive(Default)]
pub struct Microphones {
    asked: Option<Receiver<Vec<String>>>,
    names: Option<Vec<String>>,
}

impl Microphones {
    /// The names, once the system has answered; the first call asks.
    pub fn names(&mut self, ctx: &egui::Context) -> Option<&[String]> {
        if self.names.is_none() {
            match &self.asked {
                None => {
                    let (tx, rx) = mpsc::channel();
                    let ctx = ctx.clone();
                    let spawned = std::thread::Builder::new()
                        .name("microphones".into())
                        .spawn(move || {
                            let _ = tx.send(engine::record::microphones());
                            ctx.request_repaint();
                        });
                    match spawned {
                        Ok(_) => self.asked = Some(rx),
                        Err(e) => {
                            tracing::warn!("can't ask for the microphones: {e}");
                            self.names = Some(Vec::new());
                        }
                    }
                }
                Some(rx) => {
                    if let Ok(names) = rx.try_recv() {
                        self.names = Some(names);
                        self.asked = None;
                    }
                }
            }
        }
        self.names.as_deref()
    }
}

/// What a take is waiting for.
pub enum Phase {
    /// The microphone is opening; the timeline hasn't started.
    Opening,
    /// The microphone gives sound: counting in until then (its level shows).
    Counting(std::time::Instant),
    /// The timeline plays and the take is being kept.
    Rolling,
    /// Stopped: the file is being finished.
    Finishing,
    /// The finished file is being checked, as an import is.
    Checking(Receiver<Result<project::MediaRef, media::Refused>>),
}

/// A take being made.
pub struct Take {
    pub recording: engine::record::Recording,
    pub sequence: SequenceId,
    pub track: TrackId,
    /// Where on the timeline it starts.
    pub at: Time,
    pub phase: Phase,
    /// The track's name, for what the banner and status line say.
    pub track_name: String,
}

impl Take {
    /// Checks the finished file on a thread of its own (reading a WAV
    /// file's head takes a moment; never on the interface thread).
    pub fn check(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("voice-over check".into())
            .spawn(move || {
                let _ = tx.send(media::check(&path));
            });
        if let Err(e) = spawned {
            tracing::warn!("the voice-over check won't start: {e}");
        }
        self.phase = Phase::Checking(rx);
    }
}

/// The length of a take as the banner writes it: "0:07", "12:03".
pub fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(seq: &Sequence, name: &str) -> TrackId {
        seq.audio_tracks.iter().find(|t| t.name == name).unwrap().id
    }

    fn with_tracks(names: &[&str]) -> Project {
        let mut p = Project::new_automatic("T");
        let sequence = p.active_sequence;
        for name in names {
            p = project::Command::AddTrack {
                sequence,
                kind: TrackKind::Audio,
                name: (*name).into(),
            }
            .apply(&p)
            .unwrap();
        }
        p
    }

    #[test]
    fn a_take_goes_on_the_track_asked_for_then_the_target_then_the_voice_over_track() {
        let p = with_tracks(&["Mic", TRACK_NAME, "Game"]);
        let seq = p.active().unwrap();
        let [a1, voice, a3] = ["Mic", TRACK_NAME, "Game"].map(|n| named(seq, n));
        assert_eq!(pick(seq, Some(a3), Some(a1.0)), Pick::Track(a3));
        assert_eq!(pick(seq, None, Some(a1.0)), Pick::Track(a1));
        assert_eq!(pick(seq, None, None), Pick::Track(voice));
        // Gone, or a video track: as if not given.
        assert_eq!(pick(seq, Some(TrackId(999)), Some(999)), Pick::Track(voice));
        let empty = with_tracks(&["Mic"]);
        assert_eq!(pick(empty.active().unwrap(), None, None), Pick::Make);
    }

    #[test]
    fn the_track_made_for_takes_is_found_again() {
        let p = with_tracks(&["Mic", TRACK_NAME]);
        let seq = p.active().unwrap();
        assert_eq!(made_track(seq), Some(named(seq, TRACK_NAME)));
        assert_eq!(made_track(with_tracks(&["Mic"]).active().unwrap()), None);
    }

    #[test]
    fn takes_are_kept_beside_the_project_and_never_replace_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join(format!("Ranked match.{}", brand::PROJECT_FILE_EXTENSION));
        let folder = folder(&file);
        assert_eq!(folder, dir.path().join("Ranked match voice-overs"));
        std::fs::create_dir_all(&folder).unwrap();
        let first = take_path(&folder);
        assert_eq!(first, folder.join("Voice-over 1.wav"));
        std::fs::write(&first, b"").unwrap();
        assert_eq!(take_path(&folder), folder.join("Voice-over 2.wav"));
    }

    #[test]
    fn the_track_recorded_on_and_its_earlier_takes_are_muted_for_playback_only() {
        // A track of the Voice-over's name under it holds an earlier take.
        let p = with_tracks(&[TRACK_NAME, TRACK_NAME, "Game"]);
        let seq = p.active().unwrap();
        let first = seq
            .audio_tracks
            .iter()
            .position(|t| t.name == TRACK_NAME)
            .unwrap();
        let track = seq.audio_tracks[first].id;
        let heard = muted_while_recording(&p, seq.id, track).unwrap();
        let muted: Vec<(&str, bool)> = heard
            .active()
            .unwrap()
            .audio_tracks
            .iter()
            .map(|t| (t.name.as_str(), t.muted))
            .collect();
        assert_eq!(
            &muted[first..],
            [(TRACK_NAME, true), (TRACK_NAME, true), ("Game", false)]
        );
        assert!(
            p.active().unwrap().audio_tracks.iter().all(|t| !t.muted),
            "the project itself is untouched"
        );
        assert!(
            muted_while_recording(&heard, seq.id, track).is_none(),
            "muted already"
        );
    }

    #[test]
    fn a_take_length_is_minutes_and_seconds() {
        assert_eq!(clock(7.9), "0:07");
        assert_eq!(clock(723.0), "12:03");
        assert_eq!(clock(-1.0), "0:00");
    }
}
