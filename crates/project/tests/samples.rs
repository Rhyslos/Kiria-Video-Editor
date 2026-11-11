//! Every released file format keeps opening.
//!
//! `tests/samples/project-v<N>.ron` holds one project saved in format N. When
//! the format changes, the old samples stay and must still open (through the
//! migrations); a new sample is added for the new version with:
//!
//! ```sh
//! WRITE_SAMPLES=1 cargo test -p project --test samples
//! ```

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used)]

use project::file::{self, FORMAT_VERSION};
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
    };
    seq.video_tracks[0].clips.push_back(clip(v_clip, 0));
    seq.audio_tracks[0].clips.push_back(clip(a_clip, 1));
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
    p
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
fn current_format_sample_matches_the_model() {
    let path = sample_path(FORMAT_VERSION);
    if std::env::var_os("WRITE_SAMPLES").is_some() {
        file::save(&rich_project(), &path).expect("sample written");
    }
    assert_eq!(file::load(&path).expect("sample opens"), rich_project());
}
