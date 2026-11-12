//! Every released file format keeps opening.
//!
//! `tests/samples/project-v<N>.ron` holds one project saved in format N. When
//! the format changes, the old samples stay and must still open (through the
//! migrations); a new sample is added for the new version with:
//!
//! ```sh
//! WRITE_SAMPLES=1 cargo test -p project --test samples
//! ```
//!
//! Templates have their own format and their own samples
//! (`template-v<N>.ron`). A sample is written once, when its format is new
//! (the same command writes it if it is not there yet), and never again:
//! the model types a template holds may gain fields with defaults without
//! a new template format, and the stored text is what proves that a
//! template saved before such a field still reads.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used)]

use project::file::{self, FORMAT_VERSION};
use project::templates::{self, Template};
use project::{
    AudioStream, Clip, Fingerprint, LabelColor, MediaInfo, MediaKind, MediaRef, Project, Rate, Region,
    SequenceSettings, Time, TimelineLabel, VideoStream,
};
use std::path::{Path, PathBuf};

fn sample_path(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/samples/project-v{version}.ron"))
}

/// A project that uses every part of the model.
fn rich_project() -> Project {
    let mut p = Project::new("Sample project", SequenceSettings::GAME_CLIP);
    let video = p.allocate_id();
    p.media.insert(
        video,
        MediaRef {
            id: video,
            path: PathBuf::from("recordings/match_04.mkv"),
            fingerprint: Fingerprint {
                size: 2_147_483_648,
                sample_hash: "9f2c1a".into(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(600),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 2560,
                    height: 1440,
                    frame_rate: Rate::NTSC_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: vec![
                    AudioStream {
                        index: 1,
                        codec: "aac".into(),
                        channels: 2,
                        sample_rate: 48_000,
                        name: "Game".into(),
                    },
                    AudioStream {
                        index: 2,
                        codec: "opus".into(),
                        channels: 1,
                        sample_rate: 48_000,
                        name: "Mic".into(),
                    },
                ],
                // Its end was missing; imported as far as it goes (format 14).
                stated_length: Some(Time::seconds(660)),
            },
        },
    );
    let link = Some(p.allocate_id());
    let (v_clip, a_clip, label) = (p.allocate_id(), p.allocate_id(), p.allocate_id());
    let extra_audio = p.new_track(project::TrackKind::Audio, "A2 Mic");
    let seq = p.sequences.front_mut().expect("the new project has a timeline");
    seq.settings.frame_rate = Rate::NTSC_60;
    let clip = |id, stream| Clip {
        id,
        media: video,
        stream,
        start: Time::seconds(1),
        source_in: Rate::NTSC_60.frame_start(90),
        duration: Time::seconds(20),
        link,
        speed: Default::default(),
        look: Default::default(),
        sound: Default::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
        kind: Default::default(),
    };
    // The picture in a corner, cut a little, slightly see-through and warmer.
    let look = project::Look {
        x: 320_000,
        y: 280_000,
        scale: 300_000,
        turn: -2_500_000,
        crop_left: 120_000,
        crop_bottom: 50_000,
        opacity: 900_000,
        exposure: 250_000,
        temperature: 150_000,
        ..project::Look::PLAIN
    };
    // It fades in over its first second and slides right, then holds
    // its place until a jump (keyframes, format 10).
    let mut keyframes = project::Keyframes::default();
    let from = Rate::NTSC_60.frame_start(90);
    let none = (Time::ZERO, Time::ZERO);
    keyframes.set(project::LookField::Opacity, from, none, 0);
    keyframes.set(
        project::LookField::Opacity,
        from + Time::seconds(1),
        none,
        900_000,
    );
    keyframes.set(project::LookField::X, from, none, 320_000);
    keyframes.set(project::LookField::X, from + Time::seconds(2), none, 360_000);
    keyframes.set(project::LookField::Y, from, none, 280_000);
    keyframes.set(project::LookField::Y, from + Time::seconds(2), none, 280_000);
    let hold = (from, from + Time::from_ratio(1, 120).expect("half a frame"));
    keyframes.set_ease(project::LookField::X, hold, project::Ease::Smooth);
    keyframes.set(project::LookField::Scale, from + Time::seconds(5), none, 300_000);
    keyframes.set(project::LookField::Scale, from + Time::seconds(6), none, 450_000);
    let at_five = (from + Time::seconds(5), from + Time::seconds(6));
    keyframes.set_ease(project::LookField::Scale, at_five, project::Ease::Hold);
    // It eases into slow motion two seconds in and back out a second
    // later, blending frames (format 11); its sound follows the same ramp.
    let ramp = project::ramp::slow_motion(
        Time::seconds(2),
        project::Speed::NORMAL,
        project::Speed::new(1, 4).expect("a quarter"),
        Time::from_ratio(1, 2).expect("half a second"),
        Some(Time::seconds(1)),
    );
    seq.video_tracks[0].clips.push_back(Clip {
        look,
        keyframes,
        ramp: ramp.clone(),
        blend: project::FrameBlend::Blend,
        ..clip(v_clip, 0)
    });
    // The game sound a little quieter, a little to the left, fading in and out.
    let sound = project::Sound {
        gain_mdb: -3_000,
        pan: -200_000,
        fade_in: Time::from_ratio(1, 2).expect("half a second"),
        fade_out: Rate::NTSC_60.frame_start(45),
    };
    seq.audio_tracks[0].clips.push_back(Clip {
        sound,
        ramp,
        ..clip(a_clip, 1)
    });
    seq.audio_tracks.push_back(extra_audio);
    seq.audio_tracks[1].gain_db = -3.5;
    seq.labels.push_back(TimelineLabel {
        id: label,
        name: "First kill".into(),
        at: Time::seconds(24),
        color: LabelColor::Green,
    });
    seq.region = Region::between(Time::seconds(20), Time::seconds(50));
    seq.edit_needle = Time::seconds(81);

    // A second timeline holding a copy of the picture, played in the first
    // from 30 s on a track of its own, and an adjustment layer above
    // that warms everything from 21 s for 3 s (format 12).
    let main = p.active_sequence;
    let mut short = p.new_sequence("Short cut", SequenceSettings::VERTICAL_SHORT);
    let inner_clip = p.allocate_id();
    short.video_tracks[0].clips.push_back(Clip {
        start: Time::ZERO,
        duration: Time::seconds(4),
        link: None,
        ..clip(inner_clip, 0)
    });
    let (nested_id, nested_clip, adjustment) = (short.id, p.allocate_id(), p.allocate_id());
    let (mut v2, mut v3) = (
        p.new_track(project::TrackKind::Video, "V2"),
        p.new_track(project::TrackKind::Video, "V3"),
    );
    v2.clips.push_back(Clip {
        media: project::MediaId::NONE,
        start: Time::seconds(30),
        source_in: Time::seconds(1),
        duration: Time::seconds(3),
        link: None,
        look: project::Look {
            scale: 500_000,
            ..project::Look::PLAIN
        },
        kind: project::ClipKind::Timeline(nested_id),
        ..clip(nested_clip, 0)
    });
    v3.clips.push_back(Clip {
        media: project::MediaId::NONE,
        start: Time::seconds(21),
        source_in: Time::ZERO,
        duration: Time::seconds(3),
        link: None,
        look: project::Look {
            temperature: 300_000,
            saturation: 1_200_000,
            ..project::Look::PLAIN
        },
        kind: project::ClipKind::Adjustment,
        ..clip(adjustment, 0)
    });
    p.sequences.push_back(short);
    let seq = p
        .sequences
        .iter_mut()
        .find(|s| s.id == main)
        .expect("the first timeline");
    seq.video_tracks.push_back(v2);
    seq.video_tracks.push_back(v3);

    // The recording is labelled, in a list and has a note (format 13).
    let p = project::Command::AddMediaLabel {
        label: project::MediaLabel {
            id: project::MediaLabelId(0),
            name: "Gameplay".into(),
            color: LabelColor::Orange,
        },
        media: vec![video],
    }
    .apply(&p)
    .expect("a label");
    let p = project::Command::AddMediaList {
        name: "Opening sequence".into(),
        media: vec![video],
    }
    .apply(&p)
    .expect("a list");
    project::Command::SetMediaNote {
        media: video,
        note: "Clutch round at 4:10".into(),
    }
    .apply(&p)
    .expect("a note")
}

#[test]
fn version_1_sample_opens() {
    let project = file::load(&sample_path(1)).expect("the version 1 sample opens");
    let seq = project.active().expect("an open timeline");
    assert_eq!(project.name, "Sample project");
    assert_eq!(seq.settings.frame_rate, Rate::NTSC_60);
    assert_eq!(seq.video_tracks[0].clips.len(), 1);
    assert_eq!(seq.audio_tracks.len(), 2);
    assert_eq!(seq.labels[0].name, "First kill");
    assert_eq!(seq.region.map(|r| r.length()), Some(Time::seconds(30)));
    assert_eq!(seq.edit_needle, Time::seconds(81));
    let media = project.media.values().next().expect("the sample has media");
    assert_eq!(media.info, MediaInfo::default(), "format 1 had no stream details");
}

#[test]
fn version_2_sample_opens_with_stream_details() {
    let project = file::load(&sample_path(2)).expect("the version 2 sample opens");
    let media = project.media.values().next().expect("the sample has media");
    let names: Vec<&str> = media.info.audio.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Game", "Mic"]);
    assert_eq!(
        media.info.video.as_ref().map(|v| v.display_size()),
        Some((2560, 1440))
    );
}

#[test]
fn version_8_sample_opens_with_plain_sound() {
    let project = file::load(&sample_path(8)).expect("the version 8 sample opens");
    let seq = project.active().expect("an open timeline");
    let picture = &seq.video_tracks[0].clips[0];
    assert_eq!(picture.look.scale, 300_000, "format 8 kept the look");
    for clip in seq.tracks().flat_map(|t| t.clips.iter()) {
        assert_eq!(
            clip.sound,
            project::Sound::PLAIN,
            "format 8 had no sound settings"
        );
    }
}

#[test]
fn version_9_sample_opens_without_keyframes() {
    let project = file::load(&sample_path(9)).expect("the version 9 sample opens");
    let seq = project.active().expect("an open timeline");
    let picture = &seq.video_tracks[0].clips[0];
    assert_eq!(picture.look.opacity, 900_000, "format 9 kept the look");
    assert_eq!(
        seq.audio_tracks[0].clips[0].sound.gain_mdb, -3_000,
        "and the sound"
    );
    for clip in seq.tracks().flat_map(|t| t.clips.iter()) {
        assert!(clip.keyframes.is_empty(), "format 9 had no keyframes");
        assert_eq!(
            clip.look_at(clip.start),
            clip.look,
            "so a clip looks the same all along"
        );
    }
}

#[test]
fn version_10_sample_opens_without_ramps() {
    let project = file::load(&sample_path(10)).expect("the version 10 sample opens");
    let seq = project.active().expect("an open timeline");
    let picture = &seq.video_tracks[0].clips[0];
    assert!(!picture.keyframes.is_empty(), "format 10 kept the keyframes");
    for clip in seq.tracks().flat_map(|t| t.clips.iter()) {
        assert!(clip.ramp.is_empty(), "format 10 had no speed ramps");
        assert_eq!(clip.blend, project::FrameBlend::Off, "nor frame blending");
        assert_eq!(
            clip.source_len(),
            clip.duration,
            "so a clip plays at its one speed"
        );
    }
}

#[test]
fn version_11_sample_opens_with_one_timeline_of_file_clips() {
    let project = file::load(&sample_path(11)).expect("the version 11 sample opens");
    assert_eq!(project.sequences.len(), 1);
    for clip in project.sequences[0].tracks().flat_map(|t| t.clips.iter()) {
        assert_eq!(
            clip.kind,
            project::ClipKind::File,
            "format 11 clips all play files"
        );
    }
}

#[test]
fn the_labels_for_every_project_sample_opens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples/labels-v1.ron");
    let text = std::fs::read_to_string(path).expect("the sample is there");
    let set = project::GlobalLabels::parse(&text).expect("it reads");
    let names: Vec<&str> = set.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Drone", "Interview"]);
    assert_eq!(set.labels[0].color, LabelColor::Green);
    assert_eq!(
        project::global_labels::FORMAT_VERSION,
        1,
        "a new format of labels.ron needs a migration and a sample of its own"
    );
}

#[test]
fn version_12_sample_opens_with_no_labels_lists_or_notes() {
    let project = file::load(&sample_path(12)).expect("the version 12 sample opens");
    assert!(project.library.is_empty(), "format 12 had no library");
    assert_eq!(project.sequences.len(), 2, "and kept its timelines");
}

#[test]
fn version_13_sample_opens_with_whole_files() {
    let project = file::load(&sample_path(13)).expect("the version 13 sample opens");
    assert!(project.media.values().all(|m| m.info.stated_length.is_none()));
    assert!(!project.library.is_empty(), "and keeps its labels");
}

#[test]
fn the_current_sample_has_a_file_imported_with_its_end_missing() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let media = project.media.values().next().expect("a file");
    assert_eq!(media.info.stated_length, Some(Time::seconds(660)));
    assert_eq!(media.duration, Time::seconds(600));
}

#[test]
fn the_current_sample_has_labels_a_list_and_a_note() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let media = *project.media.keys().next().expect("a file");
    let library = &project.library;
    assert_eq!(library.labels_of(media)[0].name, "Gameplay");
    assert_eq!(library.lists_of(media)[0].name, "Opening sequence");
    assert_eq!(library.note(media), "Clutch round at 4:10");
}

#[test]
fn the_current_sample_has_a_nested_timeline_and_an_adjustment_layer() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    assert_eq!(project.sequences.len(), 2);
    let main = project.active().expect("an open timeline");
    let kinds: Vec<project::ClipKind> = main
        .video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| c.kind)
        .collect();
    assert!(kinds.contains(&project::ClipKind::Timeline(project.sequences[1].id)));
    assert!(kinds.contains(&project::ClipKind::Adjustment));
}

#[test]
fn current_format_sample_matches_the_model() {
    let path = sample_path(FORMAT_VERSION);
    if std::env::var_os("WRITE_SAMPLES").is_some() {
        file::save(&rich_project(), &path).expect("sample written");
    }
    assert_eq!(file::load(&path).expect("sample opens"), rich_project());
}

fn template_sample_path(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/samples/template-v{version}.ron"))
}

/// A template cut out of the middle of the sample project's clips.
fn rich_template() -> Template {
    let p = rich_project();
    let range = Region::between(Time::seconds(11), Time::seconds(18));
    Template::capture(&p, p.active_sequence, range).expect("the stretch has clips")
}

fn read_template(version: u32) -> Template {
    let text = std::fs::read_to_string(template_sample_path(version)).expect("the template sample is there");
    templates::from_str(&text).expect("the template sample opens")
}

#[test]
fn template_version_1_sample_opens() {
    let template = read_template(1);
    assert_eq!(template.length, Time::seconds(7));
    assert_eq!(template.clip_count(), 2);
    let picture = &template.video_tracks[0].clips[0];
    assert_eq!(
        picture.source_in,
        Rate::NTSC_60.frame_start(90) + Time::seconds(10)
    );
    assert_eq!(picture.link, template.audio_tracks[0].clips[0].link);
    assert_eq!(template.media[0].info.audio.len(), 2);
    assert_eq!(
        picture.look,
        project::Look::PLAIN,
        "saved before clips had a look"
    );
}

/// The version 1 sample was written before clips had a `look`, a `sound`,
/// `keyframes`, a `ramp` or `blend`, and before media had a
/// `stated_length`. Apart from
/// that, it is the template the model gives today.
#[test]
fn the_stored_template_matches_the_model_but_for_what_came_later() {
    // Cut from the project as it was then: clips at their one speed (a
    // ramp moves where in the file a cut clip begins).
    let mut p = rich_project();
    for sequence in p.sequences.iter_mut() {
        let tracks = sequence
            .video_tracks
            .iter_mut()
            .chain(sequence.audio_tracks.iter_mut());
        for clip in tracks.flat_map(|t| t.clips.iter_mut()) {
            clip.ramp = project::Ramp::default();
        }
    }
    let range = Region::between(Time::seconds(11), Time::seconds(18));
    let mut expected = Template::capture(&p, p.active_sequence, range).expect("the stretch has clips");
    let tracks = expected
        .video_tracks
        .iter_mut()
        .chain(expected.audio_tracks.iter_mut());
    for clip in tracks.flat_map(|t| t.clips.iter_mut()) {
        clip.look = project::Look::PLAIN;
        clip.sound = project::Sound::PLAIN;
        clip.keyframes = project::Keyframes::default();
        clip.ramp = project::Ramp::default();
        clip.blend = project::FrameBlend::Off;
    }
    for media in expected.media.iter_mut() {
        media.info.stated_length = None;
    }
    assert_eq!(read_template(1), expected);
}

#[test]
fn a_template_saved_now_reads_back_the_same() {
    let template = rich_template();
    assert_ne!(
        template.video_tracks[0].clips[0].look,
        project::Look::PLAIN,
        "the sample project's picture has a look, and the template keeps it"
    );
    let sound = template.audio_tracks[0].clips[0].sound;
    assert_eq!(
        (sound.gain_mdb, sound.pan),
        (-3_000, -200_000),
        "the sound's volume and pan are kept"
    );
    assert!(
        !sound.is_faded(),
        "the stretch cut both ends, and the fades with them"
    );
    let text = templates::to_string(&template).expect("the template becomes text");
    assert_eq!(templates::from_str(&text).expect("it reads back"), template);
    // A new template format gets its sample here, once.
    let path = template_sample_path(templates::FORMAT_VERSION);
    if std::env::var_os("WRITE_SAMPLES").is_some() && !path.exists() {
        std::fs::write(&path, text).expect("sample written");
    }
    assert_eq!(read_template(templates::FORMAT_VERSION).length, template.length);
}
