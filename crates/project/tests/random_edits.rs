//! Random edit sessions: whatever is done, in whatever order, with any mix
//! of undo and redo, the project stays valid, and replaying the autosave
//! journal rebuilds exactly the same project.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use project::{
    Clip, ClipId, Command, Document, Fingerprint, LabelColor, LabelId, MediaId, MediaKind, MediaRef, Project,
    Recovery, Region, SequenceSettings, Time, TimelineLabel, TrackKind,
};
use proptest::prelude::*;

/// A user action in terms of positions, turned into a real command against the current state.
#[derive(Debug, Clone)]
enum Action {
    Place {
        video: bool,
        track: usize,
        start: i64,
        len: i64,
    },
    Remove {
        pick: usize,
        ripple: bool,
    },
    Move {
        pick: usize,
        start: i64,
    },
    SplitSelected {
        pick: usize,
        at: i64,
    },
    SplitAll {
        at: i64,
    },
    AddTrack {
        video: bool,
    },
    AddLabel {
        at: i64,
    },
    RemoveLabel {
        pick: usize,
    },
    Region {
        a: i64,
        b: i64,
    },
    Needle {
        at: i64,
    },
    Undo,
    Redo,
}

fn action() -> impl Strategy<Value = Action> {
    let t = 0i64..120;
    prop_oneof![
        4 => (any::<bool>(), 0usize..3, t.clone(), 1i64..30)
            .prop_map(|(video, track, start, len)| Action::Place { video, track, start, len }),
        2 => (any::<usize>(), any::<bool>()).prop_map(|(pick, ripple)| Action::Remove { pick, ripple }),
        2 => (any::<usize>(), t.clone()).prop_map(|(pick, start)| Action::Move { pick, start }),
        2 => (any::<usize>(), t.clone()).prop_map(|(pick, at)| Action::SplitSelected { pick, at }),
        1 => t.clone().prop_map(|at| Action::SplitAll { at }),
        1 => any::<bool>().prop_map(|video| Action::AddTrack { video }),
        1 => t.clone().prop_map(|at| Action::AddLabel { at }),
        1 => any::<usize>().prop_map(|pick| Action::RemoveLabel { pick }),
        1 => (t.clone(), t.clone()).prop_map(|(a, b)| Action::Region { a, b }),
        1 => t.prop_map(|at| Action::Needle { at }),
        2 => Just(Action::Undo),
        1 => Just(Action::Redo),
    ]
}

fn command_for(action: &Action, p: &Project, media: MediaId) -> Option<Command> {
    let seq = p.active()?;
    let sequence = seq.id;
    let all_clips: Vec<ClipId> = seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
    let pick_clip = |pick: usize| (!all_clips.is_empty()).then(|| all_clips[pick % all_clips.len()]);
    Some(match *action {
        Action::Place {
            video,
            track,
            start,
            len,
        } => {
            let tracks = if video {
                &seq.video_tracks
            } else {
                &seq.audio_tracks
            };
            let track = tracks.get(track % tracks.len())?.id;
            Command::PlaceClip {
                sequence,
                track,
                clip: Clip {
                    id: ClipId(0),
                    media,
                    stream: 0,
                    start: Time::seconds(start),
                    source_in: Time::ZERO,
                    duration: Time::seconds(len),
                    link: None,
                    speed: Default::default(),
                    look: Default::default(),
                },
            }
        }
        Action::Remove { pick, ripple } => Command::RemoveClip {
            sequence,
            clip: pick_clip(pick)?,
            ripple,
        },
        Action::Move { pick, start } => Command::MoveClip {
            sequence,
            clip: pick_clip(pick)?,
            start: Time::seconds(start),
            track: None,
        },
        Action::SplitSelected { pick, at } => Command::Split {
            sequence,
            at: Time::from_ratio(at * 2 + 1, 2)?,
            clip: Some(pick_clip(pick)?),
        },
        Action::SplitAll { at } => Command::Split {
            sequence,
            at: Time::from_ratio(at * 3 + 1, 3)?,
            clip: None,
        },
        Action::AddTrack { video } => Command::AddTrack {
            sequence,
            kind: if video { TrackKind::Video } else { TrackKind::Audio },
            name: "Extra".into(),
        },
        Action::AddLabel { at } => Command::AddLabel {
            sequence,
            label: TimelineLabel {
                id: LabelId(0),
                name: "Label".into(),
                at: Time::seconds(at),
                color: LabelColor::Blue,
            },
        },
        Action::RemoveLabel { pick } => {
            let labels: Vec<_> = seq.labels.iter().map(|l| l.id).collect();
            Command::RemoveLabel {
                sequence,
                label: *labels.get(pick % labels.len().max(1))?,
            }
        }
        Action::Region { a, b } => Command::SetRegion {
            sequence,
            region: Region::between(Time::seconds(a), Time::seconds(b)),
        },
        Action::Needle { at } => Command::SetEditNeedle {
            sequence,
            at: Time::seconds(at),
        },
        Action::Undo | Action::Redo => return None,
    })
}

fn media() -> MediaRef {
    MediaRef {
        id: MediaId(0),
        path: "rec.mkv".into(),
        fingerprint: Fingerprint {
            size: 1,
            sample_hash: "x".into(),
        },
        kind: MediaKind::Video,
        duration: Time::seconds(600),
        info: Default::default(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_sessions_stay_valid_and_replay_exactly(actions in prop::collection::vec(action(), 1..60)) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("random.project");
        let mut doc = Document::create(&path, Project::new("Random", SequenceSettings::GAME_CLIP)).unwrap();
        doc.apply(Command::AddMedia { media: media() }).unwrap();
        let media_id = *doc.project().media.keys().next().unwrap();

        for action in &actions {
            match action {
                Action::Undo => { doc.undo().unwrap(); }
                Action::Redo => { doc.redo().unwrap(); }
                other => {
                    if let Some(command) = command_for(other, doc.project(), media_id) {
                        // Refused commands are fine; they must just change nothing.
                        let before = doc.project().clone();
                        if doc.apply(command).is_err() {
                            prop_assert_eq!(doc.project(), &before);
                        }
                    }
                }
            }
            let problems = doc.project().problems();
            prop_assert!(problems.is_empty(), "invalid after {:?}: {:?}", action, problems);
        }

        let expected = doc.project().clone();
        drop(doc); // crash without saving
        let recovered = Document::open(&path, Recovery::Recover).unwrap();
        prop_assert_eq!(recovered.project(), &expected);
    }
}
