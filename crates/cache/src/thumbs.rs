//! Small pictures along a video file, for drawing inside timeline clips.

use media::{Decoding, Frame, VideoDecoder};
use std::path::Path;

/// Size the pictures are fitted into.
pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 90;
/// At most this many pictures per file; long files space them further apart.
const MOST: f64 = 300.0;
/// Never closer than this (seconds).
const CLOSEST: f64 = 2.0;

/// Pictures taken every [`Strip::every`] seconds from the start of a file.
#[derive(Debug, Clone)]
pub struct Strip {
    pub every: f64,
    pub frames: Vec<Frame>,
}

/// How far apart the pictures of a file `duration` seconds long are.
pub fn spacing(duration: f64) -> f64 {
    (duration / MOST).max(CLOSEST)
}

/// Reads the pictures of a file's video stream, on a worker. `cancelled` is
/// asked between pictures, and `progress` is told the share read (0 to 1)
/// after each one.
///
/// Where the file's key frames are much closer together than the pictures
/// (a long recording: one picture every 10 s or more, a key frame every
/// few seconds), each picture is the key frame nearest its moment: one
/// frame decoded per picture, instead of everything from the key frame
/// before up to the exact moment (a hundred frames or more each, in a
/// recording with key frames far apart). The picture is then up to half a
/// key frame distance off its place, a small part of the space it is drawn
/// in. Otherwise each picture is the exact frame.
pub fn strip(
    path: &Path,
    stream: u32,
    rotation: u16,
    duration: f64,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(f32),
) -> diag::Result<Strip> {
    let every = spacing(duration);
    let mut decoder = VideoDecoder::open(path, stream, rotation, Decoding::Software)?;
    let keys = decoder.key_frames().clone();
    let glances = by_key_frames(&keys, duration, every);
    if glances {
        // One frame at a time, each as soon as its packet is in.
        decoder = VideoDecoder::open_for_glances(path, stream, rotation, Decoding::Software)?;
    }
    let mut frames = Vec::new();
    let mut t = 0.0;
    while t < duration.max(0.001) && !cancelled() {
        let picture =
            match keys.nearest(t).filter(|_| glances) {
                Some(key) => decoder
                    .key_frame_picture(key, WIDTH, HEIGHT, true)
                    .and_then(|picture| match picture {
                        media::Decoded::Rgba(frame) => Ok(frame),
                        _ => decoder.frame_at(t, WIDTH, HEIGHT),
                    }),
                None => decoder.frame_at(t, WIDTH, HEIGHT),
            };
        match picture {
            Ok(f) => frames.push(f),
            // Past the last decodable picture: what was read is enough.
            Err(_) if !frames.is_empty() => break,
            Err(err) => return Err(err),
        }
        t += every;
        progress((t / duration.max(0.001)).min(1.0) as f32);
    }
    Ok(Strip { every, frames })
}

/// Whether a strip with a picture every `every` seconds is taken from key
/// frames: listed ones, at most half that apart, so the pictures are each
/// a different key frame and at most a quarter of their spacing off.
fn by_key_frames(keys: &media::jump::KeyFrames, duration: f64, every: f64) -> bool {
    !keys.is_empty() && keys.longest_gap(duration) * 2.0 <= every
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_files_with_close_key_frames_are_read_from_key_frames() {
        let keys = media::jump::KeyFrames::new((0..900).map(|k| f64::from(k) * 4.0).collect(), 0.0);
        assert!(by_key_frames(&keys, 3_600.0, spacing(3_600.0)));
        assert!(
            !by_key_frames(&keys, 60.0, spacing(60.0)),
            "a short file: exact frames"
        );
        assert!(
            !by_key_frames(&media::jump::KeyFrames::default(), 3_600.0, 12.0),
            "none listed"
        );
        assert_eq!(keys.nearest(9.0), Some(8.0));
        assert_eq!(keys.nearest(11.0), Some(12.0));
    }

    #[test]
    fn spacing_grows_for_long_files() {
        assert_eq!(spacing(60.0), 2.0);
        assert_eq!(spacing(3_000.0), 10.0);
    }

    #[test]
    fn reads_a_strip_of_a_short_file() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let info = media::probe(&fixture).unwrap();
        let index = info.info.video.as_ref().unwrap().index;
        let told = std::cell::RefCell::new(Vec::new());
        let s = strip(
            &fixture,
            index,
            0,
            info.duration.as_secs_f64(),
            || false,
            |done| told.borrow_mut().push(done),
        )
        .unwrap();
        assert!(!s.frames.is_empty());
        let told = told.into_inner();
        assert_eq!(told.len(), s.frames.len(), "told after every picture");
        assert!(told.windows(2).all(|w| w[0] <= w[1]) && told.last() == Some(&1.0));
        assert!(s.frames.iter().all(|f| f.width <= WIDTH && f.height <= HEIGHT));
    }
}
