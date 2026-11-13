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
    /// Removes two clips (maybe the same one, maybe linked) in one step.
    RemoveSeveral {
        a: usize,
        b: usize,
        ripple: bool,
    },
    /// Removes two clips and closes the gaps on every track.
    RippleSeveral {
        a: usize,
        b: usize,
    },
    /// Moves two clips together.
    MoveSeveral {
        a: usize,
        b: usize,
        by: i64,
    },
    /// Closes a stretch of every track.
    CloseGap {
        at: i64,
        len: i64,
    },
    /// Copies a clip (with its linked clips) and pastes it.
    PasteClip {
        pick: usize,
        at: i64,
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
    /// Sets a picture clip's opacity and place as seen at `at`, after a
    /// keyframe there for them (`keyed`), so they change over time.
    Animate {
        pick: usize,
        at: i64,
        value: i32,
        keyed: bool,
    },
    /// Moves (by `by` frames), removes or eases the keyframes of a picture
    /// clip's keyframe mark, and with `both` its next mark too.
    Marks {
        pick: usize,
        mark: usize,
        both: bool,
        change: u8,
        by: i64,
    },
    /// Copies a picture clip's keyframe mark (and with `both` its next one
    /// too) and pastes them on another picture clip (or the same) `at`
    /// frames into it: often refused, past the clip's end.
    Paste {
        from: usize,
        to: usize,
        mark: usize,
        both: bool,
        at: i64,
    },
    /// Gives a clip a slow-motion ramp starting `at` seconds into it (back
    /// to full speed afterwards with `back`), or with `clear` takes it away.
    Ramp {
        pick: usize,
        at: i64,
        back: bool,
        clear: bool,
    },
    /// Plays a clip at a quarter, half, normal or twice the speed.
    Speed {
        pick: usize,
        which: u8,
    },
    /// Turns frame blending on or off for a clip (refused for sound).
    Blend {
        pick: usize,
        on: bool,
    },
    /// Trims a clip's start (`ripple`: the rest follows) or slips it, by
    /// `by` frames: what a ramp has to keep its place through.
    Trim {
        pick: usize,
        by: i64,
        how: u8,
    },
    /// Adds a timeline, or copies the open one with `copy`.
    NewTimeline {
        copy: bool,
    },
    RenameTimeline {
        pick: usize,
    },
    RemoveTimeline {
        pick: usize,
    },
    /// Opens another timeline: not an edit, nothing in the journal.
    OpenTimeline {
        pick: usize,
    },
    /// Nests a clip, and with `with` a second one, into a new timeline.
    Nest {
        pick: usize,
        with: Option<usize>,
    },
    /// Places a timeline in the open one at `at` (at its end when `None`).
    PlaceTimeline {
        pick: usize,
        at: Option<i64>,
    },
    /// Lays an adjustment layer at `at` from the video track `track` up.
    Adjust {
        at: i64,
        track: usize,
    },
    /// Adds a file to the project's media without placing it.
    AddSpare,
    /// Points every file (with `all`), or one picked, at a new place in one step.
    Relink {
        all: bool,
        pick: usize,
    },
    /// Prunes every file no timeline uses (`all`), or one file picked from
    /// all of them (refused when a timeline uses it).
    Prune {
        all: bool,
        pick: usize,
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
        1 => (any::<usize>(), any::<usize>(), any::<bool>())
            .prop_map(|(a, b, ripple)| Action::RemoveSeveral { a, b, ripple }),
        1 => (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Action::RippleSeveral { a, b }),
        1 => (any::<usize>(), any::<usize>(), -30i64..30).prop_map(|(a, b, by)| Action::MoveSeveral { a, b, by }),
        1 => (t.clone(), 1i64..20).prop_map(|(at, len)| Action::CloseGap { at, len }),
        2 => (any::<usize>(), t.clone()).prop_map(|(pick, at)| Action::PasteClip { pick, at }),
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
        3 => (any::<usize>(), 0i64..120, 0i32..=1_000_000, any::<bool>())
            .prop_map(|(pick, at, value, keyed)| Action::Animate { pick, at, value, keyed }),
        3 => (any::<usize>(), any::<usize>(), any::<bool>(), 0u8..4, -600i64..600)
            .prop_map(|(pick, mark, both, change, by)| Action::Marks { pick, mark, both, change, by }),
        2 => (any::<usize>(), any::<usize>(), any::<usize>(), any::<bool>(), -30i64..900)
            .prop_map(|(from, to, mark, both, at)| Action::Paste { from, to, mark, both, at }),
        3 => (any::<usize>(), 0i64..20, any::<bool>(), prop::bool::weighted(0.2))
            .prop_map(|(pick, at, back, clear)| Action::Ramp { pick, at, back, clear }),
        1 => (any::<usize>(), 0u8..4).prop_map(|(pick, which)| Action::Speed { pick, which }),
        1 => (any::<usize>(), any::<bool>()).prop_map(|(pick, on)| Action::Blend { pick, on }),
        3 => (any::<usize>(), -300i64..300, 0u8..3).prop_map(|(pick, by, how)| Action::Trim { pick, by, how }),
        1 => any::<bool>().prop_map(|copy| Action::NewTimeline { copy }),
        1 => any::<usize>().prop_map(|pick| Action::RenameTimeline { pick }),
        1 => any::<usize>().prop_map(|pick| Action::RemoveTimeline { pick }),
        2 => any::<usize>().prop_map(|pick| Action::OpenTimeline { pick }),
        2 => (any::<usize>(), prop::option::of(any::<usize>())).prop_map(|(pick, with)| Action::Nest { pick, with }),
        2 => (any::<usize>(), prop::option::of(0i64..120)).prop_map(|(pick, at)| Action::PlaceTimeline { pick, at }),
        2 => (0i64..120, 0usize..4).prop_map(|(at, track)| Action::Adjust { at, track }),
        1 => Just(Action::AddSpare),
        1 => (any::<bool>(), any::<usize>()).prop_map(|(all, pick)| Action::Relink { all, pick }),
        1 => (any::<bool>(), any::<usize>()).prop_map(|(all, pick)| Action::Prune { all, pick }),
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
                    lines: Default::default(),
                    id: ClipId(0),
                    media,
                    stream: 0,
                    start: Time::seconds(start),
                    source_in: Time::ZERO,
                    duration: Time::seconds(len),
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
        }
        Action::NewTimeline { copy: false } => Command::AddSequence {
            name: p.free_sequence_name("Timeline"),
            settings: seq.settings,
        },
        Action::NewTimeline { copy: true } => Command::DuplicateSequence {
            sequence,
            name: p.free_sequence_name(&format!("{} copy", seq.name)),
        },
        Action::RenameTimeline { pick } => Command::RenameSequence {
            sequence: p.sequences[pick % p.sequences.len()].id,
            name: p.free_sequence_name("Renamed"),
        },
        Action::RemoveTimeline { pick } => Command::RemoveSequence {
            sequence: p.sequences[pick % p.sequences.len()].id,
        },
        Action::OpenTimeline { .. } | Action::Undo | Action::Redo => return None,
        Action::Nest { pick, with } => Command::NestClips {
            sequence,
            clips: [Some(pick), with]
                .into_iter()
                .flatten()
                .filter_map(pick_clip)
                .collect(),
            name: p.free_sequence_name("Nested"),
        },
        Action::PlaceTimeline { pick, at } => Command::PlaceSequence {
            sequence,
            nested: p.sequences[pick % p.sequences.len()].id,
            at: at.map(Time::seconds),
            track: None,
        },
        Action::Adjust { at, track } => Command::AddAdjustment {
            sequence,
            at: Time::seconds(at),
            length: project::timelines::ADJUSTMENT_LENGTH,
            track: seq.video_tracks.get(track).map(|t| t.id),
        },
        Action::AddSpare => Command::AddMedia {
            media: MediaRef {
                path: "spare.wav".into(),
                kind: MediaKind::Audio,
                ..crate::media()
            },
        },
        Action::Relink { all, pick } => {
            let ids: Vec<MediaId> = p.media.keys().copied().collect();
            let chosen: Vec<MediaId> = if all {
                ids
            } else {
                vec![*ids.get(pick % ids.len().max(1))?]
            };
            Command::RelinkFiles {
                files: chosen
                    .into_iter()
                    .map(|id| (id, format!("moved/{}-{pick}.mkv", id.0).into()))
                    .collect(),
            }
        }
        // The two files every other action uses are never pruned, or the
        // rest of the session would only be refusals.
        Action::Prune { all: true, .. } => Command::RemoveMedia {
            media: project::prune::unused_media(p)
                .into_iter()
                .filter(|id| *id != media && *id != recording)
                .collect(),
        },
        Action::Prune { all: false, pick } => {
            let ids: Vec<MediaId> = p.media.keys().copied().filter(|id| *id != media).collect();
            Command::RemoveMedia {
                media: vec![*ids.get(pick % ids.len().max(1))?],
            }
        }
        Action::Remove { pick, ripple } => Command::RemoveClip {
            sequence,
            clip: pick_clip(pick)?,
            ripple,
        },
        Action::RemoveSeveral { a, b, ripple } => Command::RemoveClips {
            sequence,
            clips: vec![pick_clip(a)?, pick_clip(b)?],
            ripple,
        },
        Action::RippleSeveral { a, b } => Command::RippleRemoveClips {
            sequence,
            clips: vec![pick_clip(a)?, pick_clip(b)?],
        },
        Action::MoveSeveral { a, b, by } => Command::MoveClips {
            sequence,
            clips: vec![pick_clip(a)?, pick_clip(b)?],
            by: Time::seconds(by),
        },
        Action::CloseGap { at, len } => Command::CloseGap {
            sequence,
            start: Time::seconds(at),
            end: Time::seconds(at + len),
        },
        Action::PasteClip { pick, at } => Command::PasteClips {
            sequence,
            at: Time::seconds(at),
            clips: project::CopiedClips::copy(p, sequence, &[pick_clip(pick)?]).ok()?,
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
        Action::Animate {
            pick,
            at,
            value,
            keyed,
        } => {
            let pictures: Vec<&Clip> = seq.video_tracks.iter().flat_map(|t| t.clips.iter()).collect();
            let clip = pictures.get(pick % pictures.len().max(1))?;
            let at = Time::from_ratio(at * 4 + 1, 4)?;
            if keyed {
                Command::Keyframe {
                    sequence,
                    clip: clip.id,
                    at,
                    fields: vec![
                        project::LookField::Opacity,
                        project::LookField::X,
                        project::LookField::Y,
                    ],
                    change: project::animate::KeyframeChange::Add,
                }
            } else {
                Command::SetLookAt {
                    sequence,
                    clip: clip.id,
                    at,
                    look: project::Look {
                        opacity: value,
                        x: value / 4,
                        ..clip.look_at(at)
                    },
                }
            }
        }
        Action::Marks {
            pick,
            mark,
            both,
            change,
            by,
        } => {
            let rate = seq.settings.frame_rate;
            let pictures: Vec<&Clip> = seq.video_tracks.iter().flat_map(|t| t.clips.iter()).collect();
            let clip = pictures.get(pick % pictures.len().max(1))?;
            let marks = clip.keyframe_frames(&project::LookField::ALL, rate);
            let first = mark % marks.len().max(1);
            let taken = if both { 2 } else { 1 };
            let frames: Vec<Time> = marks
                .iter()
                .skip(first)
                .take(taken)
                .map(|frame| rate.frame_start(*frame))
                .collect();
            Command::ChangeKeyframes {
                sequence,
                clip: clip.id,
                frames,
                // Mostly moves: they are what can go wrong.
                change: match change {
                    0 => project::animate::KeyframesChange::Remove,
                    1 => project::animate::KeyframesChange::Ease(project::Ease::Smooth),
                    _ => project::animate::KeyframesChange::Move(rate.frame_start(by)),
                },
            }
        }
        Action::Paste {
            from,
            to,
            mark,
            both,
            at,
        } => {
            let rate = seq.settings.frame_rate;
            let pictures: Vec<&Clip> = seq.video_tracks.iter().flat_map(|t| t.clips.iter()).collect();
            let source = pictures.get(from % pictures.len().max(1))?;
            let target = pictures.get(to % pictures.len().max(1))?;
            let marks = source.keyframe_frames(&project::LookField::ALL, rate);
            let frames: Vec<Time> = marks
                .iter()
                .skip(mark % marks.len().max(1))
                .take(if both { 2 } else { 1 })
                .map(|frame| rate.frame_start(*frame))
                .collect();
            Command::PasteKeyframes {
                sequence,
                clip: target.id,
                at: target.start + rate.frame_start(at),
                keyframes: project::CopiedKeyframes::copy(source, rate, &frames).ok()?,
            }
        }
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
                name_tracks: a % 2 == 0,
            }
        }
        Action::Ramp {
            pick,
            at,
            back,
            clear,
        } => Command::SetRamp {
            sequence,
            clip: pick_clip(pick)?,
            ramp: if clear {
                project::Ramp::default()
            } else {
                project::ramp::slow_motion(
                    Time::seconds(at),
                    project::Speed::NORMAL,
                    project::Speed::new(1, 4).ok()?,
                    Time::from_ratio(1, 2)?,
                    back.then_some(Time::seconds(1)),
                )
            },
        },
        Action::Speed { pick, which } => Command::SetSpeed {
            sequence,
            clip: pick_clip(pick)?,
            speed: [(1, 4), (1, 2), (1, 1), (2, 1)]
                .get(usize::from(which))
                .and_then(|(n, d)| project::Speed::new(*n, *d).ok())?,
        },
        Action::Blend { pick, on } => Command::SetFrameBlend {
            sequence,
            clip: pick_clip(pick)?,
            blend: if on {
                project::FrameBlend::Blend
            } else {
                project::FrameBlend::Off
            },
        },
        Action::Trim { pick, by, how } => {
            let id = pick_clip(pick)?;
            let clip = seq.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == id)?;
            let by = seq.settings.frame_rate.frame_start(by);
            match how {
                0 => Command::Trim {
                    sequence,
                    clip: id,
                    edge: project::Edge::Start,
                    to: clip.start + by,
                },
                1 => Command::RippleTrim {
                    sequence,
                    clip: id,
                    edge: project::Edge::Start,
                    to: clip.start + by,
                },
                _ => Command::Slip {
                    sequence,
                    clip: id,
                    by,
                },
            }
        }
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
            stated_length: None,
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
                Action::OpenTimeline { pick } => {
                    let sequences = &doc.project().sequences;
                    let id = sequences[pick % sequences.len()].id;
                    prop_assert!(doc.open_sequence(id));
                }
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
        // Which timeline is open is not in the journal (the editor keeps it
        // with the layout); everything else is replayed exactly.
        let mut replayed = recovered.project().clone();
        prop_assert!(replayed.sequence(expected.active_sequence).is_some());
        replayed.active_sequence = expected.active_sequence;
        prop_assert_eq!(&replayed, &expected);
    }
}

/// A step of a session that saves and crashes along the way.
#[derive(Debug, Clone)]
enum Step {
    Edit(Action),
    Undo,
    Redo,
    /// A full save on the spot (Ctrl+S, or the save on closing).
    Save,
    /// A save handed to a worker (the idle autosave, or Ctrl+S): its
    /// snapshot is taken now, it writes later.
    StartSave,
    /// One of the saves under way writes (`write`) or fails, and reports.
    FinishSave {
        pick: usize,
        write: bool,
    },
    /// The app stops dead. Each save under way had written or not
    /// (`written`, one per save, in the order they were started).
    Crash {
        written: Vec<bool>,
    },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => action().prop_map(Step::Edit),
        4 => Just(Step::Undo),
        2 => Just(Step::Redo),
        2 => Just(Step::Save),
        2 => Just(Step::StartSave),
        2 => (any::<usize>(), prop::bool::weighted(0.85)).prop_map(|(pick, write)| Step::FinishSave { pick, write }),
        1 => prop::collection::vec(any::<bool>(), 4).prop_map(|written| Step::Crash { written }),
    ]
}

/// The project as recovery must rebuild it: which timeline is open is not
/// in the journal (the editor keeps it with the layout), so it is set aside,
/// and media paths are compared as opening makes them: full, from `folder`.
fn same_but_open_timeline(recovered: &Project, expected: &Project, folder: &std::path::Path) -> Project {
    let mut recovered = recovered.clone();
    project::file::resolve_media_paths(&mut recovered, folder);
    assert!(recovered.sequence(expected.active_sequence).is_some());
    recovered.active_sequence = expected.active_sequence;
    recovered
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Edits, undos, redos and saves (on the spot, and on workers finishing
    /// in any order, or failing), with crashes at random points: recovery
    /// always rebuilds exactly the project at the crash (with the same undo
    /// and redo when there was unsaved work to recover), and the session
    /// goes on from there.
    #[test]
    fn a_crash_at_any_point_recovers_exactly_what_was_there(steps in prop::collection::vec(step(), 1..80)) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("random.project");
        let mut doc = Document::create(&path, Project::new("Random", SequenceSettings::GAME_CLIP)).unwrap();
        doc.apply(Command::AddMedia { media: media() }).unwrap();
        doc.apply(Command::AddMedia { media: recording() }).unwrap();
        let mut ids = doc.project().media.keys().copied();
        let (media_id, recording_id) = (ids.next().unwrap(), ids.next().unwrap());
        let mut saves: Vec<project::document::SaveJob> = Vec::new();

        for step in steps.iter().chain(std::iter::once(&Step::Crash { written: vec![false; 4] })) {
            match step {
                Step::Undo => { doc.undo().unwrap(); }
                Step::Redo => { doc.redo().unwrap(); }
                Step::Edit(Action::OpenTimeline { pick }) => {
                    let sequences = &doc.project().sequences;
                    let id = sequences[pick % sequences.len()].id;
                    prop_assert!(doc.open_sequence(id));
                }
                Step::Edit(action) => {
                    if let Some(command) = command_for(action, doc.project(), media_id, recording_id) {
                        let _ = doc.apply(command);
                    }
                }
                Step::Save => doc.save().unwrap(),
                Step::StartSave => saves.push(doc.prepare_save()),
                Step::FinishSave { pick, write } => {
                    if !saves.is_empty() {
                        let job = saves.remove(pick % saves.len());
                        let result = if *write {
                            job.run()
                        } else {
                            Err(diag::AppError::new("The disk is full"))
                        };
                        let failed = result.is_err();
                        prop_assert_eq!(doc.finish_save(job.journal_seq(), result).is_err(), failed);
                    }
                }
                Step::Crash { written } => {
                    for (job, write) in saves.drain(..).zip(written.iter().cycle()) {
                        if *write {
                            job.run().unwrap();
                        }
                    }
                    let mut expected = doc.project().clone();
                    // Opening a file makes its relative media paths full
                    // (from the project's folder); what was on screen did not.
                    project::file::resolve_media_paths(&mut expected, tmp.path());
                    let (could_undo, could_redo) = (doc.history().can_undo(), doc.history().can_redo());
                    drop(doc);
                    let unsaved = Document::check(&path).unwrap().unsaved_changes;
                    doc = Document::open(&path, Recovery::Recover).unwrap();
                    prop_assert_eq!(&same_but_open_timeline(doc.project(), &expected, tmp.path()), &expected);
                    // Recovered work comes back with its undo and redo; with
                    // nothing unsaved the project opens as saved, afresh.
                    if unsaved > 0 {
                        prop_assert_eq!(doc.history().can_undo(), could_undo);
                        prop_assert_eq!(doc.history().can_redo(), could_redo);
                    }
                    prop_assert_eq!(Document::check(&path).unwrap().unsaved_changes, 0, "recovered work was saved");
                    // The editor opens the timeline kept with the layout.
                    doc.open_sequence(expected.active_sequence);
                }
            }
        }
    }
}
