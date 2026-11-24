//! Probing real files: the small recordings in `tests/fixtures/`, made with
//! FFmpeg's test pattern and tone generators.

use media::{describe, probe};
use project::{MediaKind, Rate, Time};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn a_game_recording_with_three_named_sound_streams() {
    let p = probe(&fixture("game-3-audio.mkv")).unwrap();
    assert_eq!(p.kind, MediaKind::Video);
    assert_eq!(p.info.container, "matroska,webm");
    let v = p.info.video.as_ref().unwrap();
    assert_eq!((v.codec.as_str(), v.width, v.height), ("h264", 320, 180));
    assert_eq!(v.frame_rate, Rate::FPS_60);
    assert_eq!(v.rotation, 0);
    let names: Vec<&str> = p.info.audio.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Game", "Mic", "Voice chat"]);
    assert!(
        p.info
            .audio
            .iter()
            .all(|a| a.codec == "opus" && a.sample_rate == 48_000)
    );
    assert_eq!(p.info.audio[0].channels, 2);
    let indices: Vec<u32> = p.info.audio.iter().map(|a| a.index).collect();
    assert_eq!(indices, [1, 2, 3]);
    let secs = p.duration.as_secs_f64();
    assert!((1.9..2.2).contains(&secs), "about 2 s, got {secs}");
}

#[test]
fn a_rotated_phone_clip_reports_its_turn_and_upright_size() {
    let p = probe(&fixture("phone-rotated.mp4")).unwrap();
    let v = p.info.video.unwrap();
    assert_eq!((v.width, v.height), (320, 180), "stored size");
    assert_eq!(
        v.rotation, 270,
        "the file asks for a quarter turn counter-clockwise"
    );
    assert_eq!(v.display_size(), (180, 320));
    assert_eq!(p.info.audio[0].name, "Audio 1", "unnamed streams are numbered");
}

#[test]
fn sound_only_and_still_pictures() {
    let mp3 = probe(&fixture("voice.mp3")).unwrap();
    assert_eq!(mp3.kind, MediaKind::Audio);
    assert!(mp3.info.video.is_none());
    assert_eq!(mp3.info.audio.len(), 1);

    let png = probe(&fixture("still.png")).unwrap();
    assert_eq!(png.kind, MediaKind::Image);
    assert_eq!(png.duration, Time::ZERO);
    assert_eq!(png.info.video.unwrap().codec, "png");
}

#[test]
fn files_that_are_not_media_or_missing_are_explained() {
    let err = probe(&fixture("not-media.mkv")).unwrap_err();
    assert!(err.what().contains("can't be imported"), "{}", err.report());
    let err = probe(&fixture("no-such-file.mkv")).unwrap_err();
    assert!(err.what().contains("can't be opened"), "{}", err.report());
}

#[test]
fn describe_fills_in_everything_the_project_keeps() {
    let path = fixture("game-3-audio.mkv");
    let m = describe(&path).unwrap();
    assert_eq!(m.path, path);
    assert_eq!(m.fingerprint.size, std::fs::metadata(&path).unwrap().len());
    assert_eq!(m.info.audio.len(), 3);
}

#[test]
fn variable_frame_rate_is_detected_and_shown_at_the_nearest_rate() {
    let vfr = probe(&fixture("vfr-60.mkv")).unwrap().info.video.unwrap();
    assert!(vfr.variable_frame_rate, "every 15th frame is missing");
    assert_eq!(vfr.frame_rate, Rate::FPS_60);

    let steady = probe(&fixture("game-3-audio.mkv")).unwrap().info.video.unwrap();
    assert!(!steady.variable_frame_rate);
    assert_eq!(steady.frame_rate, Rate::FPS_60);
}

#[test]
fn a_containers_crop_is_taken_off_the_size() {
    // Hardware AV1 encoders pad to steps (1920×1082, 3456×1440) and the
    // recorder writes how much to cut off; these are the same, small.
    for (name, size) in [("padded-bottom.mkv", (64, 34)), ("padded-right.mkv", (64, 32))] {
        let v = probe(&fixture(name)).unwrap().info.video.unwrap();
        assert_eq!((v.width, v.height), size, "{name}");
    }
    let right = probe(&fixture("padded-right.mkv")).unwrap().info.video.unwrap();
    assert_eq!(right.frame_rate, Rate::new(144, 1).unwrap());
}
