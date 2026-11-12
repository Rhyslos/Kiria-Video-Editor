//! The checks a file passes before it is imported, so nothing that can't
//! be played ever reaches a timeline. Each refusal is explained: what is
//! wrong, why, and what to do.
//!
//! In order, each only when the one before passed:
//!
//! 1. It opens as media and has a picture or sound (`probe`).
//! 2. This build can decode every picture and sound stream in it: a codec
//!    FFmpeg doesn't know is "too new"; one it knows without a decoder is
//!    "not supported".
//! 3. Its first picture decodes, and its sound gives samples ("damaged").
//! 4. It holds what it says to its end: a recording whose end is missing
//!    (the recorder stopped before finishing it) is "cut short".
//!
//! This reads the file's headers, decodes a frame and reads packets near
//! the end: tens of milliseconds. Call it from a worker.

use crate::ffi::{CodecSupport, Input, StreamKind};
use crate::{AudioDecoder, Decoding, VideoDecoder};
use diag::AppError;
use project::{MediaKind, MediaRef};
use std::path::Path;

/// How far before its stated end a file's last packet may be before it
/// counts as cut short, in seconds. Streams end a frame or a sound packet
/// early in well-formed files; half a second leaves room for that.
const END_SLACK: f64 = 0.5;

/// What kind of problem stopped an import, for summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a media file, or nothing in it to use.
    NotMedia,
    /// A codec this version doesn't know (made by something newer).
    TooNew,
    /// A codec known but not supported here.
    Unsupported,
    /// What is in it can't be decoded.
    Damaged,
    /// Its end is missing.
    CutShort,
    /// It couldn't be read at all (missing, no permission).
    Unreadable,
}

impl Refusal {
    /// A few words for a summary line: "damaged", "too new".
    pub fn words(self) -> &'static str {
        match self {
            Refusal::NotMedia => "not media",
            Refusal::TooNew => "codec too new",
            Refusal::Unsupported => "codec not supported",
            Refusal::Damaged => "damaged",
            Refusal::CutShort => "end missing",
            Refusal::Unreadable => "can't be read",
        }
    }
}

/// A file that failed its checks: the kind of problem and the explanation.
#[derive(Debug)]
pub struct Refused {
    pub refusal: Refusal,
    pub error: AppError,
}

/// Everything the project keeps about a file, once it has passed every
/// check; otherwise why it can't be imported.
pub fn check(path: &Path) -> Result<MediaRef, Refused> {
    let media = crate::describe(path).map_err(|error| Refused {
        refusal: if error.what().contains("can't be opened") {
            Refusal::Unreadable
        } else {
            Refusal::NotMedia
        },
        error,
    })?;
    let name = crate::probe::file_name(path);
    let input = Input::open(path).map_err(|e| Refused {
        refusal: Refusal::Unreadable,
        error: AppError::new("The file can't be opened")
            .why(format!("{}: {}.", path.display(), e.text))
            .fix("Check that the file still exists and that the drive is connected."),
    })?;
    let used = |index: u32| {
        media.info.video.as_ref().is_some_and(|v| v.index == index)
            || media.info.audio.iter().any(|a| a.index == index)
    };
    for stream in input.streams().iter().filter(|s| used(s.index())) {
        let what = match stream.kind() {
            StreamKind::Video => "picture",
            _ => "sound",
        };
        match stream.codec_support() {
            CodecSupport::Decodable => {}
            CodecSupport::Unknown => {
                return Err(Refused {
                    refusal: Refusal::TooNew,
                    error: AppError::new(format!("{name} can't be imported: its {what} is in an unknown format"))
                        .why(format!(
                            "Its {what} is stored in a way this version can't recognise, usually because it was made by something newer than this version."
                        ))
                        .fix("Update the app (the Update button at the bottom left of the launcher). Until then, save a copy of the file as H.264 video with AAC sound and import that."),
                });
            }
            CodecSupport::NoDecoder => {
                let codec = stream.codec();
                return Err(Refused {
                    refusal: Refusal::Unsupported,
                    error: AppError::new(format!("{name} can't be imported: its {what} format isn't supported"))
                        .why(format!("Its {what} is stored as {codec}, which this version can't play."))
                        .fix("Save a copy of the file as H.264 video with AAC sound (any converter can) and import that."),
                });
            }
        }
    }
    let duration = media.duration.as_secs_f64();
    if let Some(video) = &media.info.video {
        let decoded = VideoDecoder::open(path, video.index, video.rotation, Decoding::Software)
            .and_then(|mut d| d.frame_at(0.0, 64, 64));
        if let Err(err) = decoded {
            tracing::info!(path = %path.display(), "first picture didn't decode: {}", err.report());
            return Err(damaged(&name, "Its first picture can't be read"));
        }
    }
    for sound in &media.info.audio {
        let mut samples = [0.0f32; 2 * 960];
        let read = AudioDecoder::open(path, sound.index, 48_000).map(|mut d| d.read(&mut samples));
        if !matches!(read, Ok(n) if n > 0) {
            return Err(damaged(
                &name,
                &format!("Its sound \"{}\" can't be read", sound.name),
            ));
        }
    }
    if media.kind != MediaKind::Image && duration > 2.0 * END_SLACK {
        let stream = media
            .info
            .video
            .as_ref()
            .map(|v| v.index)
            .or_else(|| media.info.audio.first().map(|a| a.index));
        if let Some(stream) = stream {
            let from = (duration - 6.0).max(0.0);
            let last = input
                .packet_times(stream, from, 10.0)
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max);
            if last < duration - END_SLACK.max(duration * 0.02) {
                let says = clock(duration);
                let why = if last.is_finite() && last > from {
                    format!(
                        "It says it is {says} long, but what is in it stops at {}: the recording was stopped before it was finished, or the copy was interrupted.",
                        clock(last)
                    )
                } else {
                    format!(
                        "It says it is {says} long, but its end is missing: the recording was stopped before it was finished, or the copy was interrupted."
                    )
                };
                return Err(Refused {
                    refusal: Refusal::CutShort,
                    error: AppError::new(format!("{name}: its end is missing"))
                        .why(why)
                        .fix("Import what's there to use the part it holds. Or skip it and repair it: copy the file again if a copy was interrupted, else copy its streams into a new file without re-encoding (a remux), and import that."),
                });
            }
        }
    }
    Ok(media)
}

/// A file whose end is missing, as far as it goes: its length is what is
/// really in it (to the frame after its last picture, or its last sound
/// packet), and `stated_length` keeps the length it says it has. When the
/// end can't be found near where it should be, the whole file is read
/// (packets only, nothing decoded), which takes a while for a long
/// recording: call it from a worker, and only when asked.
pub fn salvage(path: &Path) -> diag::Result<MediaRef> {
    let mut media = crate::describe(path)?;
    let name = crate::probe::file_name(path);
    let input = Input::open(path).map_err(|e| {
        AppError::new("The file can't be opened")
            .why(format!("{}: {}.", path.display(), e.text))
            .fix("Check that the file still exists and that the drive is connected.")
    })?;
    let stated = media.duration;
    let duration = stated.as_secs_f64();
    let video = media.info.video.as_ref().map(|v| (v.index, v.frame_rate));
    let Some(stream) = video
        .map(|(index, _)| index)
        .or_else(|| media.info.audio.first().map(|a| a.index))
    else {
        return Ok(media);
    };
    let latest = |times: Vec<f64>| times.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let from = (duration - 6.0).max(0.0);
    let mut last = latest(input.packet_times(stream, from, 10.0));
    if !(last.is_finite() && last > from) {
        last = latest(input.packet_times(stream, 0.0, f64::MAX));
    }
    if !last.is_finite() || last <= 0.0 {
        return Err(
            AppError::new(format!("{name} can't be imported: nothing in it can be read"))
                .why("Its end is missing, and no picture or sound was found in what is there.")
                .fix("Copy the file again from where it came from."),
        );
    }
    let real = match video {
        // To the end of the last picture's frame.
        Some((_, rate)) => rate.frame_start((last * rate.as_f64()).round() as i64 + 1),
        None => project::Time::from_ratio((last * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(stated),
    };
    if real < stated {
        media.duration = real;
        media.info.stated_length = Some(stated);
    }
    Ok(media)
}

/// "Its end is missing: it is 0:41 long, though it says 2:00", for a file
/// imported with its end missing; `None` for a whole file.
pub fn end_missing_words(media: &MediaRef) -> Option<String> {
    let stated = media.info.stated_length?;
    Some(format!(
        "Its end is missing: it is {} long, though it says {}",
        clock(media.duration.as_secs_f64()),
        clock(stated.as_secs_f64())
    ))
}

fn damaged(name: &str, part: &str) -> Refused {
    Refused {
        refusal: Refusal::Damaged,
        error: AppError::new(format!("{name} can't be imported: it is damaged"))
            .why(format!("{part}: the data in the file is broken."))
            .fix("Copy the file again from where it came from. If the original plays in a media player, report it with the file attached."),
    }
}

/// Seconds as "0.9 s" (under a minute), "1:05" or "1:02:03".
fn clock(seconds: f64) -> String {
    if seconds < 60.0 {
        return format!("{:.1} s", seconds.max(0.0));
    }
    let s = seconds.floor() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_times() {
        assert_eq!(clock(0.94), "0.9 s");
        assert_eq!(clock(2.008), "2.0 s");
        assert_eq!(clock(65.2), "1:05");
        assert_eq!(clock(3723.0), "1:02:03");
    }
}
