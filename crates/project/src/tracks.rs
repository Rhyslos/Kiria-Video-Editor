//! Tracks as a whole: adding one, removing one, renaming one and hiding a
//! video track. What happens to clips is in `command.rs` and `edits.rs`.

use crate::command::{missing, sequence_mut, track_mut};
use crate::ids::{SequenceId, TrackId};
use crate::model::{Project, Sequence, Track, TrackKind};
use diag::AppError;

/// The longest name a track can be given, in characters.
pub const LONGEST_NAME: usize = 40;

impl Sequence {
    /// The track with this id, video or audio.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks().find(|t| t.id == id)
    }

    /// The name a new track of `kind` gets: "V2" after one video track,
    /// "A3" after two audio tracks, and the next number when a track of
    /// that kind already has the name.
    pub fn next_track_name(&self, kind: TrackKind) -> String {
        let (letter, tracks) = match kind {
            TrackKind::Video => ('V', &self.video_tracks),
            TrackKind::Audio => ('A', &self.audio_tracks),
        };
        let mut number = tracks.len() + 1;
        loop {
            let name = format!("{letter}{number}");
            if !tracks.iter().any(|t| t.name == name) {
                return name;
            }
            number += 1;
        }
    }
}

/// A name as it is kept: without spaces around it. Refused when nothing is
/// left or it is too long for a track's header.
fn tidy(name: &str) -> diag::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::new("A track needs a name").fix("Type at least one letter."));
    }
    if name.chars().count() > LONGEST_NAME {
        return Err(AppError::new("That track name is too long")
            .why(format!(
                "A track's name can be {LONGEST_NAME} characters at most."
            ))
            .fix("Type a shorter name."));
    }
    Ok(name.to_owned())
}

/// Adds an empty track after the others of its kind: a video track on top
/// (it covers the ones before it), an audio track at the bottom.
pub(crate) fn add(p: &mut Project, sequence: SequenceId, kind: TrackKind, name: &str) -> diag::Result<()> {
    let track = p.new_track(kind, &tidy(name)?);
    let seq = sequence_mut(p, sequence)?;
    match kind {
        TrackKind::Video => seq.video_tracks.push_back(track),
        TrackKind::Audio => seq.audio_tracks.push_back(track),
    }
    Ok(())
}

/// Removes an empty track. A track that still has clips stays, and so does
/// the last track of its kind.
pub(crate) fn remove(p: &mut Project, sequence: SequenceId, track: TrackId) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let found = seq.track(track).ok_or_else(|| missing("track"))?;
    let (name, kind, clips) = (found.name.clone(), found.kind, found.clips.len());
    if clips > 0 {
        return Err(AppError::new(format!("{name} still has clips on it"))
            .why(format!(
                "Removing a track never removes clips: {name} has {clips} clip{}.",
                if clips == 1 { "" } else { "s" }
            ))
            .fix("Move its clips to another track or delete them, then remove the track."));
    }
    let tracks = match kind {
        TrackKind::Video => &mut seq.video_tracks,
        TrackKind::Audio => &mut seq.audio_tracks,
    };
    if tracks.len() <= 1 {
        let kind = match kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        };
        return Err(AppError::new(format!("{name} is the only {kind} track"))
            .why(format!("A timeline keeps at least one {kind} track."))
            .fix("Add another track of that kind first if this one should go."));
    }
    tracks.retain(|t| t.id != track);
    Ok(())
}

pub(crate) fn rename(p: &mut Project, sequence: SequenceId, track: TrackId, name: &str) -> diag::Result<()> {
    let name = tidy(name)?;
    let seq = sequence_mut(p, sequence)?;
    let track = track_mut(seq, track)?;
    track.name = name;
    Ok(())
}

/// Hides a video track (its clips are not seen in the picture) or shows it again.
pub(crate) fn set_hidden(
    p: &mut Project,
    sequence: SequenceId,
    track: TrackId,
    hidden: bool,
) -> diag::Result<()> {
    let track = track_mut(sequence_mut(p, sequence)?, track)?;
    if track.kind != TrackKind::Video {
        return Err(AppError::new("Only video tracks can be hidden")
            .why(format!("{} is an audio track.", track.name))
            .fix("Mute it instead: the M in its header."));
    }
    track.hidden = hidden;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::ids::{ClipId, MediaId};
    use crate::model::{
        Clip, Fingerprint, MediaInfo, MediaKind, MediaRef, Project, SequenceSettings, TrackKind,
    };
    use crate::time::Time;

    fn project() -> Project {
        Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: "rec.mkv".into(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: "x".into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(100),
                info: MediaInfo::default(),
            },
        }
        .apply(&Project::new("Test", SequenceSettings::GAME_CLIP))
        .unwrap()
    }

    fn add(p: &Project, kind: TrackKind) -> Project {
        let seq = p.active().unwrap();
        Command::AddTrack {
            sequence: seq.id,
            kind,
            name: seq.next_track_name(kind),
        }
        .apply(p)
        .unwrap()
    }

    fn names(p: &Project, kind: TrackKind) -> Vec<String> {
        let seq = p.active().unwrap();
        let tracks = match kind {
            TrackKind::Video => &seq.video_tracks,
            TrackKind::Audio => &seq.audio_tracks,
        };
        tracks.iter().map(|t| t.name.clone()).collect()
    }

    fn with_a_clip_on(p: &Project, track: crate::ids::TrackId) -> Project {
        Command::PlaceClip {
            sequence: p.active_sequence,
            track,
            clip: Clip {
                id: ClipId(0),
                media: *p.media.keys().next().unwrap(),
                stream: 0,
                start: Time::ZERO,
                source_in: Time::ZERO,
                duration: Time::seconds(5),
                link: None,
                speed: Default::default(),
                look: Default::default(),
                sound: Default::default(),
                keyframes: Default::default(),
                ramp: Default::default(),
                blend: Default::default(),
                kind: Default::default(),
            },
        }
        .apply(p)
        .unwrap()
    }

    #[test]
    fn new_tracks_go_after_the_others_and_take_the_next_number() {
        let p = project();
        let p = add(&add(&p, TrackKind::Video), TrackKind::Video);
        assert_eq!(
            names(&p, TrackKind::Video),
            ["V1", "V2", "V3"],
            "the first is the lowest layer"
        );
        let p = add(&p, TrackKind::Audio);
        assert_eq!(names(&p, TrackKind::Audio), ["A1", "A2"]);
        assert_eq!(
            names(&p, TrackKind::Video).len(),
            3,
            "the other kind is untouched"
        );
        let seq = p.active().unwrap();
        assert!(seq.video_tracks.iter().all(|t| t.kind == TrackKind::Video));
        assert!(
            seq.audio_tracks[1].clips.is_empty()
                && !seq.audio_tracks[1].muted
                && seq.audio_tracks[1].gain_db == 0.0
        );
    }

    #[test]
    fn a_new_track_never_takes_a_name_in_use() {
        let p = add(&add(&project(), TrackKind::Video), TrackKind::Video);
        // V2 goes: two tracks are left, named V1 and V3.
        let v2 = p.active().unwrap().video_tracks[1].id;
        let p = Command::RemoveTrack {
            sequence: p.active_sequence,
            track: v2,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(names(&p, TrackKind::Video), ["V1", "V3"]);
        assert_eq!(
            p.active().unwrap().next_track_name(TrackKind::Video),
            "V4",
            "not a second V3"
        );
        // Audio tracks named after their streams count too.
        let a1 = p.active().unwrap().audio_tracks[0].id;
        let p = Command::RenameTrack {
            sequence: p.active_sequence,
            track: a1,
            name: "A2".into(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.active().unwrap().next_track_name(TrackKind::Audio), "A3");
    }

    #[test]
    fn a_track_without_a_name_is_refused() {
        let p = project();
        let err = Command::AddTrack {
            sequence: p.active_sequence,
            kind: TrackKind::Video,
            name: "  ".into(),
        }
        .apply(&p)
        .unwrap_err();
        assert!(err.what().contains("needs a name"), "{}", err.what());
    }

    #[test]
    fn an_empty_track_is_removed_and_one_with_clips_is_refused() {
        let p = add(&project(), TrackKind::Video);
        let sequence = p.active_sequence;
        let v2 = p.active().unwrap().video_tracks[1].id;
        let gone = Command::RemoveTrack { sequence, track: v2 }.apply(&p).unwrap();
        assert_eq!(names(&gone, TrackKind::Video), ["V1"]);

        let used = with_a_clip_on(&p, v2);
        let err = Command::RemoveTrack { sequence, track: v2 }
            .apply(&used)
            .unwrap_err();
        assert!(err.what().contains("V2 still has clips"), "{}", err.what());
        assert!(err.report().contains("1 clip."), "{}", err.report());
        assert_eq!(names(&used, TrackKind::Video), ["V1", "V2"], "nothing changed");

        let err = Command::RemoveTrack {
            sequence,
            track: crate::ids::TrackId(9_999),
        }
        .apply(&p)
        .unwrap_err();
        assert!(err.what().contains("no longer exists"), "{}", err.what());
    }

    #[test]
    fn the_last_track_of_a_kind_stays() {
        let p = project();
        let seq = p.active().unwrap();
        for (track, kind) in [
            (seq.video_tracks[0].id, "video"),
            (seq.audio_tracks[0].id, "audio"),
        ] {
            let err = Command::RemoveTrack {
                sequence: seq.id,
                track,
            }
            .apply(&p)
            .unwrap_err();
            assert!(
                err.what().contains(&format!("the only {kind} track")),
                "{}",
                err.what()
            );
        }
        // With a second one, the first can go: the other takes its place.
        let p = add(&p, TrackKind::Audio);
        let seq = p.active().unwrap();
        let p = Command::RemoveTrack {
            sequence: seq.id,
            track: seq.audio_tracks[0].id,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(names(&p, TrackKind::Audio), ["A2"]);
    }

    #[test]
    fn a_track_is_renamed_and_the_name_is_tidied() {
        let p = project();
        let seq = p.active().unwrap();
        let (sequence, track) = (seq.id, seq.video_tracks[0].id);
        let rename = |name: &str| {
            Command::RenameTrack {
                sequence,
                track,
                name: name.into(),
            }
            .apply(&p)
        };
        let renamed = rename("  Game  ").unwrap();
        assert_eq!(names(&renamed, TrackKind::Video), ["Game"]);
        assert!(rename("").unwrap_err().what().contains("needs a name"));
        assert!(
            rename(&"x".repeat(super::LONGEST_NAME + 1))
                .unwrap_err()
                .what()
                .contains("too long")
        );
        assert!(
            rename(&"é".repeat(super::LONGEST_NAME)).is_ok(),
            "letters, not bytes"
        );
    }

    #[test]
    fn a_video_track_is_hidden_and_shown_and_an_audio_track_is_not() {
        let p = project();
        let seq = p.active().unwrap();
        let (sequence, v1, a1) = (seq.id, seq.video_tracks[0].id, seq.audio_tracks[0].id);
        let hide = |p: &Project, track, hidden| {
            Command::SetTrackHidden {
                sequence,
                track,
                hidden,
            }
            .apply(p)
        };
        let hidden = hide(&p, v1, true).unwrap();
        assert!(hidden.active().unwrap().video_tracks[0].hidden);
        let shown = hide(&hidden, v1, false).unwrap();
        assert_eq!(shown, p, "showing it again is all that changed back");
        let err = hide(&p, a1, true).unwrap_err();
        assert!(err.what().contains("Only video tracks"), "{}", err.what());
    }

    #[test]
    fn track_changes_are_undone_and_redone() {
        let mut history = crate::History::new(project());
        let sequence = history.current().active_sequence;
        let v1 = history.current().active().unwrap().video_tracks[0].id;
        history
            .apply(&Command::AddTrack {
                sequence,
                kind: TrackKind::Video,
                name: "V2".into(),
            })
            .unwrap();
        let v2 = history.current().active().unwrap().video_tracks[1].id;
        history
            .apply(&Command::SetTrackHidden {
                sequence,
                track: v2,
                hidden: true,
            })
            .unwrap();
        history
            .apply(&Command::RenameTrack {
                sequence,
                track: v1,
                name: "Game".into(),
            })
            .unwrap();
        history
            .apply(&Command::RemoveTrack { sequence, track: v2 })
            .unwrap();
        assert_eq!(names(history.current(), TrackKind::Video), ["Game"]);
        history.undo();
        let seq = history.current().active().unwrap().clone();
        assert_eq!(seq.video_tracks.len(), 2, "the track is back");
        assert!(
            seq.video_tracks[1].hidden && seq.video_tracks[1].id == v2,
            "as it was"
        );
        history.undo();
        assert_eq!(names(history.current(), TrackKind::Video), ["V1", "V2"]);
        history.undo();
        assert!(!history.current().active().unwrap().video_tracks[1].hidden);
        history.undo();
        assert_eq!(names(history.current(), TrackKind::Video), ["V1"]);
        for _ in 0..4 {
            history.redo();
        }
        assert_eq!(names(history.current(), TrackKind::Video), ["Game"]);
    }
}
