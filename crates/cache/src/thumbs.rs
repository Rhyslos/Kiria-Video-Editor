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

/// Reads the pictures of a file's video stream. Slow (one seek per picture):
/// run it on a worker. `cancelled` is asked between pictures, and `progress`
/// is told the share read (0 to 1) after each one.
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
    let mut frames = Vec::new();
    let mut t = 0.0;
    while t < duration.max(0.001) && !cancelled() {
        match decoder.frame_at(t, WIDTH, HEIGHT) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
