use super::*;
use crate::command::Command;
use crate::model::{Fingerprint, MediaInfo, MediaRef, SequenceSettings};
use crate::{AudioStream, MediaId, VideoStream};

/// A 10 s recording (picture and one sound stream) placed twice: at 0 s
/// and at 10 s on V1 and A1.
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
            stated_length: None,
        },
    };
    let p = Project::new("T", SequenceSettings::GAME_CLIP);
    let sequence = p.active_sequence;
    let p = Command::Import { sequence, media }.apply(&p).unwrap();
    let id = *p.media.keys().next().unwrap();
    Command::PlaceMedia { sequence, media: id }.apply(&p).unwrap()
}

fn spans(track: &Track) -> Vec<(i64, i64)> {
    track
        .clips
        .iter()
        .map(|c| {
            (
                c.start.as_secs_f64().round() as i64,
                c.end().as_secs_f64().round() as i64,
            )
        })
        .collect()
}

fn paste(p: &Project, sequence: SequenceId, at: i64, clips: &CopiedClips) -> diag::Result<Project> {
    Command::PasteClips {
        sequence,
        at: Time::seconds(at),
        clips: clips.clone(),
    }
    .apply(p)
}

#[test]
fn a_clip_is_copied_with_its_sound_and_pasted_linked_on_the_same_tracks() {
    let p = project();
    let seq = p.active_sequence;
    let second = p.active().unwrap().video_tracks[0].clips[1].clone();
    let copied = CopiedClips::copy(&p, seq, &[second.id]).unwrap();
    assert_eq!(copied.len(), 2, "its linked sound came along");
    assert_eq!(copied.says(), "2 clips");
    let q = paste(&p, seq, 25, &copied).unwrap();
    let s = q.active().unwrap();
    assert_eq!(spans(&s.video_tracks[0]), vec![(0, 10), (10, 20), (25, 35)]);
    assert_eq!(spans(&s.audio_tracks[0]), vec![(0, 10), (10, 20), (25, 35)]);
    let (picture, sound) = (&s.video_tracks[0].clips[2], &s.audio_tracks[0].clips[2]);
    assert!(picture.link.is_some());
    assert_eq!(picture.link, sound.link, "the copies are linked to each other");
    assert_ne!(picture.link, second.link, "not to the originals");
    assert_ne!(picture.id, second.id);
    assert_eq!(picture.source_in, second.source_in);
}

#[test]
fn a_paste_replaces_what_it_lands_on_and_nothing_moves() {
    let p = project();
    let seq = p.active_sequence;
    let first = p.active().unwrap().video_tracks[0].clips[0].id;
    let copied = CopiedClips::copy(&p, seq, &[first]).unwrap();
    let q = paste(&p, seq, 5, &copied).unwrap();
    let s = q.active().unwrap();
    assert_eq!(spans(&s.video_tracks[0]), vec![(0, 5), (5, 15), (15, 20)]);
    assert_eq!(spans(&s.audio_tracks[0]), vec![(0, 5), (5, 15), (15, 20)]);
}

#[test]
fn clips_paste_into_another_timeline_making_the_tracks_they_need() {
    let p = project();
    let main = p.active_sequence;
    let p = Command::AddAdjustment {
        sequence: main,
        at: Time::seconds(2),
        length: Time::seconds(5),
        track: None,
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    let picked = [s.video_tracks[0].clips[0].id, s.video_tracks[1].clips[0].id];
    let copied = CopiedClips::copy(&p, main, &picked).unwrap();
    assert_eq!(copied.len(), 3);
    let p = Command::AddSequence {
        name: "Other".into(),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&p)
    .unwrap();
    let other = p.active_sequence;
    assert_ne!(other, main);
    let q = paste(&p, other, 0, &copied).unwrap();
    let s = q.sequence(other).unwrap();
    assert_eq!(s.video_tracks.len(), 2, "a second video track was made");
    assert_eq!(spans(&s.video_tracks[0]), vec![(0, 10)]);
    assert_eq!(spans(&s.video_tracks[1]), vec![(2, 7)]);
    assert!(s.video_tracks[1].clips[0].is_adjustment());
    assert_eq!(spans(&s.audio_tracks[0]), vec![(0, 10)]);
}

#[test]
fn a_nested_clip_never_pastes_into_its_own_timeline() {
    let p = project();
    let main = p.active_sequence;
    let p = Command::AddSequence {
        name: "Inner".into(),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&p)
    .unwrap();
    let inner = p.active_sequence;
    let first = p.sequence(main).unwrap().video_tracks[0].clips[0].id;
    let copied = CopiedClips::copy(&p, main, &[first]).unwrap();
    let p = paste(&p, inner, 0, &copied).unwrap();
    let p = Command::PlaceSequence {
        sequence: main,
        nested: inner,
        at: Some(Time::seconds(30)),
        track: None,
    }
    .apply(&p)
    .unwrap();
    let nested = p
        .sequence(main)
        .unwrap()
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.nested() == Some(inner))
        .unwrap()
        .id;
    let copied = CopiedClips::copy(&p, main, &[nested]).unwrap();
    assert_eq!(copied.left_out(&p, inner), copied.len());
    let err = paste(&p, inner, 20, &copied).unwrap_err();
    assert!(err.what().contains("Nothing copied"), "{}", err.report());
    // In the timeline it came from it pastes like any clip.
    assert_eq!(copied.left_out(&p, main), 0);
    let q = paste(&p, main, 60, &copied).unwrap();
    let count = |p: &Project| {
        p.sequence(main)
            .unwrap()
            .tracks()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.nested() == Some(inner))
            .count()
    };
    assert_eq!(count(&q), count(&p) * 2);
}

#[test]
fn nothing_selected_is_nothing_to_copy() {
    let p = project();
    assert!(CopiedClips::copy(&p, p.active_sequence, &[]).is_err());
}
