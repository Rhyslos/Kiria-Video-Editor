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
        lines: Default::default(),
        effects: Default::default(),
        mode: Default::default(),
        transition: None,
        title: None,
        grade: Default::default(),
        fades: project::fades::PictureFades::NONE,
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
    // A soft vignette, and a harder one switched off (effects, format 19).
    let (effects, _) = project::Effects::default().with_added(project::EffectKind::ALL[0]);
    let (mut effects, _) = effects.with_added(project::EffectKind::Vignette(project::effects::Vignette {
        amount: 800_000,
        size: 400_000,
        softness: 200_000,
    }));
    effects.0[1].off = true;
    // Grain, sharpen and glow (format 20).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[1]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[2]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[3]);
    // A blurred box and the whole picture pixelated, switched off (format 21).
    let (effects, _) = effects.with_added(project::EffectKind::Blur(project::effects::Blur {
        radius: 700_000,
        spot: project::effects::Spot::MIDDLE,
    }));
    let (mut effects, _) = effects.with_added(project::EffectKind::ALL[5]);
    effects.0[6].off = true;
    // A zoom in on the top right, a light shake and a pan (format 22).
    let (effects, _) = effects.with_added(project::EffectKind::Zoom(project::effects::Zoom {
        amount: 500_000,
        x: 800_000,
        y: 200_000,
        sharpen: 0,
    }));
    let (effects, _) = effects.with_added(project::EffectKind::ALL[7]);
    let (mut effects, _) = effects.with_added(project::EffectKind::ALL[8]);
    // The blurred box moves right over the first two seconds (format 24).
    let second = Time::from_ratio(1, 1).expect("a second");
    let near = |at: Time| (at, at + Time::from_ratio(1, 60).expect("a frame"));
    effects.0[5].keys.set(1, Time::ZERO, near(Time::ZERO), 100_000);
    effects.0[5]
        .keys
        .set(1, second + second, near(second + second), 500_000);
    // Mirrored, its corners rounded, a gold outline and a drop shadow that
    // falls further over the first second (format 35).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[10]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[11]);
    let (effects, _) = effects.with_added(project::EffectKind::Outline(project::effects::Outline {
        width: 250_000,
        colour: project::effects::Paint::from_rgb([240, 196, 92]),
    }));
    let (mut effects, shadow) = effects.with_added(project::EffectKind::ALL[13]);
    if let Some(at) = effects.0.iter().position(|e| e.id == shadow) {
        effects.0[at].keys.set(2, Time::ZERO, near(Time::ZERO), 0);
        effects.0[at].keys.set(2, second, near(second), 300_000);
    }
    // Keyed off a green screen, in a cinema shape (format 36).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[14]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[15]);
    // Half black and white, a duotone, its colours split (format 37).
    let (effects, _) = effects.with_added(project::EffectKind::Mono(project::effects::Amount {
        amount: 500_000,
    }));
    let (effects, _) = effects.with_added(project::EffectKind::ALL[18]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[20]);
    // Smeared sideways, zoom-blurred and glitching (format 38).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[21]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[22]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[23]);
    // Through a fisheye, tiled and folded into a kaleidoscope (format 39).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[24]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[25]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[26]);
    // Posterised, in halftone dots, on an old screen (format 40).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[27]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[28]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[29]);
    // Light leaking in, a flare, speed lines and an impact frame (format 41).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[30]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[31]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[32]);
    let (effects, _) = effects.with_added(project::EffectKind::ALL[33]);
    // Motion blur, and a zoom that keeps it sharp (format 44).
    let (effects, _) = effects.with_added(project::EffectKind::ALL[34]);
    let (effects, _) = effects.with_added(project::EffectKind::Zoom(project::effects::Zoom {
        sharpen: 600_000,
        ..project::effects::Zoom::USUAL
    }));
    // Graded on the Color page: warmer and a stop down, its shadows
    // lifted a little towards blue (format 28).
    let mut grade = project::Grade::default();
    let base = &mut grade.layer_mut(0).primary;
    base.exposure = -1_000_000;
    base.temperature = 200_000;
    base.shadows_wheel = project::Wheel {
        red: 0,
        green: 0,
        blue: 100_000,
        brightness: 50_000,
    };
    // A gentle S on its brightness (format 29).
    grade.0[0].curves.master = project::Curve(vec![(250_000, 220_000), (750_000, 790_000)]);
    // A second layer brightening skin inside a soft ellipse (format 30).
    let skin = grade.layer_mut(1);
    skin.name = "Face".into();
    skin.primary.exposure = 300_000;
    skin.range = Some(project::ColorRange::SKIN);
    skin.mask = Some(project::Mask::MIDDLE);
    seq.video_tracks[0].clips.push_back(Clip {
        grade,
        look,
        keyframes,
        ramp: ramp.clone(),
        blend: project::FrameBlend::Blend,
        effects,
        // Mixed as light over what is below (format 23).
        mode: project::BlendMode::Screen,
        ..clip(v_clip, 0)
    });
    // The game sound a little quieter, a little to the left, fading in and out.
    let sound = project::Sound {
        gain_mdb: -3_000,
        pan: -200_000,
        fade_in: Time::from_ratio(1, 2).expect("half a second"),
        fade_out: Rate::NTSC_60.frame_start(45),
        fade_in_curve: 0,
        fade_out_curve: 0,
    };
    // Its own volume line dips under a shout and its pan line swings
    // right, at moments of the file (format 17).
    let point = |s: i64, value: i32| project::track_sound::LinePoint {
        at: Rate::NTSC_60.frame_start(90) + Time::seconds(s),
        value,
    };
    let lines = project::ClipLines {
        volume: [point(3, 0), point(4, -9_000), point(6, 0)].into_iter().collect(),
        pan: [point(8, 0), point(10, 600_000)].into_iter().collect(),
    };
    seq.audio_tracks[0].clips.push_back(Clip {
        sound,
        ramp,
        lines,
        ..clip(a_clip, 1)
    });
    seq.audio_tracks.push_back(extra_audio);
    seq.audio_tracks[1].gain_db = -3.5;
    // The mic is dialogue, cleaned up, high-passed and compressed, with a
    // dip in its volume line; the game sound ducks under it and the mix
    // goes through the limiter (format 16).
    let mut mic = project::TrackSound {
        role: project::TrackRole::Dialogue,
        pan: 100_000,
        volume_line: [(0, 0), (5, -6_000), (6, 0)]
            .into_iter()
            .map(|(s, value)| project::track_sound::LinePoint {
                at: Time::seconds(s),
                value,
            })
            .collect(),
        ..Default::default()
    };
    mic.high_pass.on = true;
    mic.compressor.on = true;
    mic.eq.on = true;
    mic.eq.bands[3].gain_mdb = 2_500;
    mic.cleanup.on = true;
    mic.cleanup.profile = Some(project::track_sound::NoiseProfile {
        levels_mdb: (0..64).map(|b| -95_000 + b * 250).collect(),
    });
    seq.audio_tracks[1].sound = mic;
    seq.audio_tracks[0].sound.role = project::TrackRole::Effects;
    seq.audio_tracks[0].sound.ducking.on = true;
    // The game sound a little lower (format 45).
    seq.audio_tracks[0].sound.pitch = project::track_sound::Pitch {
        on: true,
        cents: -300,
        mix_milli: 1_000,
    };
    // The game sound in a room, with an echo, through a radio (format 43).
    seq.audio_tracks[0].sound.reverb = project::track_sound::Reverb {
        on: true,
        size_milli: 700,
        ..Default::default()
    };
    seq.audio_tracks[0].sound.echo = project::track_sound::Echo {
        on: true,
        ping_pong: true,
        ..Default::default()
    };
    seq.audio_tracks[0].sound.distortion = project::track_sound::Distortion {
        on: true,
        style: project::track_sound::DistortionStyle::Radio,
        ..Default::default()
    };
    seq.master_sound.limiter.on = true;
    seq.labels.push_back(TimelineLabel {
        id: label,
        name: "First kill".into(),
        at: Time::seconds(24),
        color: LabelColor::Green,
        until: None,
    });
    seq.region = Region::between(Time::seconds(20), Time::seconds(50));
    seq.edit_needle = Time::seconds(81);

    // A second timeline holding a copy of the picture, played in the first
    // from 30 s on a track of its own, and an adjustment layer above
    // that warms everything from 21 s for 3 s (format 12).
    let main = p.active_sequence;
    let mut short = p.new_sequence("Short cut", SequenceSettings::VERTICAL_SHORT);
    let inner_clip = p.allocate_id();
    // Steadied, its view keyed by an analysis (format 27).
    let (steadied, steady_id) = project::Effects::default()
        .with_added(project::EffectKind::Stabilise(project::effects::Stabilise::USUAL));
    let mut steadied = steadied;
    let view = [
        (Time::ZERO, (500_000, 500_000)),
        (Time::seconds(1), (510_000, 495_000)),
    ];
    if let Some(at) = steadied.0.iter().position(|e| e.id == steady_id) {
        steadied.0[at] = steadied.0[at].keyed_pair(project::effects::Stabilise::MIDDLE, &view);
    }
    short.video_tracks[0].clips.push_back(Clip {
        start: Time::ZERO,
        duration: Time::seconds(4),
        link: None,
        effects: steadied,
        ..clip(inner_clip, 0)
    });
    // A second shot dissolving in over a second across its cut (format 25).
    let second_shot = p.allocate_id();
    short.video_tracks[0].clips.push_back(Clip {
        start: Time::seconds(4),
        source_in: Time::seconds(40),
        duration: Time::seconds(3),
        link: None,
        transition: Some(project::Transition {
            kind: project::TransitionKind::Dissolve,
            length: Time::seconds(1),
        }),
        ..clip(second_shot, 0)
    });
    // A third whipped in across its cut (format 42).
    let third_shot = p.allocate_id();
    short.video_tracks[0].clips.push_back(Clip {
        start: Time::seconds(7),
        source_in: Time::seconds(60),
        duration: Time::seconds(2),
        link: None,
        transition: Some(project::Transition {
            kind: project::TransitionKind::WhipPan,
            length: Time::from_ratio(1, 2).expect("half a second"),
        }),
        ..clip(third_shot, 0)
    });
    let (nested_id, nested_clip, adjustment) = (short.id, p.allocate_id(), p.allocate_id());
    let lower_third = p.allocate_id();
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
    // A yellow lower third from 40 s for 4 s, no band (format 26).
    let mut words = project::Title::new(project::TitleStyle::LowerThird);
    words.text = "Sam\nCasting the final".into();
    words.colour = [255, 220, 0];
    words.backing = false;
    v2.clips.push_back(Clip {
        media: project::MediaId::NONE,
        start: Time::seconds(40),
        source_in: Time::ZERO,
        duration: Time::seconds(4),
        link: None,
        kind: project::ClipKind::Title,
        title: Some(Box::new(words)),
        ..clip(lower_third, 0)
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
    let p = project::Command::SetMediaNote {
        media: video,
        note: "Clutch round at 4:10".into(),
    }
    .apply(&p)
    .expect("a note");
    // Its marked part and a stream's level are kept (format 15).
    let stream = p.media[&video].info.audio[0].index;
    let p = project::Command::SetMediaUses {
        uses: vec![(
            video,
            project::MediaUse {
                mark_in: Some(Time::seconds(250)),
                mark_out: Some(Time::seconds(262)),
                levels: [(stream, -3_000)].into_iter().collect(),
            },
        )],
    }
    .apply(&p)
    .expect("marks");
    // A labelled stretch: the region labelled as a whole (format 31). Last,
    // so the ids of everything above stay as older samples have them.
    let sequence = p.active_sequence;
    project::Command::AddLabel {
        sequence,
        label: TimelineLabel {
            id: project::LabelId(0),
            name: "Clutch round".into(),
            at: Time::seconds(30),
            color: LabelColor::Orange,
            until: Some(Time::seconds(45)),
        },
    }
    .apply(&p)
    .expect("a labelled stretch");
    // The first picture fades in on a curve and the first sound fades out
    // on one (format 32).
    let seq = p.active().expect("an open timeline");
    let picture = seq.video_tracks[0].clips[0].id;
    let sound_clip = seq.audio_tracks[0].clips[0].clone();
    let p = project::Command::SetFades {
        sequence,
        fades: vec![(
            picture,
            project::fades::PictureFades {
                fade_in: Time::seconds(1),
                fade_in_curve: 400_000,
                ..project::fades::PictureFades::NONE
            },
        )],
    }
    .apply(&p)
    .expect("picture fades");
    let p = project::Command::SetSound {
        sequence,
        sounds: vec![(
            sound_clip.id,
            project::Sound {
                fade_out_curve: -500_000,
                ..sound_clip.sound
            },
        )],
    }
    .apply(&p)
    .expect("a curved sound fade");
    // The first audio track in a colour of its own (format 33).
    let first_audio = p.active().expect("an open timeline").audio_tracks[0].id;
    let p = project::Command::SetTrackColour {
        sequence,
        track: first_audio,
        colour: Some(LabelColor::Purple),
    }
    .apply(&p)
    .expect("a coloured track");
    // Its sync lock off (format 34).
    project::Command::SetTrackSync {
        sequence,
        track: first_audio,
        synced: false,
    }
    .apply(&p)
    .expect("a track out of sync lock")
}

/// The header read from the text before the project gives what reading
/// the whole file gives, on every sample.
#[test]
fn every_sample_header_reads_the_same_from_its_head() {
    #[derive(serde::Deserialize)]
    struct Whole {
        format_version: u32,
        #[serde(default)]
        journal_seq: u64,
    }
    let tmp = tempfile::tempdir().expect("a temporary folder");
    for version in 1..=FORMAT_VERSION {
        let text = std::fs::read_to_string(sample_path(version))
            .expect("the sample is there")
            .replacen("journal_seq: 0,", &format!("journal_seq: {},", 100 + version), 1);
        let path = tmp.path().join(format!("v{version}.project"));
        std::fs::write(&path, &text).expect("written");
        let whole: Whole = ron::from_str(&text).expect("the whole sample parses");
        assert_eq!(
            file::read_journal_seq(&path).expect("read"),
            whole.journal_seq,
            "v{version}"
        );
        assert_eq!(
            file::read_info(&path).expect("read").format_version,
            whole.format_version,
            "v{version}"
        );
        let (project, seq) = file::load_with_journal_seq(&path).expect("opens");
        assert_eq!(seq, whole.journal_seq, "v{version}");
        assert_eq!(project, file::from_str(&text).expect("opens"), "v{version}");
    }
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
fn version_15_sample_opens_with_every_effect_off() {
    let project = file::load(&sample_path(15)).expect("the version 15 sample opens");
    for seq in &project.sequences {
        assert_eq!(seq.master_sound, project::MasterSound::default());
        for track in seq.tracks() {
            assert_eq!(
                track.sound,
                project::TrackSound::default(),
                "format 15 had no track sound"
            );
        }
    }
    assert_eq!(
        project.active().expect("a timeline").audio_tracks[1].gain_db,
        -3.5
    );
}

#[test]
fn version_16_sample_opens_with_no_clip_lines() {
    let project = file::load(&sample_path(16)).expect("the version 16 sample opens");
    for seq in &project.sequences {
        for track in seq.tracks() {
            assert!(
                track.clips.iter().all(|c| c.lines.is_empty()),
                "format 16 had no clip lines"
            );
        }
    }
    assert!(project.active().expect("a timeline").master_sound.limiter.on);
}

#[test]
fn version_18_sample_opens_with_no_effects() {
    let project = file::load(&sample_path(18)).expect("the version 18 sample opens");
    for seq in &project.sequences {
        assert!(seq.tracks().all(|t| t.clips.iter().all(|c| c.effects.is_empty())));
    }
}

#[test]
fn version_19_sample_opens_with_vignettes_only() {
    let project = file::load(&sample_path(19)).expect("the version 19 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(
        clip.effects
            .iter()
            .all(|e| matches!(e.kind, project::EffectKind::Vignette(_)))
    );
    assert_eq!(clip.effects.iter().count(), 2);
}

#[test]
fn version_20_sample_opens_without_blur_or_pixelate() {
    let project = file::load(&sample_path(20)).expect("the version 20 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert_eq!(clip.effects.iter().count(), 5);
}

#[test]
fn version_21_sample_opens_without_motion_effects() {
    let project = file::load(&sample_path(21)).expect("the version 21 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert_eq!(clip.effects.iter().count(), 7);
}

#[test]
fn version_22_sample_opens_with_every_picture_over() {
    let project = file::load(&sample_path(22)).expect("the version 22 sample opens");
    for seq in &project.sequences {
        assert!(
            seq.tracks()
                .all(|t| t.clips.iter().all(|c| c.mode == project::BlendMode::Normal))
        );
    }
}

#[test]
fn version_23_sample_opens_with_still_effects() {
    let project = file::load(&sample_path(23)).expect("the version 23 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(clip.effects.iter().all(|e| e.keys.is_empty()));
}

#[test]
fn version_24_sample_opens_with_plain_cuts() {
    let project = file::load(&sample_path(24)).expect("the version 24 sample opens");
    for seq in project.sequences.iter() {
        assert!(
            seq.tracks()
                .all(|t| t.clips.iter().all(|c| c.transition.is_none()))
        );
    }
}

#[test]
fn version_25_sample_opens_without_titles() {
    let project = file::load(&sample_path(25)).expect("the version 25 sample opens");
    for seq in project.sequences.iter() {
        assert!(
            seq.tracks()
                .all(|t| t.clips.iter().all(|c| !c.is_title() && c.title.is_none()))
        );
    }
}

#[test]
fn version_26_sample_opens_without_stabilise() {
    let project = file::load(&sample_path(26)).expect("the version 26 sample opens");
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| e.kind.name() != "Stabilise")
        );
    }
}

#[test]
fn the_current_sample_has_a_steadied_shot() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let short = project
        .sequences
        .iter()
        .find(|s| s.name == "Short cut")
        .expect("the second timeline");
    let steady = short.video_tracks[0].clips[0]
        .effects
        .iter()
        .next()
        .expect("an effect");
    assert_eq!(steady.kind.name(), "Stabilise");
    assert_eq!(steady.keys.of(project::effects::Stabilise::MIDDLE).len(), 2);
}

#[test]
fn version_34_sample_opens_without_the_shape_effects() {
    let project = file::load(&sample_path(34)).expect("the version 34 sample opens");
    let shapes = ["Mirror", "Rounded corners", "Outline", "Drop shadow"];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !shapes.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_a_framed_picture_with_a_shadow() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let effects: Vec<&project::Effect> = project
        .sequences
        .iter()
        .flat_map(|s| s.tracks())
        .flat_map(|t| t.clips.iter())
        .flat_map(|c| c.effects.iter())
        .collect();
    let named = |name: &str| effects.iter().find(|e| e.kind.name() == name).copied();
    assert!(named("Mirror").is_some());
    assert!(named("Rounded corners").is_some());
    match named("Outline").map(|e| e.kind) {
        Some(project::EffectKind::Outline(o)) => assert_eq!(o.colour.rgb(), [240, 196, 92]),
        other => panic!("no outline: {other:?}"),
    }
    let shadow = named("Drop shadow").expect("a drop shadow");
    assert_eq!(shadow.keys.of(2).len(), 2, "its distance is keyed");
}

#[test]
fn version_35_sample_opens_without_a_key_or_letterbox() {
    let project = file::load(&sample_path(35)).expect("the version 35 sample opens");
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !["Chroma key", "Letterbox"].contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_a_keyed_picture_in_a_cinema_shape() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let kinds: Vec<project::EffectKind> = clip.effects.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&project::EffectKind::Key(project::effects::Key::USUAL)));
    match kinds
        .iter()
        .find(|k| matches!(k, project::EffectKind::Letterbox(_)))
    {
        Some(project::EffectKind::Letterbox(l)) => assert!((l.width_per_height() - 2.39).abs() < 0.001),
        other => panic!("no letterbox: {other:?}"),
    }
}

#[test]
fn version_36_sample_opens_without_the_colour_looks() {
    let project = file::load(&sample_path(36)).expect("the version 36 sample opens");
    let looks = [
        "Black and white",
        "Sepia",
        "Duotone",
        "Invert",
        "Chromatic aberration",
    ];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !looks.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_colour_looks() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let kinds: Vec<project::EffectKind> = clip.effects.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&project::EffectKind::Mono(project::effects::Amount {
            amount: 500_000
        }))
    );
    assert!(kinds.contains(&project::EffectKind::Duotone(project::effects::Duotone::USUAL)));
    assert!(kinds.contains(&project::EffectKind::Split(project::effects::Split::USUAL)));
}

#[test]
fn version_37_sample_opens_without_the_motion_blurs_or_glitch() {
    let project = file::load(&sample_path(37)).expect("the version 37 sample opens");
    let new = ["Directional blur", "Radial blur", "Glitch"];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !new.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_blurs_that_move_and_a_glitch() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let names: Vec<&str> = clip.effects.iter().map(|e| e.kind.name()).collect();
    for name in ["Directional blur", "Radial blur", "Glitch"] {
        assert!(names.contains(&name), "{name}");
    }
}

#[test]
fn version_38_sample_opens_without_lens_mosaic_or_kaleidoscope() {
    let project = file::load(&sample_path(38)).expect("the version 38 sample opens");
    let new = ["Lens distortion", "Mosaic", "Kaleidoscope"];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !new.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_a_fisheye_a_mosaic_and_a_kaleidoscope() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let kinds: Vec<project::EffectKind> = clip.effects.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&project::EffectKind::Lens(project::effects::Lens::USUAL)));
    assert!(kinds.contains(&project::EffectKind::Tile(project::effects::Tile::USUAL)));
    assert!(kinds.contains(&project::EffectKind::Kaleidoscope(
        project::effects::Kaleidoscope::USUAL
    )));
}

#[test]
fn version_39_sample_opens_without_the_print_looks() {
    let project = file::load(&sample_path(39)).expect("the version 39 sample opens");
    let new = ["Posterise", "Halftone", "Scanlines"];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !new.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_is_posterised_in_dots_on_an_old_screen() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let names: Vec<&str> = clip.effects.iter().map(|e| e.kind.name()).collect();
    for name in ["Posterise", "Halftone", "Scanlines"] {
        assert!(names.contains(&name), "{name}");
    }
}

#[test]
fn version_40_sample_opens_without_the_light_and_action_effects() {
    let project = file::load(&sample_path(40)).expect("the version 40 sample opens");
    let new = ["Light leak", "Lens flare", "Speed lines", "Impact frame"];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(
            clips
                .flat_map(|c| c.effects.iter())
                .all(|e| !new.contains(&e.kind.name()))
        );
    }
}

#[test]
fn the_current_sample_has_light_and_action_effects() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let names: Vec<&str> = clip.effects.iter().map(|e| e.kind.name()).collect();
    for name in ["Light leak", "Lens flare", "Speed lines", "Impact frame"] {
        assert!(names.contains(&name), "{name}");
    }
}

#[test]
fn version_42_sample_has_no_distortion_echo_or_reverb() {
    let project = file::load(&sample_path(42)).expect("the version 42 sample opens");
    for seq in project.sequences.iter() {
        for t in seq.audio_tracks.iter() {
            assert!(!t.sound.distortion.on && !t.sound.echo.on && !t.sound.reverb.on);
        }
    }
}

#[test]
fn the_current_sample_has_game_sound_through_a_radio_in_a_room() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let game = &project.active().expect("a timeline").audio_tracks[0].sound;
    assert_eq!(
        game.distortion.style,
        project::track_sound::DistortionStyle::Radio
    );
    assert!(game.echo.on && game.echo.ping_pong);
    assert_eq!((game.reverb.on, game.reverb.size_milli), (true, 700));
}

#[test]
fn version_44_sample_has_no_pitch_shift() {
    let project = file::load(&sample_path(44)).expect("the version 44 sample opens");
    for seq in project.sequences.iter() {
        for t in seq.audio_tracks.iter() {
            assert!(!t.sound.pitch.on);
        }
    }
}

#[test]
fn the_current_sample_has_its_game_sound_lower() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let game = &project.active().expect("a timeline").audio_tracks[0].sound;
    assert_eq!((game.pitch.on, game.pitch.cents), (true, -300));
}

#[test]
fn version_43_sample_opens_without_motion_blur_and_zooms_without_sharpening() {
    let project = file::load(&sample_path(43)).expect("the version 43 sample opens");
    for seq in project.sequences.iter() {
        for effect in seq
            .tracks()
            .flat_map(|t| t.clips.iter())
            .flat_map(|c| c.effects.iter())
        {
            assert_ne!(effect.kind.name(), "Motion blur");
            if let project::EffectKind::Zoom(z) = effect.kind {
                assert_eq!(z.sharpen, 0);
            }
        }
    }
}

#[test]
fn the_current_sample_has_motion_blur_and_a_sharpened_zoom() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(clip.effects.iter().any(|e| e.kind.name() == "Motion blur"));
    assert!(
        clip.effects
            .iter()
            .any(|e| matches!(e.kind, project::EffectKind::Zoom(z) if z.sharpen == 600_000))
    );
}

#[test]
fn version_27_sample_opens_ungraded() {
    let project = file::load(&sample_path(27)).expect("the version 27 sample opens");
    for seq in project.sequences.iter() {
        assert!(seq.tracks().all(|t| t.clips.iter().all(|c| !c.grade.is_graded())));
    }
}

#[test]
fn version_29_sample_opens_with_layers_everywhere() {
    let project = file::load(&sample_path(29)).expect("the version 29 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(
        clip.grade
            .0
            .iter()
            .all(|layer| layer.range.is_none() && layer.mask.is_none())
    );
}

#[test]
fn the_current_sample_has_a_layer_on_skin_in_an_ellipse() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let face = &clip.grade.0[1];
    assert_eq!(face.name, "Face");
    assert_eq!(face.range, Some(project::ColorRange::SKIN));
    assert_eq!(face.mask, Some(project::Mask::MIDDLE));
}

#[test]
fn version_28_sample_opens_with_straight_curves() {
    let project = file::load(&sample_path(28)).expect("the version 28 sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(clip.grade.0.iter().all(|layer| layer.curves.is_plain()));
}

#[test]
fn the_current_sample_has_a_graded_picture() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert!(clip.grade.is_graded());
    assert_eq!(clip.grade.0[0].primary.exposure, -1_000_000);
    assert!(clip.grade.check().is_ok());
    assert!(!clip.grade.0[0].curves.is_plain(), "an S curve");
}

#[test]
fn the_current_sample_has_a_lower_third() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let seq = project.active().expect("a timeline");
    let title = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.is_title())
        .expect("a title");
    assert_eq!(project.clip_name(title), "Title: Sam");
    assert_eq!(title.title.as_ref().map(|t| t.colour), Some([255, 220, 0]));
}

#[test]
fn the_current_sample_has_a_dissolve() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let short = project
        .sequences
        .iter()
        .find(|s| s.name == "Short cut")
        .expect("the second timeline");
    let track = &short.video_tracks[0];
    let across = project::transitions::across(track, Time::seconds(4)).expect("a transition at the cut");
    assert_eq!(across.kind, project::TransitionKind::Dissolve);
    assert_eq!(across.done, 0.5);
    let whip = project::transitions::across(track, Time::seconds(7)).expect("a whip pan at the next cut");
    assert_eq!(whip.kind, project::TransitionKind::WhipPan);
}

#[test]
fn version_41_sample_has_none_of_the_new_transitions() {
    let project = file::load(&sample_path(41)).expect("the version 41 sample opens");
    let new = &project::TransitionKind::ALL[5..];
    for seq in project.sequences.iter() {
        let clips = seq.tracks().flat_map(|t| t.clips.iter());
        assert!(clips.filter_map(|c| c.transition).all(|t| !new.contains(&t.kind)));
    }
}

#[test]
fn the_current_sample_has_a_moving_blur_box() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    let blur = clip
        .effects
        .iter()
        .find(|e| !e.keys.is_empty())
        .expect("keyframes");
    assert_eq!(blur.keys.of(1).len(), 2);
}

#[test]
fn the_current_sample_has_a_picture_mixed_as_light() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert_eq!(clip.mode, project::BlendMode::Screen);
}

#[test]
fn the_current_sample_has_effects_one_switched_off() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").video_tracks[0].clips[0];
    assert_eq!(clip.effects.iter().count(), 34);
    assert_eq!(clip.effects.applied().count(), 32);
    assert!(clip.effects.check().is_ok());
}

#[test]
fn the_current_sample_has_clip_volume_and_pan_lines() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let clip = &project.active().expect("a timeline").audio_tracks[0].clips[0];
    assert_eq!((clip.lines.volume.len(), clip.lines.pan.len()), (3, 2));
    assert!(clip.lines.check().is_ok());
}

#[test]
fn the_current_sample_has_track_effects_lines_and_a_limiter() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let seq = project.active().expect("a timeline");
    let mic = &seq.audio_tracks[1].sound;
    assert_eq!(mic.role, project::TrackRole::Dialogue);
    assert!(mic.cleanup.on && mic.compressor.on && mic.high_pass.on);
    assert_eq!(mic.cleanup.profile.as_ref().map(|p| p.levels_mdb.len()), Some(64));
    assert_eq!(mic.volume_line.len(), 3);
    assert!(mic.check().is_ok());
    assert!(seq.audio_tracks[0].sound.ducking.on);
    assert!(seq.master_sound.limiter.on);
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
fn version_14_sample_opens_with_no_marks() {
    let project = file::load(&sample_path(14)).expect("the version 14 sample opens");
    assert!(project.library.uses.is_empty(), "format 14 kept no marks");
    assert!(!project.library.labels.is_empty(), "and keeps its labels");
}

#[test]
fn the_current_sample_keeps_a_marked_part_and_a_level() {
    let project = file::load(&sample_path(FORMAT_VERSION)).expect("the sample opens");
    let media = *project.media.keys().next().expect("a file");
    let used = project.library.use_of(media);
    assert_eq!(
        (used.mark_in, used.mark_out),
        (Some(Time::seconds(250)), Some(Time::seconds(262)))
    );
    assert_eq!(used.levels.values().copied().collect::<Vec<_>>(), [-3_000]);
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
        // Clip lines came with format 17, effects with 19.
        clip.lines = project::ClipLines::default();
        clip.effects = project::Effects::default();
        // Blend modes came with 23.
        clip.mode = project::BlendMode::Normal;
        // Transitions with 25, grades with 28.
        clip.transition = None;
        clip.grade = project::Grade::default();
    }
    for media in expected.media.iter_mut() {
        media.info.stated_length = None;
    }
    // Track sound came with format 16.
    for track in expected
        .video_tracks
        .iter_mut()
        .chain(expected.audio_tracks.iter_mut())
    {
        track.sound = project::TrackSound::default();
        // Track colours came with format 33, sync locks with 34.
        track.colour = None;
        track.unsynced = false;
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
