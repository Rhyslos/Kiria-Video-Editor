//! Writing a file and reading it back with the crate's own probe and decoder.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use media::encode::{AudioSettings, VideoSettings, Writer};
use media::{Decoded, Decoding, VideoDecoder};

const SIZE: (u32, u32) = (320, 180);

/// A grey picture whose brightness says which frame it is.
fn picture(frame: u32) -> Vec<u8> {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut nv12 = vec![(40 + frame * 6) as u8; w * h];
    nv12.extend(std::iter::repeat_n(128u8, w * h / 2));
    nv12
}

fn settings() -> VideoSettings {
    VideoSettings {
        width: SIZE.0,
        height: SIZE.1,
        rate: (30, 1),
        bit_rate: 2_000_000,
        // Whatever machine runs this has a CPU.
        software: true,
    }
}

#[test]
fn a_written_file_has_its_pictures_and_sound_where_they_belong() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.mp4");
    let sound = AudioSettings {
        sample_rate: 48_000,
        bit_rate: 128_000,
    };
    let mut writer = Writer::create(&path, &settings(), Some(&sound)).unwrap();
    assert_eq!(writer.video_encoder(), "libx264");
    assert!(!writer.on_graphics_card());
    // One second: 30 pictures, and a tone handed over in uneven pieces.
    let tone: Vec<f32> = (0..48_000)
        .flat_map(|i| {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            [s, s]
        })
        .collect();
    let mut pieces = tone.chunks(2 * 1601);
    for frame in 0..30 {
        writer.write_picture(&picture(frame)).unwrap();
        if let Some(piece) = pieces.next() {
            writer.write_sound(piece).unwrap();
        }
    }
    for piece in pieces {
        writer.write_sound(piece).unwrap();
    }
    writer.finish().unwrap();

    let media = media::describe(&path).unwrap();
    let video = media.info.video.expect("a picture stream");
    assert_eq!((video.width, video.height), SIZE);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.frame_rate.nominal_fps(), 30);
    assert_eq!(media.info.audio.len(), 1);
    assert_eq!((media.info.audio[0].sample_rate, media.info.audio[0].channels), (48_000, 2));
    let seconds = media.duration.as_secs_f64();
    assert!((seconds - 1.0).abs() < 0.08, "{seconds} s");

    // Frame 20 is where frame 20 was put, and as bright as it was made.
    let mut decoder = VideoDecoder::open(&path, video.index, 0, Decoding::Software).unwrap();
    let Decoded::Yuv(frame) = decoder.picture_at(20.0 / 30.0, SIZE.0, SIZE.1).unwrap() else {
        panic!("planes expected");
    };
    assert!((frame.time - 20.0 / 30.0).abs() < 0.001, "at {}", frame.time);
    let luma = frame.into_planes().as_ref()[(SIZE.0 * 90 + 160) as usize];
    assert!(luma.abs_diff(40 + 20 * 6) <= 4, "brightness {luma}");
}

#[test]
fn a_file_without_sound_and_pictures_of_the_wrong_size_are_handled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent.mp4");
    let mut writer = Writer::create(&path, &settings(), None).unwrap();
    writer.write_sound(&[0.0; 64]).unwrap(); // nowhere to go: ignored
    assert!(writer.write_picture(&[0u8; 100]).is_err());
    for frame in 0..5 {
        writer.write_picture(&picture(frame)).unwrap();
    }
    writer.finish().unwrap();
    let media = media::describe(&path).unwrap();
    assert!(media.info.audio.is_empty());
    assert!(media.info.video.is_some());

    // Odd sizes can't hold colour at half size.
    let odd = VideoSettings {
        width: 321,
        ..settings()
    };
    let err = Writer::create(&dir.path().join("odd.mp4"), &odd, None).unwrap_err();
    assert!(err.report().contains("even"), "{}", err.report());
    // A folder that isn't there.
    let nowhere = dir.path().join("missing").join("out.mp4");
    assert!(Writer::create(&nowhere, &settings(), None).is_err());
}

#[test]
fn the_cpu_encoder_is_always_among_those_present() {
    assert!(media::encode::video_encoders_present().contains(&"libx264"));
    assert!(media::encode::encoder_description("libx264").is_some());
}
