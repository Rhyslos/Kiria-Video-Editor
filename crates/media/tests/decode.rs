//! Decoding real files: the frame on screen at a time, scaled and upright.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::unwrap_used)]

use media::{Decoded, Decoding, Newer, Reached, VideoDecoder};
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

/// The largest difference between two pictures' colour values, and the mean.
fn difference(a: &media::Frame, b: &media::Frame) -> (u8, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut worst, mut total) = (0u8, 0u64);
    for (x, y) in a.rgba.iter().zip(&b.rgba) {
        worst = worst.max(x.abs_diff(*y));
        total += u64::from(x.abs_diff(*y));
    }
    (worst, total as f64 / a.rgba.len() as f64)
}

#[test]
fn planes_convert_to_the_same_picture_as_the_rgba_path() {
    for (name, rotation) in [
        ("game-3-audio.mkv", 0),
        ("phone-rotated.mp4", 270),
        ("vfr-60.mkv", 0),
    ] {
        let mut d = open(name, rotation);
        let rgba = d.frame_at(0.5, 4000, 4000).unwrap();
        let Decoded::Yuv(yuv) = d.picture_at(0.5, 4000, 4000).unwrap() else {
            panic!("{name}: 8-bit 4:2:0 comes as planes");
        };
        assert_eq!(yuv.time, rgba.time);
        assert_eq!(yuv.rotation, rotation);
        assert_eq!(yuv.upright_size(), (rgba.width, rgba.height));
        assert_eq!(yuv.luma().len(), (yuv.width * yuv.height) as usize);
        let (cw, ch) = yuv.chroma_size();
        assert_eq!(yuv.chroma().len(), (cw * ch * 2) as usize);
        // The scaler rounds in fixed point and takes the colour samples a
        // little differently, so single values differ by a few steps where
        // colours change sharply; the picture as a whole is the same.
        let (worst, mean) = difference(&yuv.to_rgba(), &rgba);
        assert!(mean < 1.0 && worst <= 12, "{name}: mean {mean:.3}, worst {worst}");
    }
}

#[test]
fn planes_come_smaller_when_the_box_is() {
    let mut d = open("game-3-audio.mkv", 0);
    let Decoded::Yuv(full) = d.picture_at(1.0, 320, 180).unwrap() else {
        panic!("planes");
    };
    assert_eq!((full.width, full.height), (320, 180));
    let Decoded::Yuv(half) = d.picture_at(1.0, 160, 160).unwrap() else {
        panic!("planes");
    };
    assert_eq!((half.width, half.height), (160, 90));
    assert_eq!(half.chroma_size(), (80, 45));
    // Halved by averaging: each sample is the mean of the four it replaces.
    let mean_of_four = |plane: &[u8], width: usize, x: usize, y: usize| {
        let at = |dx: usize, dy: usize| u32::from(plane[(y * 2 + dy) * width + x * 2 + dx]);
        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8
    };
    for (i, sample) in half.luma().iter().enumerate() {
        assert_eq!(
            *sample,
            mean_of_four(full.luma(), 320, i % 160, i / 160),
            "luma {i}"
        );
    }
    // Colour too, U and V each on their own (planar: U first, then V).
    let (full_u, full_v) = full.chroma().split_at(160 * 90);
    let (half_u, half_v) = half.chroma().split_at(80 * 45);
    for i in 0..80 * 45 {
        assert_eq!(half_u[i], mean_of_four(full_u, 160, i % 80, i / 80), "U {i}");
        assert_eq!(half_v[i], mean_of_four(full_v, 160, i % 80, i / 80), "V {i}");
    }
    // A box that needs more than half keeps the file's size.
    let Decoded::Yuv(kept) = d.picture_at(1.0, 200, 200).unwrap() else {
        panic!("planes");
    };
    assert_eq!((kept.width, kept.height), (320, 180));
}

#[test]
fn turned_video_keeps_its_planes_as_stored() {
    let mut d = open("phone-rotated.mp4", 270);
    let Decoded::Yuv(yuv) = d.picture_at(0.5, 1000, 1000).unwrap() else {
        panic!("planes");
    };
    assert_eq!((yuv.width, yuv.height), (320, 180), "stored on its side");
    assert_eq!(yuv.upright_size(), (180, 320));
}

/// The middle pixel of one of the three squares in `colour-709-full.mp4`.
fn square(frame: &media::Frame, index: usize) -> [u8; 3] {
    let at = (16 * frame.width as usize + 16 + 32 * index) * 4;
    [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2]]
}

#[test]
fn colours_follow_the_matrix_and_range_the_file_names() {
    // Three squares (red, green and blue at three quarters), stored as
    // BT.709 with full range in a picture small enough that guessing by
    // size would say BT.601. Made with FFmpeg's `color` sources, `hstack`
    // and `scale=out_color_matrix=bt709:out_range=pc`.
    let mut d = open("colour-709-full.mp4", 0);
    let rgba = d.frame_at(0.0, 4000, 4000).unwrap();
    let Decoded::Yuv(yuv) = d.picture_at(0.0, 4000, 4000).unwrap() else {
        panic!("planes");
    };
    assert_eq!(yuv.colour.matrix, media::Matrix::Bt709);
    assert_eq!(yuv.colour.range, media::Range::Full);
    let planes = yuv.to_rgba();
    for (index, want) in [[192, 0, 0], [0, 192, 0], [0, 0, 192]].into_iter().enumerate() {
        for (path, got) in [("RGBA", square(&rgba, index)), ("planes", square(&planes, index))] {
            let near = got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 4);
            assert!(near, "{path} path, square {index}: got {got:?}, wanted {want:?}");
        }
    }
}

#[test]
fn the_containers_key_frames_are_listed() {
    // One key frame at the start of this two-second recording.
    assert_eq!(open("game-3-audio.mkv", 0).key_frames().len(), 1);
    // This one has a second key frame, 2.15 s in.
    assert_eq!(open("vfr-60.mkv", 0).key_frames().len(), 2);
}

/// What a decode is told between frames, changing after `after` frames.
fn then(after: u32, to: Newer) -> impl FnMut() -> Newer {
    let mut asked = 0;
    move || {
        asked += 1;
        if asked > after { to } else { Newer::Nothing }
    }
}

fn shown_at(reached: Reached<media::Frame>) -> Option<(f64, f64)> {
    match reached {
        Reached::Shown { picture, at } => Some((at, picture.time)),
        Reached::Dropped => None,
    }
}

#[test]
fn a_decode_nobody_can_use_any_more_is_given_up() {
    let mut d = open("game-3-audio.mkv", 0);
    // Another file is wanted by now.
    let mut newer = then(10, Newer::Elsewhere);
    let reached = d.frame_at_while(1.8, 160, 160, &mut newer).unwrap();
    assert!(shown_at(reached).is_none());
    // Or a moment behind where the decode has got to.
    let mut d = open("game-3-audio.mkv", 0);
    d.frame_at(1.0, 160, 160).unwrap();
    let mut newer = then(10, Newer::At(0.5));
    let reached = d.frame_at_while(1.8, 160, 160, &mut newer).unwrap();
    assert!(shown_at(reached).is_none());
    // The decoder is none the worse: it gives the right frame afterwards.
    let straight = open("game-3-audio.mkv", 0).frame_at(0.5, 160, 160).unwrap();
    let after = d.frame_at(0.5, 160, 160).unwrap();
    assert_eq!(after.time, straight.time);
    assert_eq!(after.rgba, straight.rgba);
}

#[test]
fn an_earlier_moment_still_ahead_stops_the_decode_there() {
    let mut d = open("game-3-audio.mkv", 0);
    // On the way to 1.8 s, with 20 frames (a third of a second) decoded, 0.9 s is wanted.
    let mut newer = then(20, Newer::At(0.9));
    let reached = d.frame_at_while(1.8, 160, 160, &mut newer).unwrap();
    let (at, time) = shown_at(reached).unwrap();
    assert_eq!(at, 0.9);
    assert!((time - 0.9).abs() < 0.02, "{time}");
    let straight = open("game-3-audio.mkv", 0).frame_at(0.9, 160, 160).unwrap();
    assert_eq!(time, straight.time);
}

#[test]
fn a_later_moment_on_the_way_does_not_stop_the_one_before_it() {
    let mut d = open("game-3-audio.mkv", 0);
    let mut newer = then(20, Newer::At(1.9));
    let reached = d.frame_at_while(1.2, 160, 160, &mut newer).unwrap();
    let (at, time) = shown_at(reached).unwrap();
    assert_eq!(at, 1.2);
    assert!((time - 1.2).abs() < 0.02, "{time}");
    // And the later one follows without a jump: the decoder carries on.
    let (at, time) = shown_at(d.frame_at_while(1.9, 160, 160, &mut || Newer::Nothing).unwrap()).unwrap();
    assert_eq!(at, 1.9);
    assert!((time - 1.9).abs() < 0.02, "{time}");
}
