//! The project's picture for the launcher's cards: one frame of the
//! timeline, fitted into 16:9 on black, written beside the project file.

use crate::timeline::video_at;
use diag::{AppError, Explain};
use media::{Decoding, VideoDecoder};
use project::{Project, SequenceId, Time};
use std::path::Path;

/// Size of the picture written.
pub const WIDTH: u32 = 480;
pub const HEIGHT: u32 = 270;

/// The frame to show: the one at `at`, or the first frame of the first
/// clip when `at` falls in a gap. `None` with no video on the timeline.
pub fn frame_time(project: &Project, sequence: SequenceId, at: Time) -> Option<Time> {
    let seq = project.sequences.iter().find(|s| s.id == sequence)?;
    if video_at(project, seq, at).is_some() {
        return Some(at);
    }
    seq.video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| c.start)
        .min()
}

/// Writes the timeline's picture at `at` (see [`frame_time`]) to `out` as a
/// PNG. With no video on the timeline, nothing is written and `Ok(false)`
/// comes back.
pub fn write(project: &Project, sequence: SequenceId, at: Time, out: &Path) -> diag::Result<bool> {
    let Some(at) = frame_time(project, sequence, at) else {
        return Ok(false);
    };
    let Some(seq) = project.sequences.iter().find(|s| s.id == sequence) else {
        return Ok(false);
    };
    let Some(showing) = video_at(project, seq, at) else {
        return Ok(false);
    };
    let rotation = showing.media.info.video.as_ref().map_or(0, |v| v.rotation);
    // The CPU is plenty for one small frame and leaves the GPU to playback.
    let mut decoder = VideoDecoder::open(
        &showing.media.path,
        showing.clip.stream,
        rotation,
        Decoding::Software,
    )?;
    let frame = decoder.frame_at(showing.source_time.as_secs_f64(), WIDTH, HEIGHT)?;
    let picture = letterbox(&frame.rgba, frame.width, frame.height);
    save_png(&picture, WIDTH, HEIGHT, out, "The project picture can't be saved")?;
    Ok(true)
}

/// Saves the timeline's frame at `at` at the full size of its file, as a
/// PNG. Refused in a gap, where there is no picture.
pub fn save_frame(project: &Project, sequence: SequenceId, at: Time, out: &Path) -> diag::Result<()> {
    let showing = project
        .sequences
        .iter()
        .find(|s| s.id == sequence)
        .and_then(|seq| video_at(project, seq, at))
        .ok_or_else(|| {
            AppError::new("There is no picture here")
                .why("The playhead is in a gap or on sound only.")
                .fix("Move the playhead onto a video clip.")
        })?;
    let video = showing.media.info.video.as_ref();
    let rotation = video.map_or(0, |v| v.rotation);
    let (w, h) = video.map_or((u32::MAX, u32::MAX), |v| v.display_size());
    let mut decoder = VideoDecoder::open(
        &showing.media.path,
        showing.clip.stream,
        rotation,
        Decoding::Software,
    )?;
    let frame = decoder.frame_at(showing.source_time.as_secs_f64(), w, h)?;
    save_png(
        &frame.rgba,
        frame.width,
        frame.height,
        out,
        "The frame can't be saved",
    )
}

/// Writes beside `out`, then renames, so nobody reads half a file.
fn save_png(rgba: &[u8], w: u32, h: u32, out: &Path, what: &str) -> diag::Result<()> {
    let partial = out.with_extension("partial.png");
    image::save_buffer_with_format(
        &partial,
        rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| {
        AppError::new(what)
            .why(format!("Writing {} failed: {e}.", partial.display()))
            .fix("Check that the folder can be written to and the drive has space.")
    })?;
    std::fs::rename(&partial, out).or_explain(|| {
        AppError::new(what)
            .why(format!("Renaming it to {} failed.", out.display()))
            .fix("Check that the folder can be written to.")
    })
}

/// `rgba` (`w` × `h`, at most [`WIDTH`] × [`HEIGHT`]) centred on black.
fn letterbox(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let (w, h) = (w.min(WIDTH) as usize, h.min(HEIGHT) as usize);
    let (x0, y0) = ((WIDTH as usize - w) / 2, (HEIGHT as usize - h) / 2);
    let stride = WIDTH as usize * 4;
    for (y, row) in rgba.chunks_exact(w * 4).take(h).enumerate() {
        let at = (y0 + y) * stride + x0 * 4;
        out[at..at + w * 4].copy_from_slice(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Command;

    #[test]
    fn letterbox_centres_the_picture() {
        let red = [255u8, 0, 0, 255].repeat(2 * 4);
        let out = letterbox(&red, 2, 4);
        let px = |x: u32, y: u32| {
            let i = ((y * WIDTH + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        assert_eq!(px(WIDTH / 2, HEIGHT / 2), [255, 0, 0, 255]);
        assert_eq!(px(0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn writes_a_picture_of_the_timeline() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        let media = media::describe(&fixture).unwrap();
        let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
        // Far past the end: falls back to the first clip.
        assert_eq!(
            frame_time(&p, seq, Time::from_ratio(100, 1).unwrap()),
            Some(Time::ZERO)
        );

        let dir = std::env::temp_dir().join(format!("thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("p.thumbnail.png");
        assert!(write(&p, seq, Time::ZERO, &out).unwrap());
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (WIDTH, HEIGHT));

        let full = dir.join("frame.png");
        save_frame(&p, seq, Time::ZERO, &full).unwrap();
        let v = p.media.values().next().unwrap().info.video.clone().unwrap();
        let img = image::open(&full).unwrap();
        assert_eq!((img.width(), img.height()), v.display_size(), "full size");
        assert!(
            save_frame(&p, seq, Time::from_ratio(100, 1).unwrap(), &full).is_err(),
            "a gap"
        );

        let empty = Project::new_automatic("E");
        assert!(!write(&empty, empty.active_sequence, Time::ZERO, &out).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
