use super::*;
use crate::command::Command;
use crate::command::tests::{
    clip, clips, first_video, fixture, gap_then_streams, imported, many_linked, place, recording, spans,
};
use crate::time::Rate;

#[test]
fn overlapping_placement_is_refused_and_nothing_changes() {
    let f = fixture();
    let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
    let err = place(&f, &p, f.v1, clip(&f, 5, 10, None)).unwrap_err();
    assert!(err.what().contains("already a clip"));
    assert_eq!(clips(&p, 0, false), vec![(0, 10)]);
}

#[test]
fn delete_leaves_a_gap_and_ripple_delete_closes_it() {
    let f = fixture();
    let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 20, 5, None)).unwrap();
    let middle = p.active().unwrap().video_tracks[0].clips[1].id;
    let gap = Command::RemoveClip {
        sequence: f.seq,
        clip: middle,
        ripple: false,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(clips(&gap, 0, false), vec![(0, 10), (20, 5)]);
    let closed = Command::RemoveClip {
        sequence: f.seq,
        clip: middle,
        ripple: true,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(clips(&closed, 0, false), vec![(0, 10), (15, 5)]);
}

#[test]
fn removing_several_clips_is_one_step_and_matches_removing_them_one_by_one() {
    let mut f = fixture();
    let link = Some(f.p.allocate_id());
    let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, link)).unwrap();
    let p = place(&f, &p, f.a1, clip(&f, 0, 10, link)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 20, 5, None)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 30, 5, None)).unwrap();
    let v = &p.active().unwrap().video_tracks[0];
    // The linked pair's audio goes with its picture, even when both are picked.
    let a = p.active().unwrap().audio_tracks[0].clips[0].id;
    let picked = vec![v.clips[0].id, a, v.clips[2].id];
    for ripple in [false, true] {
        let together = Command::RemoveClips {
            sequence: f.seq,
            clips: picked.clone(),
            ripple,
        }
        .apply(&p)
        .unwrap();
        let mut one_by_one = p.clone();
        for id in [picked[0], picked[2]] {
            one_by_one = Command::RemoveClip {
                sequence: f.seq,
                clip: id,
                ripple,
            }
            .apply(&one_by_one)
            .unwrap();
        }
        assert_eq!(clips(&together, 0, false), clips(&one_by_one, 0, false));
        assert_eq!(clips(&together, 0, true), clips(&one_by_one, 0, true));
        assert!(clips(&together, 0, true).is_empty());
    }
    let gaps = Command::RemoveClips {
        sequence: f.seq,
        clips: picked,
        ripple: true,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(clips(&gaps, 0, false), vec![(0, 5), (15, 5)]);
}

#[test]
fn removing_several_clips_changes_nothing_when_one_is_gone() {
    let f = fixture();
    let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
    let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
    let first = p.active().unwrap().video_tracks[0].clips[0].id;
    let err = Command::RemoveClips {
        sequence: f.seq,
        clips: vec![first, ClipId(9999)],
        ripple: false,
    }
    .apply(&p)
    .unwrap_err();
    assert!(err.what().contains("no longer exists"));
    let none = Command::RemoveClips {
        sequence: f.seq,
        clips: Vec::new(),
        ripple: false,
    }
    .apply(&p);
    assert!(none.is_err());
}

#[test]
fn moving_keeps_order_and_refuses_the_wrong_kind_of_track() {
    let f = fixture();
    let p = place(&f, &f.p, f.v1, clip(&f, 0, 5, None)).unwrap();
    let id = p.active().unwrap().video_tracks[0].clips[0].id;
    let moved = Command::MoveClip {
        sequence: f.seq,
        clip: id,
        start: Time::seconds(30),
        track: None,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(clips(&moved, 0, false), vec![(30, 5)]);
    let wrong = Command::MoveClip {
        sequence: f.seq,
        clip: id,
        start: Time::ZERO,
        track: Some(f.a1),
    }
    .apply(&p);
    assert!(wrong.is_err());
}

#[test]
fn importing_puts_each_sound_stream_on_its_own_named_track() {
    let p = Project::new("Test", SequenceSettings::GAME_CLIP);
    let seq = p.active_sequence;
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Game", "Mic", "Voice chat"], 30),
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Game", "Mic", "Voice chat"]);
    assert_eq!(s.video_tracks[0].clips.len(), 1);
    let all: Vec<&Clip> = s.tracks().flat_map(|t| t.clips.iter()).collect();
    assert_eq!(all.len(), 4);
    assert!(
        all.iter().all(|c| c.link.is_some() && c.link == all[0].link),
        "all linked"
    );
    let streams: Vec<u32> = s.audio_tracks.iter().map(|t| t.clips[0].stream).collect();
    assert_eq!(streams, [1, 2, 3]);
    assert_eq!(s.length(), Time::seconds(30));
}

/// Streams without a title in their file (named "Audio 1" and
/// "Audio 2" for the Library) leave the tracks their usual short names,
/// as tracks added by hand get: "A1" and "A2", never "Audio 1" beside
/// "A2".
#[test]
fn untitled_sound_streams_keep_the_usual_track_names() {
    let p = Project::new("Test", SequenceSettings::GAME_CLIP);
    let seq = p.active_sequence;
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Audio 1", "Audio 2"], 30),
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["A1", "A2"]);
    assert_eq!(s.next_track_name(TrackKind::Audio), "A3");
    assert!(crate::tracks::titled("Mic") && crate::tracks::titled("Audio"));
    assert!(!crate::tracks::titled("Audio 12"));
}

#[test]
fn a_second_import_goes_after_the_first_and_keeps_track_names() {
    let p = Project::new("Test", SequenceSettings::GAME_CLIP);
    let seq = p.active_sequence;
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Game"], 30),
    }
    .apply(&p)
    .unwrap();
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Other", "Mic"], 10),
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Game", "Mic"], "a used track keeps its name");
    assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(30));
    assert_eq!(s.length(), Time::seconds(40));
    assert_eq!(p.media.len(), 2);
}

#[test]
fn images_get_a_fixed_length_and_files_without_length_are_refused() {
    let p = Project::new("Test", SequenceSettings::GAME_CLIP);
    let seq = p.active_sequence;
    let mut image = recording(&[], 0);
    image.kind = MediaKind::Image;
    let q = Command::Import {
        sequence: seq,
        media: image,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(q.active().unwrap().length(), STILL_LENGTH);
    assert!(q.active().unwrap().video_tracks[0].clips[0].link.is_none());

    let err = Command::Import {
        sequence: seq,
        media: recording(&["Game"], 0),
    }
    .apply(&p)
    .unwrap_err();
    assert!(err.what().contains("can't be placed"));
}

#[test]
fn several_files_imported_at_once_settle_on_what_most_share() {
    let p = Project::new_automatic("Test");
    let seq = p.active_sequence;
    let sized = |w: u32, h: u32, rate: Rate, secs: i64| {
        let mut m = recording(&["Audio 1"], secs);
        if let Some(v) = m.info.video.as_mut() {
            (v.width, v.height, v.rotation, v.frame_rate) = (w, h, 0, rate);
        }
        m
    };
    let mut still = sized(4000, 3000, Rate::FPS_60, 5);
    still.kind = MediaKind::Image;
    // A tiny test pattern first, then a picture, then the real clips.
    let fixture = {
        let mut m = sized(96, 32, Rate::NTSC_30, 1);
        m.duration = Time::from_ratio(1, 10).unwrap();
        m
    };
    let files = [
        fixture.clone(),
        still,
        sized(160, 90, Rate::new(25, 1).unwrap(), 4),
        sized(1920, 1080, Rate::FPS_60, 30),
        sized(160, 90, Rate::new(25, 1).unwrap(), 2),
    ];
    let refs: Vec<&MediaRef> = files.iter().collect();
    let chosen = SequenceSettings::most_common(&refs).unwrap();
    assert_eq!((chosen.width, chosen.height), (160, 90));
    assert_eq!(chosen.frame_rate, Rate::new(25, 1).unwrap());
    assert_eq!(
        SequenceSettings::most_common(&[&fixture]).map(|s| s.width),
        Some(96),
        "a short video alone still counts"
    );
    assert_eq!(
        SequenceSettings::most_common(&[&files[1]]),
        None,
        "a picture is no video"
    );

    let p = Command::ImportSettled {
        sequence: seq,
        media: fixture,
        settings: chosen,
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    assert!(!s.automatic);
    assert_eq!(s.settings, chosen, "in one step with the first file");
    assert_eq!(s.video_tracks[0].clips.len(), 1);
    let again = Command::ImportSettled {
        sequence: seq,
        media: sized(1920, 1080, Rate::FPS_60, 3),
        settings: SequenceSettings::GAME_CLIP,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(again.active().unwrap().settings, chosen, "fixed settings stay");
}

#[test]
fn an_automatic_timeline_takes_the_first_videos_settings() {
    let p = Project::new_automatic("Test");
    let seq = p.active_sequence;
    assert!(p.active().unwrap().automatic);

    // Sound alone doesn't settle it.
    let mut voice = recording(&["Voice"], 5);
    voice.kind = MediaKind::Audio;
    voice.info.video = None;
    let p = Command::Import {
        sequence: seq,
        media: voice,
    }
    .apply(&p)
    .unwrap();
    assert!(p.active().unwrap().automatic);

    let mut phone = recording(&["Audio 1"], 10);
    if let Some(v) = phone.info.video.as_mut() {
        (v.width, v.height, v.rotation, v.frame_rate) = (1920, 1080, 90, Rate::NTSC_30);
    }
    phone.info.audio[0].sample_rate = 44_100;
    let p = Command::Import {
        sequence: seq,
        media: phone,
    }
    .apply(&p)
    .unwrap();
    let s = p.active().unwrap();
    assert!(!s.automatic);
    assert_eq!(
        s.settings,
        SequenceSettings {
            width: 1080,
            height: 1920,
            frame_rate: Rate::NTSC_30,
            sample_rate: 44_100
        }
    );

    // Later videos don't change it.
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Game"], 10),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(p.active().unwrap().settings.width, 1080);
}

#[test]
fn a_project_with_fixed_settings_keeps_them() {
    let p = Project::new("Test", SequenceSettings::GAME_CLIP);
    let seq = p.active_sequence;
    let p = Command::Import {
        sequence: seq,
        media: recording(&["Game"], 10),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(p.active().unwrap().settings, SequenceSettings::GAME_CLIP);
}

#[test]
fn moving_takes_linked_clips_along() {
    let (p, seq) = imported();
    let second = first_video(&p, 1).id;
    let q = Command::MoveLinked {
        sequence: seq,
        clip: second,
        by: Time::seconds(5),
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(35));
    assert_eq!(
        s.audio_tracks[0].clips[1].start,
        Time::seconds(35),
        "its sound came along"
    );
    let back = Command::MoveLinked {
        sequence: seq,
        clip: second,
        by: Time::seconds(-5),
    }
    .apply(&p);
    assert!(
        back.unwrap_err().what().contains("already a clip"),
        "it would overlap the first"
    );
    let first = first_video(&p, 0).id;
    assert!(
        Command::MoveLinked {
            sequence: seq,
            clip: first,
            by: Time::seconds(-1)
        }
        .apply(&p)
        .is_err()
    );
}

#[test]
fn trimming_either_edge_keeps_the_picture_in_place() {
    let (p, seq) = imported();
    let first = first_video(&p, 0).id;
    let q = Command::Trim {
        sequence: seq,
        clip: first,
        edge: Edge::Start,
        to: Time::seconds(4),
    }
    .apply(&p)
    .unwrap();
    let c = first_video(&q, 0);
    assert_eq!(
        (c.start, c.source_in, c.duration),
        (Time::seconds(4), Time::seconds(4), Time::seconds(26))
    );
    let mic = &q.active().unwrap().audio_tracks[1].clips[0];
    assert_eq!(mic.start, Time::seconds(4), "linked sound trimmed too");
    // Back out again, but not before the file starts.
    let r = Command::Trim {
        sequence: seq,
        clip: first,
        edge: Edge::Start,
        to: Time::seconds(1),
    }
    .apply(&q)
    .unwrap();
    assert_eq!(first_video(&r, 0).source_in, Time::seconds(1));
    assert!(
        Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(-1)
        }
        .apply(&q)
        .is_err()
    );

    let e = Command::Trim {
        sequence: seq,
        clip: first,
        edge: Edge::End,
        to: Time::seconds(20),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(first_video(&e, 0).duration, Time::seconds(20));
    let long = Command::Trim {
        sequence: seq,
        clip: first,
        edge: Edge::End,
        to: Time::seconds(31),
    }
    .apply(&e);
    assert!(
        long.unwrap_err()
            .why_text()
            .unwrap_or_default()
            .contains("ends before")
    );
    let gone = Command::Trim {
        sequence: seq,
        clip: first,
        edge: Edge::End,
        to: Time::ZERO,
    }
    .apply(&p);
    assert!(gone.is_err());
}

#[test]
fn ripple_delete_closes_the_gap_on_every_track() {
    let (p, seq) = gap_then_streams();
    let s = p.active().unwrap();
    assert_eq!(s.audio_tracks[1].clips[0].start, Time::seconds(10), "the mic");
    assert_eq!(s.video_tracks[1].clips[0].start, Time::seconds(15), "the layer");
    let first = first_video(&p, 0).id;
    // Older journals' ripple removal keeps its meaning: same tracks only.
    let old = Command::RemoveClip {
        sequence: seq,
        clip: first,
        ripple: true,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(
        old.active().unwrap().audio_tracks[1].clips[0].start,
        Time::seconds(10)
    );
    {
        let q = Command::RippleRemoveClips {
            sequence: seq,
            clips: vec![first],
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.video_tracks[0].clips[0].start, Time::ZERO);
        assert_eq!(s.audio_tracks[0].clips[0].start, Time::ZERO, "game sound");
        assert_eq!(
            s.audio_tracks[1].clips[0].start,
            Time::ZERO,
            "the mic stays in step with its picture"
        );
        assert_eq!(
            s.video_tracks[1].clips[0].start,
            Time::seconds(5),
            "the adjustment layer moves with the picture under it"
        );
    }
}

#[test]
fn ripple_delete_is_refused_when_another_track_has_something_in_the_gap() {
    let (p, seq) = gap_then_streams();
    // The layer at 15..20 s sits over the second recording; removing
    // that recording would cut the layer when the gap closes.
    let second = first_video(&p, 1).id;
    let err = Command::RippleRemoveClips {
        sequence: seq,
        clips: vec![second],
    }
    .apply(&p)
    .unwrap_err();
    assert!(err.what().contains("can't be closed"), "{}", err.report());
    assert!(err.report().contains("Adjustment layer"), "{}", err.report());
    // Without closing the gap it goes, the layer stays.
    let q = Command::RemoveClip {
        sequence: seq,
        clip: second,
        ripple: false,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(q.active().unwrap().video_tracks[1].clips.len(), 1);
    // Taken along, both go and the gap closes.
    let layer = p.active().unwrap().video_tracks[1].clips[0].id;
    let both = Command::RippleRemoveClips {
        sequence: seq,
        clips: vec![second, layer],
    }
    .apply(&p)
    .unwrap();
    assert!(both.active().unwrap().tracks().all(|t| t.clips.len() <= 1));
}

#[test]
fn several_clips_move_together_with_their_sound() {
    let (p, seq) = gap_then_streams();
    let s = p.active().unwrap();
    let picked = vec![s.video_tracks[0].clips[1].id, s.video_tracks[1].clips[0].id];
    let q = Command::MoveClips {
        sequence: seq,
        clips: picked.clone(),
        by: Time::seconds(5),
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(15));
    assert_eq!(
        s.audio_tracks[1].clips[0].start,
        Time::seconds(15),
        "the mic came along"
    );
    assert_eq!(
        s.video_tracks[1].clips[0].start,
        Time::seconds(20),
        "the layer too"
    );
    assert_eq!(s.video_tracks[0].clips[0].start, Time::ZERO, "the first stays");
    // Into the first clip: refused, nothing changes.
    let err = Command::MoveClips {
        sequence: seq,
        clips: picked,
        by: Time::seconds(-5),
    }
    .apply(&p)
    .unwrap_err();
    assert!(err.what().contains("already a clip"), "{}", err.report());
}

#[test]
fn a_gap_closes_on_every_track_or_not_at_all() {
    let (p, seq) = gap_then_streams();
    let first = first_video(&p, 0).id;
    // Leave a gap at 0..10 s, then close it.
    let gap = Command::RemoveClips {
        sequence: seq,
        clips: vec![first],
        ripple: false,
    }
    .apply(&p)
    .unwrap();
    let closed = Command::CloseGap {
        sequence: seq,
        start: Time::ZERO,
        end: Time::seconds(10),
    }
    .apply(&gap)
    .unwrap();
    let s = closed.active().unwrap();
    assert_eq!(s.video_tracks[0].clips[0].start, Time::ZERO);
    assert_eq!(s.audio_tracks[1].clips[0].start, Time::ZERO);
    assert_eq!(s.video_tracks[1].clips[0].start, Time::seconds(5));
    // Something else in the stretch: refused.
    assert!(
        Command::CloseGap {
            sequence: seq,
            start: Time::seconds(12),
            end: Time::seconds(16),
        }
        .apply(&closed)
        .is_err()
    );
}

#[test]
fn placing_a_file_again_adds_it_at_the_end() {
    let (p, seq) = imported();
    let media = first_video(&p, 0).media;
    let q = Command::PlaceMedia {
        sequence: seq,
        media,
        targets: Default::default(),
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(q.media.len(), p.media.len(), "the same file, not a copy");
    let last = s.video_tracks[0].clips.last().unwrap();
    assert_eq!((last.start, last.media), (Time::seconds(40), media));
    assert_eq!(
        s.audio_tracks[1].clips.last().unwrap().start,
        Time::seconds(40),
        "with its sound"
    );
    let gone = Command::PlaceMedia {
        sequence: seq,
        media: MediaId(9_999),
        targets: Default::default(),
    };
    assert!(gone.apply(&p).is_err());
}

#[test]
fn large_selections_move_and_close_as_small_ones_do() {
    let (f, p) = many_linked(300);
    let seq = p.active().unwrap();
    let every_third: Vec<ClipId> = seq.video_tracks[0]
        .clips
        .iter()
        .step_by(3)
        .map(|c| c.id)
        .collect();
    // Rippled out: every track closes up, linked sound included.
    let q = Command::RippleRemoveClips {
        sequence: f.seq,
        clips: every_third.clone(),
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(s.video_tracks[0].clips.len(), 200);
    assert_eq!(s.length(), Time::seconds(600));
    assert!(
        s.video_tracks[0]
            .clips
            .iter()
            .zip(s.audio_tracks[0].clips.iter())
            .all(|(v, a)| v.start == a.start)
    );
    // Everything from 450 s on moves 30 s later, sound with it.
    let later: Vec<ClipId> = seq.video_tracks[0]
        .clips
        .iter()
        .filter(|c| c.start >= Time::seconds(450))
        .map(|c| c.id)
        .collect();
    let q = Command::MoveClips {
        sequence: f.seq,
        clips: later,
        by: Time::seconds(30),
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    for t in s.tracks() {
        assert!(
            t.clips
                .iter()
                .zip(t.clips.iter().skip(1))
                .all(|(a, b)| a.end() <= b.start),
            "in order"
        );
    }
    assert_eq!(s.length(), Time::seconds(930));
    // Moved onto clips that stay: refused, nothing changed.
    let onto = Command::MoveClips {
        sequence: f.seq,
        clips: every_third,
        by: Time::seconds(1),
    };
    assert!(onto.apply(&p).is_err());
}

#[test]
fn insert_pushes_later_and_overwrite_covers() {
    let (p, seq) = imported(); // 0-30 s (video, game, mic) and 30-40 s (video, game)
    let short = first_video(&p, 1).media; // the 10 s file
    let ins = Command::InsertMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(10),
        targets: Default::default(),
    }
    .apply(&p)
    .unwrap();
    let s = ins.active().unwrap();
    assert_eq!(s.length(), Time::seconds(50), "ten seconds longer");
    let starts: Vec<Time> = s.video_tracks[0].clips.iter().map(|c| c.start).collect();
    assert_eq!(
        starts,
        [
            Time::ZERO,
            Time::seconds(10),
            Time::seconds(20),
            Time::seconds(40)
        ]
    );
    assert_eq!(
        s.audio_tracks[1].clips[1].start,
        Time::seconds(20),
        "the mic's second half moved too"
    );

    let over = Command::OverwriteMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(25),
    }
    .apply(&p)
    .unwrap();
    let s = over.active().unwrap();
    assert_eq!(s.length(), Time::seconds(40), "nothing moved");
    let v: Vec<(Time, Time)> = s.video_tracks[0]
        .clips
        .iter()
        .map(|c| (c.start, c.end()))
        .collect();
    assert_eq!(
        v,
        [
            (Time::ZERO, Time::seconds(25)),
            (Time::seconds(25), Time::seconds(35)),
            (Time::seconds(35), Time::seconds(40)),
        ]
    );
    assert_eq!(
        s.audio_tracks[1].clips.last().unwrap().end(),
        Time::seconds(25),
        "the mic under it was cut"
    );
}

/// The two imported files of [`imported`] and a second, empty video track.
fn with_two_video_tracks() -> (Project, SequenceId, TrackId) {
    let (p, seq) = imported();
    let p = Command::AddTrack {
        sequence: seq,
        kind: TrackKind::Video,
        name: "V2".into(),
    }
    .apply(&p)
    .unwrap();
    let v2 = p.active().unwrap().video_tracks[1].id;
    (p, seq, v2)
}

#[test]
fn a_clip_moves_to_another_track_and_its_linked_clips_stay_on_theirs() {
    let (p, seq, v2) = with_two_video_tracks();
    let second = first_video(&p, 1); // 30-40 s, with its game sound on A1
    let up = Command::MoveToTrack {
        sequence: seq,
        clip: second.id,
        track: v2,
        by: Time::seconds(2),
    }
    .apply(&p)
    .unwrap();
    let s = up.active().unwrap();
    assert_eq!(spans(&s.video_tracks[0]), [(0, 30)], "it left V1");
    assert_eq!(spans(&s.video_tracks[1]), [(32, 42)]);
    assert_eq!(s.video_tracks[1].clips[0].id, second.id, "the same clip");
    assert_eq!(
        spans(&s.audio_tracks[0]),
        [(0, 30), (32, 42)],
        "its sound moved sideways with it, on its own track"
    );
    assert_eq!(
        s.audio_tracks[0].clips[1].link, second.link,
        "and they are still linked"
    );
    // Straight up, without moving sideways.
    let straight = Command::MoveToTrack {
        sequence: seq,
        clip: second.id,
        track: v2,
        by: Time::ZERO,
    }
    .apply(&p)
    .unwrap();
    assert_eq!(spans(&straight.active().unwrap().video_tracks[1]), [(30, 40)]);
    // And back down again.
    let v1 = p.active().unwrap().video_tracks[0].id;
    let back = Command::MoveToTrack {
        sequence: seq,
        clip: second.id,
        track: v1,
        by: Time::ZERO,
    }
    .apply(&straight)
    .unwrap();
    assert_eq!(back, p);
    // To the track it is on: a plain move.
    let same = Command::MoveToTrack {
        sequence: seq,
        clip: second.id,
        track: v1,
        by: Time::seconds(5),
    }
    .apply(&p)
    .unwrap();
    let plain = Command::MoveLinked {
        sequence: seq,
        clip: second.id,
        by: Time::seconds(5),
    }
    .apply(&p)
    .unwrap();
    assert_eq!(same, plain);
}

#[test]
fn a_clip_keeps_its_look_and_speed_on_another_track_and_under_a_dropped_file() {
    let (p, seq, v2) = with_two_video_tracks(); // 0-30 s and 30-40 s on V1
    let (first, second) = (first_video(&p, 0), first_video(&p, 1));
    let look = Look {
        x: -250_000,
        scale: 400_000,
        crop_top: 50_000,
        opacity: 800_000,
        tint: -100_000,
        ..Look::PLAIN
    };
    let mut p = p;
    for clip in [first.id, second.id] {
        p = Command::SetLook {
            sequence: seq,
            clip,
            look,
        }
        .apply(&p)
        .unwrap();
    }
    let half = Speed::new(1, 2).unwrap();
    let p = Command::SetSpeed {
        sequence: seq,
        clip: second.id,
        speed: half,
    }
    .apply(&p)
    .unwrap();
    // The second clip, now 30-50 s at half speed, goes up a track.
    let up = Command::MoveToTrack {
        sequence: seq,
        clip: second.id,
        track: v2,
        by: Time::seconds(3),
    }
    .apply(&p)
    .unwrap();
    let moved = &up.active().unwrap().video_tracks[1].clips[0];
    assert_eq!(
        moved,
        &Clip {
            start: Time::seconds(33),
            ..p.active().unwrap().video_tracks[0].clips[1].clone()
        },
        "nothing but its place changed"
    );
    assert_eq!((moved.look, moved.speed), (look, half));
    // A file dropped on V2 over the first clip: the first clip is cut
    // where its sound is covered, and every piece keeps the look.
    let short = second.media;
    let dropped = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(5),
        track: Some(v2),
        insert: false,
    }
    .apply(&p)
    .unwrap();
    let s = dropped.active().unwrap();
    let pieces: Vec<&Clip> = s.video_tracks[0]
        .clips
        .iter()
        .filter(|c| c.media == first.media)
        .collect();
    assert_eq!(pieces.len(), 3, "0-5, 5-15 and 15-30 s");
    assert!(pieces.iter().all(|c| c.look == look));
    assert_eq!(
        s.video_tracks[1].clips[0].look,
        Look::PLAIN,
        "the new clip starts plain"
    );
}

#[test]
fn a_sound_clip_moves_to_another_audio_track() {
    let (p, seq) = imported();
    let s = p.active().unwrap();
    let (game, mic_track) = (s.audio_tracks[0].clips[1].clone(), s.audio_tracks[1].id);
    let q = Command::MoveToTrack {
        sequence: seq,
        clip: game.id,
        track: mic_track,
        by: Time::ZERO,
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(spans(&s.audio_tracks[0]), [(0, 30)]);
    assert_eq!(spans(&s.audio_tracks[1]), [(0, 30), (30, 40)]);
    assert_eq!(
        spans(&s.video_tracks[0]),
        [(0, 30), (30, 40)],
        "its picture stayed"
    );
}

#[test]
fn moving_to_a_track_is_refused_where_it_does_not_fit_and_nothing_changes() {
    let (p, seq, v2) = with_two_video_tracks();
    let second = first_video(&p, 1);
    let to = |p: &Project, track, by| {
        Command::MoveToTrack {
            sequence: seq,
            clip: second.id,
            track,
            by: Time::seconds(by),
        }
        .apply(p)
    };
    // Its sound would land on the first clip's sound.
    let err = to(&p, v2, -30).unwrap_err();
    assert!(err.what().contains("already a clip"), "{}", err.what());
    assert!(err.report().contains("Game"), "names the track: {}", err.report());
    // A clip is already there on the track it goes to.
    let taken = Command::PlaceClip {
        sequence: seq,
        track: v2,
        clip: Clip {
            start: Time::seconds(35),
            link: None,
            ..second.clone()
        },
    }
    .apply(&p)
    .unwrap();
    let err = to(&taken, v2, 0).unwrap_err();
    assert!(err.report().contains("V2"), "{}", err.report());
    // A video clip on an audio track, a track that is gone, before the start.
    let a1 = p.active().unwrap().audio_tracks[0].id;
    assert!(
        to(&p, a1, 0)
            .unwrap_err()
            .what()
            .contains("can't move to that track")
    );
    assert!(
        to(&p, TrackId(9_999), 0)
            .unwrap_err()
            .what()
            .contains("no longer exists")
    );
    assert!(to(&p, v2, -31).unwrap_err().what().contains("can't move there"));
}

#[test]
fn a_file_dropped_on_a_higher_track_leaves_the_picture_under_it() {
    let (p, seq, v2) = with_two_video_tracks(); // 0-30 s (video, game, mic), 30-40 s (video, game)
    let short = first_video(&p, 1).media; // the 10 s file, with game sound
    let q = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(5),
        track: Some(v2),
        insert: false,
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(s.length(), Time::seconds(40), "nothing moved");
    assert_eq!(spans(&s.video_tracks[1]), [(5, 15)], "the picture is on V2");
    assert_eq!(
        spans(&s.video_tracks[0]),
        [(0, 5), (5, 15), (15, 30), (30, 40)],
        "V1 keeps its picture under it (cut where its sound was cut)"
    );
    assert_eq!(
        spans(&s.audio_tracks[0]),
        [(0, 5), (5, 15), (15, 30), (30, 40)],
        "its sound lies over the game sound"
    );
    let (picture, sound) = (&s.video_tracks[1].clips[0], &s.audio_tracks[0].clips[1]);
    assert_eq!((sound.media, sound.link), (short, picture.link));
    assert!(picture.link.is_some());
    assert_eq!(
        spans(&s.audio_tracks[1]),
        [(0, 5), (5, 15), (15, 30)],
        "the mic, which nothing landed on, keeps going"
    );
    // Every piece of the first clip is still as long as the pieces linked to it.
    for v in &s.video_tracks[0].clips {
        for a in s.audio_tracks.iter().flat_map(|t| t.clips.iter()) {
            if a.link == v.link {
                assert_eq!((a.start, a.duration), (v.start, v.duration));
            }
        }
    }
}

#[test]
fn a_dropped_file_covers_only_the_tracks_it_lands_on() {
    let (p, seq, v2) = with_two_video_tracks();
    let short = first_video(&p, 1).media;
    // Something on V2 over the place where the file is dropped on V1.
    let p = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(40),
        track: Some(v2),
        insert: false,
    }
    .apply(&p)
    .unwrap();
    let q = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(35),
        track: None,
        insert: false,
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(
        spans(&s.video_tracks[0]),
        [(0, 30), (30, 35), (35, 45)],
        "on the first video track, over what was there"
    );
    assert_eq!(
        spans(&s.video_tracks[1]),
        [(40, 45), (45, 50)],
        "V2 keeps its clip (cut where its sound was covered)"
    );
    assert_eq!(spans(&s.audio_tracks[0]), [(0, 30), (30, 35), (35, 45), (45, 50)]);
}

#[test]
fn a_file_inserted_on_a_track_pushes_every_track_later() {
    let (p, seq, v2) = with_two_video_tracks();
    let short = first_video(&p, 1).media;
    let q = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(5),
        track: Some(v2),
        insert: true,
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(s.length(), Time::seconds(50), "ten seconds longer");
    assert_eq!(spans(&s.video_tracks[1]), [(5, 15)]);
    assert_eq!(spans(&s.video_tracks[0]), [(0, 5), (15, 40), (40, 50)]);
    assert_eq!(spans(&s.audio_tracks[0]), [(0, 5), (5, 15), (15, 40), (40, 50)]);
    assert_eq!(spans(&s.audio_tracks[1]), [(0, 5), (15, 40)]);
}

#[test]
fn a_file_is_not_dropped_on_a_track_that_cannot_take_its_picture() {
    let (p, seq, _) = with_two_video_tracks();
    let short = first_video(&p, 1).media;
    let on = |track| {
        Command::DropMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(5),
            track: Some(track),
            insert: false,
        }
        .apply(&p)
    };
    let a1 = p.active().unwrap().audio_tracks[0].id;
    let err = on(a1).unwrap_err();
    assert!(err.what().contains("picture can't go"), "{}", err.what());
    assert!(
        on(TrackId(9_999))
            .unwrap_err()
            .what()
            .contains("no longer exists")
    );
    let early = Command::DropMedia {
        sequence: seq,
        media: short,
        at: Time::seconds(-1),
        track: None,
        insert: true,
    }
    .apply(&p);
    assert!(early.unwrap_err().what().contains("can't be placed there"));
}

/// `p` with the sync lock of `track` (by place among its kind) off.
fn unsynced(p: &Project, seq: SequenceId, video: bool, index: usize) -> Project {
    let s = p.active().unwrap();
    let track = match video {
        true => s.video_tracks[index].id,
        false => s.audio_tracks[index].id,
    };
    Command::SetTrackSync {
        sequence: seq,
        track,
        synced: false,
    }
    .apply(p)
    .unwrap()
}

#[test]
fn a_track_out_of_sync_lock_stays_put_through_a_ripple_delete() {
    let (p, seq) = gap_then_streams();
    // The layer's track out of sync lock: closing the gap before it
    // leaves it where it is.
    let p = unsynced(&p, seq, true, 1);
    let first = first_video(&p, 0).id;
    let q = Command::RippleRemoveClips {
        sequence: seq,
        clips: vec![first],
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert_eq!(
        s.video_tracks[0].clips[0].start,
        Time::ZERO,
        "the picture moved back"
    );
    assert_eq!(
        s.video_tracks[1].clips[0].start,
        Time::seconds(15),
        "the layer stayed"
    );
    // So the second recording can now be removed: the layer is no longer
    // in the way of a gap that closes.
    let second = first_video(&p, 1).id;
    assert!(
        Command::RippleRemoveClips {
            sequence: seq,
            clips: vec![second],
        }
        .apply(&p)
        .is_ok()
    );
}

#[test]
fn an_insert_leaves_a_track_out_of_sync_lock_whole_unless_it_lands_there() {
    let (p, seq) = imported(); // 0-30 s (video, game, mic) and 30-40 s (video, game)
    let short = first_video(&p, 1).media; // the 10 s file: a picture and the game
    let p = unsynced(&p, seq, false, 1); // the mic
    let insert = |p: &Project, targets: Targets| {
        Command::InsertMedia {
            sequence: seq,
            media: short,
            at: Time::seconds(10),
            targets,
        }
        .apply(p)
        .unwrap()
    };
    let q = insert(&p, Targets::default());
    let s = q.active().unwrap();
    let spans_of =
        |t: &Track| -> Vec<(Time, Time)> { t.clips.iter().map(|c| (c.start, c.duration)).collect() };
    assert_eq!(
        spans_of(&s.audio_tracks[1]),
        [(Time::ZERO, Time::seconds(30))],
        "the mic is neither cut nor moved"
    );
    assert_eq!(
        s.video_tracks[0].clips[1].start,
        Time::seconds(10),
        "the picture went in"
    );
    // Its game sound aimed at the mic's track: that track takes it, so it
    // is cut and moves as an insert on it would.
    let mic = s.audio_tracks[1].id;
    let q = insert(
        &p,
        Targets {
            video: None,
            audio: Some(mic),
        },
    );
    let s = q.active().unwrap();
    let landed = s.audio_tracks[1].clips.iter().find(|c| c.media == short).unwrap();
    assert_eq!(landed.start, Time::seconds(10));
    assert_eq!(s.audio_tracks[1].clips.len(), 3, "the mic cut around it");
    assert!(
        s.audio_tracks[0]
            .clips
            .iter()
            .all(|c| c.media != short || c.start != Time::seconds(10)),
        "nothing new on the game track"
    );
}

#[test]
fn placing_on_targets_puts_the_picture_and_sound_there() {
    let (p, seq) = imported();
    let short = first_video(&p, 1).media;
    let s = p.active().unwrap();
    let (mic, v1) = (s.audio_tracks[1].id, s.video_tracks[0].id);
    let q = Command::PlaceMedia {
        sequence: seq,
        media: short,
        targets: Targets {
            video: Some(v1),
            audio: Some(mic),
        },
    }
    .apply(&p)
    .unwrap();
    let s = q.active().unwrap();
    assert!(
        s.audio_tracks[1]
            .clips
            .iter()
            .any(|c| c.media == short && c.start == Time::seconds(40))
    );
    // A target that is gone: the first tracks, as before.
    let gone = Targets {
        video: Some(TrackId(999)),
        audio: Some(TrackId(998)),
    };
    let q = Command::PlaceMedia {
        sequence: seq,
        media: short,
        targets: gone,
    }
    .apply(&p)
    .unwrap();
    assert!(
        q.active().unwrap().audio_tracks[0]
            .clips
            .iter()
            .any(|c| c.start == Time::seconds(40))
    );
}
