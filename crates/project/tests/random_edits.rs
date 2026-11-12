//! Random edit sessions: whatever is done, in whatever order, with any mix
//! of undo and redo, the project stays valid, and replaying the autosave
//! journal rebuilds exactly the same project.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use project::{
    AudioStream, Clip, ClipId, Command, Document, Fingerprint, LabelColor, LabelId, MediaId, MediaInfo,
    MediaKind, MediaRef, Project, Rate, Recovery, Region, SequenceSettings, Template, Time, TimelineLabel,
    TrackKind, VideoStream,
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
    RemoveTrack {
        video: bool,
        track: usize,
    },
    RenameTrack {
        video: bool,
        track: usize,
    },
    HideTrack {
        track: usize,
        hidden: bool,
    },
    MoveToTrack {
        pick: usize,
        video: bool,
        track: usize,
        by: i64,
    },
    Drop {
        at: i64,
        track: Option<usize>,
        insert: bool,
    },
    /// Lays the recording over a track: its sound finds a free place.
    Lay {
        at: i64,
        track: Option<usize>,
    },
    /// Moves a clip to a track, or to a new one when `track` is `None`:
    /// linked sound finds a free place.
    MoveBetween {
        pick: usize,
        video: bool,
        track: Option<usize>,
        by: i64,
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
    /// Saves the stretch between `a` and `b` (the whole timeline when they
    /// are equal) as a template and drops it in at `at`.
    Template {
        a: i64,
        b: i64,
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
        1 => (any::<bool>(), 0usize..4).prop_map(|(video, track)| Action::RemoveTrack { video, track }),
        1 => (any::<bool>(), 0usize..4).prop_map(|(video, track)| Action::RenameTrack { video, track }),
        1 => (0usize..4, any::<bool>()).prop_map(|(track, hidden)| Action::HideTrack { track, hidden }),
        3 => (any::<usize>(), any::<bool>(), 0usize..4, -40i64..40)
            .prop_map(|(pick, video, track, by)| Action::MoveToTrack { pick, video, track, by }),
        2 => (-5i64..120, prop::option::of(0usize..4), any::<bool>())
            .prop_map(|(at, track, insert)| Action::Drop { at, track, insert }),
        3 => (-5i64..120, prop::option::of(0usize..4)).prop_map(|(at, track)| Action::Lay { at, track }),
        3 => (any::<usize>(), any::<bool>(), prop::option::weighted(0.8, 0usize..4), -40i64..40)
            .prop_map(|(pick, video, track, by)| Action::MoveBetween { pick, video, track, by }),
        1 => t.clone().prop_map(|at| Action::AddLabel { at }),
        1 => any::<usize>().prop_map(|pick| Action::RemoveLabel { pick }),
        1 => (t.clone(), t.clone()).prop_map(|(a, b)| Action::Region { a, b }),
        1 => t.clone().prop_map(|at| Action::Needle { at }),
        2 => (t.clone(), t.clone(), t).prop_map(|(a, b, at)| Action::Template { a, b, at }),
        2 => Just(Action::Undo),
        1 => Just(Action::Redo),
    ]
}

fn command_for(action: &Action, p: &Project, media: MediaId, recording: MediaId) -> Option<Command> {
    let seq = p.active()?;
    let sequence = seq.id;
    let tracks = |video: bool| {
        if video {
            &seq.video_tracks
        } else {
            &seq.audio_tracks
        }
    };
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
            name: seq.next_track_name(if video { TrackKind::Video } else { TrackKind::Audio }),
        },
        // Often refused: the track has clips, or is the last of its kind.
        Action::RemoveTrack { video, track } => Command::RemoveTrack {
            sequence,
            track: tracks(video).get(track % tracks(video).len())?.id,
        },
        Action::RenameTrack { video, track } => Command::RenameTrack {
            sequence,
            track: tracks(video).get(track % tracks(video).len())?.id,
            name: format!("Renamed {track}"),
        },
        // Any track, so hiding an audio track is tried (and refused) too.
        Action::HideTrack { track, hidden } => {
            let all: Vec<_> = seq.tracks().map(|t| t.id).collect();
            Command::SetTrackHidden {
                sequence,
                track: *all.get(track % all.len())?,
                hidden,
            }
        }
        // To a track of either kind: the wrong kind is refused.
        Action::MoveToTrack {
            pick,
            video,
            track,
            by,
        } => Command::MoveToTrack {
            sequence,
            clip: pick_clip(pick)?,
            track: tracks(video).get(track % tracks(video).len())?.id,
            by: Time::seconds(by),
        },
        Action::Drop { at, track, insert } => {
            let all: Vec<_> = seq.tracks().map(|t| t.id).collect();
            Command::DropMedia {
                sequence,
                media: recording,
                at: Time::seconds(at),
                track: match track {
                    Some(track) => Some(*all.get(track % all.len())?),
                    None => None,
                },
                insert,
            }
        }
        // On any track: an audio track is refused.
        Action::Lay { at, track } => {
            let all: Vec<_> = seq.tracks().map(|t| t.id).collect();
            Command::LayMedia {
                sequence,
                media: recording,
                at: Time::seconds(at),
                track: match track {
                    Some(track) => Some(*all.get(track % all.len())?),
                    None => None,
                },
            }
        }
        Action::MoveBetween {
            pick,
            video,
            track,
            by,
        } => Command::MoveBetweenTracks {
            sequence,
            clip: pick_clip(pick)?,
            track: match track {
                Some(track) => Some(tracks(video).get(track % tracks(video).len())?.id),
                None => None,
            },
            by: Time::seconds(by),
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
        Action::Template { a, b, at } => {
            let range = Region::between(Time::seconds(a), Time::seconds(b));
            Command::InsertTemplate {
                sequence,
                // A stretch with no clips in it is nothing to save.
                template: Box::new(Template::capture(p, sequence, range).ok()?),
                at: Time::from_ratio(at * 2 + 1, 2)?,
            }
        }
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

/// A file with a picture and one sound stream, so dropping it places linked clips.
fn recording() -> MediaRef {
    MediaRef {
        path: "match.mkv".into(),
        duration: Time::seconds(20),
        info: MediaInfo {
            container: "matroska,webm".into(),
            video: Some(VideoStream {
                index: 0,
                codec: "hevc".into(),
                width: 1920,
                height: 1080,
                frame_rate: Rate::FPS_60,
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
        ..media()
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
        doc.apply(Command::AddMedia { media: recording() }).unwrap();
        let mut ids = doc.project().media.keys().copied();
        let (media_id, recording_id) = (ids.next().unwrap(), ids.next().unwrap());

        for action in &actions {
            match action {
                Action::Undo => { doc.undo().unwrap(); }
                Action::Redo => { doc.redo().unwrap(); }
                other => {
                    if let Some(command) = command_for(other, doc.project(), media_id, recording_id) {
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
