use super::*;
use crate::command::Command;
use crate::model::{Fingerprint, MediaInfo, MediaKind, MediaRef};
use crate::{AudioStream, History, VideoStream};

/// A project with a 10 s recording (picture and one sound stream) imported
/// twice: at 0 s and at 10 s on V1 and A1.
fn project() -> Project {
    let media = MediaRef {
        id: MediaId(0),
        path: "rec.mkv".into(),
        fingerprint: Fingerprint {
            size: 1,
            sample_hash: "x".into(),
        },
        kind: MediaKind::Video,
        duration: Time::seconds(10),
        info: MediaInfo {
            container: String::new(),
            video: Some(VideoStream {
                index: 0,
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                frame_rate: crate::Rate::FPS_60,
                variable_frame_rate: false,
                rotation: 0,
            }),
            audio: vec![AudioStream {
                index: 1,
                codec: "aac".into(),
                channels: 2,
                sample_rate: 48_000,
                name: "Game".into(),
            }],
        },
    };
    let p = Project::new("T", SequenceSettings::GAME_CLIP);
    let sequence = p.active_sequence;
    let p = Command::Import {
        sequence,
        media: media.clone(),
    }
    .apply(&p)
    .unwrap();
    let id = *p.media.keys().next().unwrap();
    Command::PlaceMedia { sequence, media: id }.apply(&p).unwrap()
}

fn main(p: &Project) -> SequenceId {
    p.sequences[0].id
}

fn first_clip(p: &Project, sequence: SequenceId, video: bool) -> Clip {
    let seq = p.sequence(sequence).unwrap();
    let tracks = if video {
        &seq.video_tracks
    } else {
        &seq.audio_tracks
    };
    tracks[0].clips[0].clone()
}

#[test]
fn timelines_are_added_renamed_copied_and_removed() {
    let p = project();
    let first = main(&p);
    let added = Command::AddSequence {
        name: "Short cut".into(),
        settings: SequenceSettings::VERTICAL_SHORT,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(added.sequences.len(), 2);
    let short = added.active_sequence;
    assert_ne!(short, first, "the new timeline is opened");
    assert_eq!(
        added.sequence(short).unwrap().settings,
        SequenceSettings::VERTICAL_SHORT
    );

    // Names are unique whatever their capitals.
    let err = Command::RenameSequence {
        sequence: short,
        name: " main SEQUENCE ".into(),
    }
    .apply(&added)
    .unwrap_err();
    assert!(err.what().contains("already a timeline"), "{}", err.what());
    let renamed = Command::RenameSequence {
        sequence: short,
        name: " Vertical ".into(),
    }
    .apply(&added)
    .unwrap();
    assert_eq!(renamed.sequence(short).unwrap().name, "Vertical");

    let copied = Command::DuplicateSequence {
        sequence: first,
        name: renamed.free_sequence_name("Main sequence"),
    }
    .apply(&renamed)
    .unwrap();
    let copy = copied.active().unwrap();
    assert_eq!(copy.name, "Main sequence 2");
    let original = copied.sequence(first).unwrap();
    assert_eq!(copy.length(), original.length());
    let ids =
        |s: &Sequence| -> Vec<ClipId> { s.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect() };
    assert!(
        ids(copy).iter().all(|id| !ids(original).contains(id)),
        "new clip ids"
    );
    // Linked in the original, linked in the copy, under a new link.
    let (picture, sound) = (&copy.video_tracks[0].clips[0], &copy.audio_tracks[0].clips[0]);
    assert_eq!(picture.link, sound.link);
    assert_ne!(picture.link, original.video_tracks[0].clips[0].link);

    let removed = Command::RemoveSequence { sequence: copy.id }
        .apply(&copied)
        .unwrap();
    assert_eq!(removed.sequences.len(), 2);
    assert_eq!(removed.active_sequence, first, "the first one left opens");
}

#[test]
fn the_last_timeline_stays() {
    let p = project();
    let err = Command::RemoveSequence { sequence: main(&p) }
        .apply(&p)
        .unwrap_err();
    assert!(err.what().contains("only timeline"), "{}", err.what());
}

#[test]
fn nesting_moves_clips_into_a_timeline_of_their_own() {
    let p = project();
    let outer = main(&p);
    let second = p.sequence(outer).unwrap().video_tracks[0].clips[1].id;
    let nested = Command::NestClips {
        sequence: outer,
        clips: vec![second],
        name: "Highlight".into(),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(nested.active_sequence, outer, "the outer timeline stays open");
    let seq = nested.sequence(outer).unwrap();
    let inner = nested.sequences.iter().find(|s| s.name == "Highlight").unwrap();
    // The picture and its linked sound went in, from the inner timeline's start.
    assert_eq!(inner.video_tracks[0].clips.len(), 1);
    assert_eq!(inner.audio_tracks[0].clips.len(), 1);
    assert_eq!(inner.video_tracks[0].clips[0].start, Time::ZERO);
    assert_eq!(inner.settings, seq.settings);
    // One clip of each kind plays it in their place, linked.
    let (picture, sound) = (&seq.video_tracks[0].clips[1], &seq.audio_tracks[0].clips[1]);
    assert_eq!(picture.kind, ClipKind::Timeline(inner.id));
    assert_eq!(sound.kind, ClipKind::Timeline(inner.id));
    assert_eq!(
        (picture.start, picture.duration),
        (Time::seconds(10), Time::seconds(10))
    );
    assert!(picture.link.is_some() && picture.link == sound.link);
    assert_eq!(picture.media, MediaId::NONE);
    assert_eq!(seq.length(), Time::seconds(20), "nothing else moved");
}

#[test]
fn nesting_is_refused_around_a_clip_left_out() {
    let mut p = project();
    let outer = main(&p);
    // Three clips on V1 (0, 10, 20 s); nest the first and the last only.
    p = Command::PlaceMedia {
        sequence: outer,
        media: *p.media.keys().next().unwrap(),
    }
    .apply(&p)
    .unwrap();
    let v1 = &p.sequence(outer).unwrap().video_tracks[0];
    let (a, c) = (v1.clips[0].id, v1.clips[2].id);
    let err = Command::NestClips {
        sequence: outer,
        clips: vec![a, c],
        name: "Gap".into(),
    }
    .apply(&p)
    .unwrap_err();
    assert!(
        err.why_text().unwrap_or_default().contains("isn't selected"),
        "{:?}",
        err.why_text()
    );
}

#[test]
fn a_nested_clip_trims_within_its_timeline() {
    let p = project();
    let outer = main(&p);
    let second = p.sequence(outer).unwrap().video_tracks[0].clips[1].id;
    let p = Command::NestClips {
        sequence: outer,
        clips: vec![second],
        name: "N".into(),
    }
    .apply(&p)
    .unwrap();
    let nested = p.sequence(outer).unwrap().video_tracks[0].clips[1].clone();
    let longer = Command::Trim {
        sequence: outer,
        clip: nested.id,
        edge: crate::Edge::End,
        to: Time::seconds(21),
    }
    .apply(&p)
    .unwrap_err();
    assert!(longer.why_text().unwrap_or_default().contains("ends before"));
    let shorter = Command::Trim {
        sequence: outer,
        clip: nested.id,
        edge: crate::Edge::Start,
        to: Time::seconds(12),
    }
    .apply(&p)
    .unwrap();
    let trimmed = first_clip(&shorter, outer, true);
    assert_eq!(trimmed.start, Time::ZERO, "the first clip is untouched");
    let trimmed = &shorter.sequence(outer).unwrap().video_tracks[0].clips[1];
    assert_eq!(
        (trimmed.start, trimmed.source_in),
        (Time::seconds(12), Time::seconds(2))
    );
}

#[test]
fn a_timeline_is_placed_in_another_but_never_in_itself() {
    let p = project();
    let outer = main(&p);
    let p = Command::AddSequence {
        name: "Second".into(),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&p)
    .unwrap();
    let second = p.active_sequence;
    // The main timeline (20 s of picture and sound) placed in the second.
    let placed = Command::PlaceSequence {
        sequence: second,
        nested: outer,
        at: None,
        track: None,
    }
    .apply(&p)
    .unwrap();
    let seq = placed.sequence(second).unwrap();
    assert_eq!(seq.length(), Time::seconds(20));
    assert_eq!(first_clip(&placed, second, true).kind, ClipKind::Timeline(outer));
    assert_eq!(first_clip(&placed, second, false).kind, ClipKind::Timeline(outer));
    assert!(placed.plays(second, outer) && !placed.plays(outer, second));

    // Not in itself, and not in what it plays.
    for (sequence, nested) in [(second, second), (outer, second)] {
        let err = Command::PlaceSequence {
            sequence,
            nested,
            at: None,
            track: None,
        }
        .apply(&placed)
        .unwrap_err();
        assert!(err.what().contains("can't be placed"), "{}", err.what());
    }
    // An empty timeline has nothing to play.
    let third = Command::AddSequence {
        name: "Empty".into(),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&placed)
    .unwrap();
    let empty = third.active_sequence;
    assert!(
        Command::PlaceSequence {
            sequence: outer,
            nested: empty,
            at: None,
            track: None,
        }
        .apply(&third)
        .is_err()
    );
    // A timeline played elsewhere can't be removed.
    let err = Command::RemoveSequence { sequence: outer }
        .apply(&placed)
        .unwrap_err();
    assert!(err.what().contains("used in another timeline"), "{}", err.what());
}

#[test]
fn a_loop_through_nested_timelines_is_a_problem() {
    let p = project();
    let outer = main(&p);
    let p = Command::AddSequence {
        name: "Second".into(),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&p)
    .unwrap();
    let second = p.active_sequence;
    let mut p = Command::PlaceSequence {
        sequence: second,
        nested: outer,
        at: None,
        track: None,
    }
    .apply(&p)
    .unwrap();
    // Made by hand, as a damaged file could have it: the main timeline plays the second.
    let mut clip = first_clip(&p, second, true);
    clip.kind = ClipKind::Timeline(second);
    clip.start = Time::seconds(40);
    p.sequences[0].video_tracks[0].clips.push_back(clip);
    assert!(
        p.problems()
            .iter()
            .any(|problem| problem.contains("plays itself")),
        "{:?}",
        p.problems()
    );
    // And one that plays a timeline the project doesn't have.
    let mut p = project();
    let mut clip = first_clip(&p, outer, true);
    clip.kind = ClipKind::Timeline(SequenceId(999));
    clip.media = MediaId::NONE;
    clip.start = Time::seconds(40);
    p.sequences[0].video_tracks[0].clips.push_back(clip);
    assert!(
        p.problems()
            .iter()
            .any(|problem| problem.contains("missing timeline"))
    );
}

#[test]
fn an_adjustment_layer_goes_above_what_it_changes() {
    let p = project();
    let outer = main(&p);
    let v1 = p.sequence(outer).unwrap().video_tracks[0].id;
    // V1 has a clip at 2 s: the layer goes on a new track above it.
    let p = Command::AddAdjustment {
        sequence: outer,
        at: Time::seconds(2),
        length: ADJUSTMENT_LENGTH,
        track: Some(v1),
    }
    .apply(&p)
    .unwrap();
    let seq = p.sequence(outer).unwrap();
    assert_eq!(seq.video_tracks.len(), 2);
    assert_eq!(seq.video_tracks[1].name, "V2");
    let layer = &seq.video_tracks[1].clips[0];
    assert!(layer.is_adjustment());
    assert_eq!(
        (layer.start, layer.duration),
        (Time::seconds(2), Time::seconds(5))
    );
    // A second one over the same stretch finds no room on V2 either.
    let p = Command::AddAdjustment {
        sequence: outer,
        at: Time::seconds(4),
        length: ADJUSTMENT_LENGTH,
        track: Some(v1),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(p.sequence(outer).unwrap().video_tracks.len(), 3);
    // Not on an audio track.
    let a1 = p.sequence(outer).unwrap().audio_tracks[0].id;
    assert!(
        Command::AddAdjustment {
            sequence: outer,
            at: Time::seconds(30),
            length: ADJUSTMENT_LENGTH,
            track: Some(a1),
        }
        .apply(&p)
        .is_err()
    );
    // It can be stretched back as far as wanted: it plays nothing.
    let layer = p.sequence(outer).unwrap().video_tracks[1].clips[0].id;
    let p = Command::Trim {
        sequence: outer,
        clip: layer,
        edge: crate::Edge::Start,
        to: Time::ZERO,
    }
    .apply(&p)
    .unwrap();
    let layer = &p.sequence(outer).unwrap().video_tracks[1].clips[0];
    assert_eq!((layer.start, layer.duration), (Time::ZERO, Time::seconds(7)));
}

#[test]
fn switching_timelines_is_not_an_undo_step() {
    let mut history = History::new(project());
    let first = history.current().active_sequence;
    history
        .apply(&Command::AddSequence {
            name: "Second".into(),
            settings: SequenceSettings::GAME_CLIP,
        })
        .unwrap();
    let second = history.current().active_sequence;
    history
        .apply(&Command::RenameProject {
            name: "Renamed".into(),
        })
        .unwrap();
    assert!(history.open(first));
    assert!(!history.open(SequenceId(999)));
    // Undoing the rename keeps the first timeline open.
    assert!(history.undo());
    assert_eq!(history.current().active_sequence, first);
    // Undoing the new timeline: it is gone; the first stays open.
    history.open(second);
    assert!(history.undo());
    assert_eq!(history.current().active_sequence, first);
    assert!(history.current().sequence(second).is_none());
}
