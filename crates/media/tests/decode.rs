//! Decoding real files: the frame on screen at a time, scaled and upright.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::unwrap_used)]

use media::{Decoding, VideoDecoder};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn open(name: &str, rotation: u16) -> VideoDecoder {
    VideoDecoder::open(&fixture(name), 0, rotation, Decoding::Software).unwrap()
}

#[test]
fn the_frame_on_screen_at_a_time() {
    let mut d = open("game-3-audio.mkv", 0);
    let f = d.frame_at(1.0, 4000, 4000).unwrap();
    assert_eq!((f.width, f.height), (320, 180), "never enlarged");
    assert_eq!(f.rgba.len(), 320 * 180 * 4);
    assert!((f.time - 1.0).abs() < 0.02, "the frame at 1 s, got {}", f.time);
    // Between two frames: the earlier one stays on screen.
    let g = d.frame_at(1.01, 4000, 4000).unwrap();
    assert!(g.time <= 1.01 && g.time > 0.98, "{}", g.time);
    assert_ne!(
        f.rgba,
        d.frame_at(1.5, 4000, 4000).unwrap().rgba,
        "the picture moves"
    );
}

#[test]
fn jumping_back_and_far_ahead() {
    let mut d = open("game-3-audio.mkv", 0);
    let late = d.frame_at(1.8, 160, 160).unwrap();
    let early = d.frame_at(0.2, 160, 160).unwrap();
    assert!((early.time - 0.2).abs() < 0.02, "{}", early.time);
    assert!((late.time - 1.8).abs() < 0.02, "{}", late.time);
    // The same moment gives the same picture whichever way it was reached.
    let again = d.frame_at(1.8, 160, 160).unwrap();
    assert_eq!(late.rgba, again.rgba);
    assert_eq!((late.width, late.height), (160, 90), "scaled to fit, aspect kept");
    // Past the end: the last frame stays.
    let end = d.frame_at(99.0, 160, 160).unwrap();
    assert!(end.time > 1.9, "{}", end.time);
}

#[test]
fn rotated_video_comes_out_upright() {
    let mut d = open("phone-rotated.mp4", 270);
    let f = d.frame_at(0.5, 1000, 1000).unwrap();
    assert_eq!((f.width, f.height), (180, 320));
}

#[test]
fn variable_frame_rate_frames_are_found_by_time() {
    let mut d = open("vfr-60.mkv", 0);
    // Frame 7 of every 15 is missing: at 7/60 s the previous frame is still showing.
    let f = d.frame_at(7.0 / 60.0 + 0.001, 160, 160).unwrap();
    assert!((f.time - 6.0 / 60.0).abs() < 0.002, "{}", f.time);
    let g = d.frame_at(8.0 / 60.0 + 0.001, 160, 160).unwrap();
    assert!((g.time - 8.0 / 60.0).abs() < 0.002, "{}", g.time);
}

#[test]
fn best_decoding_falls_back_when_there_is_no_gpu() {
    let mut d = VideoDecoder::open(&fixture("game-3-audio.mkv"), 0, 0, Decoding::Best).unwrap();
    assert!(d.frame_at(0.5, 320, 320).is_ok(), "decodes with {}", d.method);
}
