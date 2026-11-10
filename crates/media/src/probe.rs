//! What a media file holds, read from its headers without decoding.

use crate::ffi::{Input, Stream, StreamKind};
use diag::AppError;
use project::{AudioStream, MediaInfo, MediaKind, Rate, Time, VideoStream};
use std::path::Path;

/// The result of probing a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub kind: MediaKind,
    /// Zero for still images.
    pub duration: Time,
    pub info: MediaInfo,
}

/// Reads a file's container, streams, codecs, frame rate, length and rotation.
pub fn probe(path: &Path) -> diag::Result<Probe> {
    let input = Input::open(path).map_err(|e| {
        if e.not_media {
            AppError::new("This file can't be imported")
                .why(format!(
                    "{} isn't a video, sound or picture file the app can read ({}).",
                    name(path),
                    e.text
                ))
                .fix("Check that it plays in a media player. If it does, report it with the file attached.")
        } else {
            AppError::new("The file can't be opened")
                .why(format!("{}: {}.", path.display(), e.text))
                .fix("Check that the file still exists and that the drive is connected.")
        }
    })?;

    let streams = input.streams();
    let container = input.container();
    let picture = streams
        .iter()
        .find(|s| s.kind() == StreamKind::Video && !s.is_attached_picture());
    let mut video = picture.map(video_stream);
    let audio: Vec<AudioStream> = streams
        .iter()
        .filter(|s| s.kind() == StreamKind::Audio)
        .enumerate()
        .map(|(n, s)| audio_stream(s, n + 1))
        .collect();

    let kind = match (&video, audio.is_empty()) {
        (Some(_), _) if is_still_image(&container) => MediaKind::Image,
        (Some(_), _) => MediaKind::Video,
        (None, false) => MediaKind::Audio,
        (None, true) => {
            return Err(AppError::new("This file has nothing to import")
                .why(format!("No picture or sound was found in {}.", name(path))));
        }
    };
    let duration = if kind == MediaKind::Image {
        Time::ZERO
    } else {
        length(&input, &streams)
    };
    drop(streams);
    if kind == MediaKind::Video
        && let Some(v) = video.as_mut()
    {
        measure_timing(&input, v, duration);
    }
    Ok(Probe {
        kind,
        duration,
        info: MediaInfo {
            container,
            video,
            audio,
        },
    })
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Single pictures are opened through FFmpeg's image readers.
fn is_still_image(container: &str) -> bool {
    container == "image2" || container.ends_with("_pipe")
}

fn video_stream(s: &Stream) -> VideoStream {
    let (width, height) = s.size();
    let frame_rate = s
        .frame_rate()
        .and_then(|(n, d)| Rate::new(n, d).ok())
        .unwrap_or(Rate::FPS_30);
    VideoStream {
        index: s.index(),
        codec: s.codec(),
        width,
        height,
        frame_rate,
        rotation: s.display_matrix().map_or(0, |m| clockwise_turn(&m)),
        variable_frame_rate: false,
    }
}

/// Reads packet times at the start, middle and end of the file and settles
/// the frame rate: constant or variable, and the rate it is shown at.
fn measure_timing(input: &Input, video: &mut VideoStream, duration: Time) {
    let secs = duration.as_secs_f64();
    let mut windows = vec![input.packet_times(video.index, 0.0, 10.0)];
    if secs > 30.0 {
        windows.push(input.packet_times(video.index, secs / 2.0, 5.0));
        windows.push(input.packet_times(video.index, secs - 6.0, 5.0));
    }
    if let Some(timing) = crate::timing::analyse(&windows) {
        video.frame_rate = timing.rate;
        video.variable_frame_rate = timing.variable;
    }
}

fn audio_stream(s: &Stream, number: usize) -> AudioStream {
    // Generic handler names some recorders write say nothing about the stream.
    let handler = s.tag("handler_name").filter(|h| {
        !matches!(
            h.as_str(),
            "SoundHandler" | "Sound Media Handler" | "Core Media Audio"
        )
    });
    let name = s
        .tag("title")
        .or(handler)
        .unwrap_or_else(|| format!("Audio {number}"));
    AudioStream {
        index: s.index(),
        codec: s.codec(),
        channels: s.channels(),
        sample_rate: s.sample_rate(),
        name,
    }
}

/// The container's length, else the longest stream's.
fn length(input: &Input, streams: &[Stream]) -> Time {
    if let Some(us) = input.duration_us() {
        return Time::from_ratio(us, 1_000_000).unwrap_or(Time::ZERO);
    }
    streams
        .iter()
        .filter_map(Stream::duration)
        .filter_map(|(num, den)| Time::from_ratio(num, den.unsigned_abs()))
        .max()
        .unwrap_or(Time::ZERO)
}

/// The clockwise turn, in whole quarter turns, that shows a picture upright.
///
/// The display matrix stores a counter-clockwise rotation (as FFmpeg's
/// `av_display_rotation_get` reads it); showing the picture upright means
/// turning it the same amount counter-clockwise, which is the negative
/// clockwise turn.
fn clockwise_turn(m: &[i32; 9]) -> u16 {
    let (a, b, c, d) = (f64::from(m[0]), f64::from(m[1]), f64::from(m[3]), f64::from(m[4]));
    let (scale0, scale1) = (a.hypot(c), b.hypot(d));
    if scale0 == 0.0 || scale1 == 0.0 {
        return 0;
    }
    let counter_clockwise = -(b / scale1).atan2(a / scale0).to_degrees();
    let clockwise = -counter_clockwise;
    let quarter = (clockwise / 90.0).round() as i64;
    (quarter.rem_euclid(4) * 90) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A display matrix for a clockwise turn, the way FFmpeg's
    /// `av_display_rotation_set` writes one.
    fn matrix_clockwise(degrees: f64) -> [i32; 9] {
        let r = (-degrees).to_radians();
        let (s, c) = (r.sin(), r.cos());
        let fix = |v: f64| (v * 65536.0).round() as i32;
        [fix(c), fix(-s), 0, fix(s), fix(c), 0, 0, 0, 1 << 30]
    }

    #[test]
    fn rotations_become_clockwise_quarter_turns() {
        assert_eq!(clockwise_turn(&matrix_clockwise(0.0)), 0);
        assert_eq!(clockwise_turn(&matrix_clockwise(90.0)), 90);
        assert_eq!(clockwise_turn(&matrix_clockwise(-90.0)), 270);
        assert_eq!(clockwise_turn(&matrix_clockwise(180.0)), 180);
        assert_eq!(
            clockwise_turn(&matrix_clockwise(89.6)),
            90,
            "rounded to a quarter turn"
        );
        assert_eq!(clockwise_turn(&[0; 9]), 0, "an empty matrix means no turn");
    }
}
