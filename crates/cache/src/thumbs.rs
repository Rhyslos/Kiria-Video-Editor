//! Small pictures along a video file, for drawing inside timeline clips,
//! and kept on disk by the recording's fingerprint ([`Store`]), so a
//! file's pictures are read once, not once a session.

use media::{Decoding, Frame, VideoDecoder};
use project::Fingerprint;
use std::path::{Path, PathBuf};

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
/// in. Otherwise, or with `exact` (Settings › Editing › Clip pictures),
/// each picture is the exact frame.
pub fn strip(
    path: &Path,
    stream: u32,
    rotation: u16,
    duration: f64,
    exact: bool,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(f32),
) -> diag::Result<Strip> {
    let every = spacing(duration);
    // Opened for glances (one frame at a time, each as soon as its packet
    // is in), what a long recording's pictures are read with; opened again
    // with every thread only for exact frames.
    let mut decoder = VideoDecoder::open_for_glances(path, stream, rotation, Decoding::Software)?;
    let keys = decoder.key_frames().clone();
    let glances = !exact && by_key_frames(&keys, duration, every);
    if !glances {
        decoder = VideoDecoder::open(path, stream, rotation, Decoding::Software)?;
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

/// Strips kept on disk, one file per recording and picture stream, named by
/// the recording's fingerprint (as proxies and waveforms are), each picture
/// a small JPEG: a few hundred kilobytes a recording. The folder keeps the
/// most recently used within its budget ([`Store::trim`]).
#[derive(Debug, Clone)]
pub struct Store {
    folder: PathBuf,
}

/// What a pictures file starts with: its kind and format version.
const MAGIC: &[u8; 8] = b"PICTS\0\0\x01";

/// How finely the pictures are kept: plenty for 160 × 90.
const QUALITY: u8 = 85;

impl Store {
    pub fn new(folder: PathBuf) -> Store {
        Store { folder }
    }

    fn file_for(&self, fingerprint: &Fingerprint, stream: u32) -> PathBuf {
        self.folder.join(format!(
            "{}-{}-{stream}.pictures",
            fingerprint.size, fingerprint.sample_hash
        ))
    }

    /// The strip kept for this stream, if any, readable and taken the same
    /// way (`exact` frames or not).
    pub fn load(&self, fingerprint: &Fingerprint, stream: u32, exact: bool) -> Option<Strip> {
        let path = self.file_for(fingerprint, stream);
        let bytes = std::fs::read(&path).ok()?;
        let strip = decode(&bytes, exact);
        if strip.is_some() {
            // Its last use: what `trim` keeps the newest of.
            let _ = std::fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        }
        strip
    }

    /// Keeps `strip` (taken `exact`ly or not) for this stream. Failing only
    /// costs reading it again.
    pub fn save(&self, fingerprint: &Fingerprint, stream: u32, exact: bool, strip: &Strip) {
        let written = encode(strip, exact).and_then(|bytes| {
            std::fs::create_dir_all(&self.folder)
                .and_then(|()| project::disk::write_atomic(&self.file_for(fingerprint, stream), &bytes))
                .map_err(|e| e.to_string())
        });
        if let Err(why) = written {
            tracing::info!("clip pictures could not be kept on disk: {why}");
        }
    }

    /// Removes every strip kept (Settings › Cache). Returns how many went.
    pub fn clear(&self) -> usize {
        self.trim(0)
    }

    /// Removes the strips used longest ago until those left take at most
    /// `budget` bytes. Returns how many went.
    pub fn trim(&self, budget: u64) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return 0;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "pictures"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), e.path()))
            })
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        let mut kept = 0u64;
        let mut removed = 0;
        for (_, len, path) in files {
            kept += len;
            if kept > budget && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

fn encode(strip: &Strip, exact: bool) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&strip.every.to_le_bytes());
    bytes.push(u8::from(exact));
    bytes.extend_from_slice(&(strip.frames.len() as u32).to_le_bytes());
    for frame in &strip.frames {
        let rgb: Vec<u8> = frame
            .rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
            .encode(&rgb, frame.width, frame.height, image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
        bytes.extend_from_slice(&frame.time.to_le_bytes());
        bytes.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&jpeg);
    }
    Ok(bytes)
}

fn decode(bytes: &[u8], exact: bool) -> Option<Strip> {
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let every = f64::from_le_bytes(rest.get(0..8)?.try_into().ok()?);
    if *rest.get(8)? != u8::from(exact) {
        return None;
    }
    let count = u32::from_le_bytes(rest.get(9..13)?.try_into().ok()?) as usize;
    let mut at = 13;
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let time = f64::from_le_bytes(rest.get(at..at + 8)?.try_into().ok()?);
        let len = u32::from_le_bytes(rest.get(at + 8..at + 12)?.try_into().ok()?) as usize;
        let jpeg = rest.get(at + 12..at + 12 + len)?;
        at += 12 + len;
        let picture = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            .ok()?
            .to_rgba8();
        frames.push(Frame {
            width: picture.width(),
            height: picture.height(),
            rgba: picture.into_raw(),
            time,
        });
    }
    (at == rest.len()).then_some(Strip { every, frames })
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
            false,
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

    #[test]
    fn strips_are_kept_on_disk_by_fingerprint_and_the_way_they_were_taken() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let media = media::describe(&fixture).unwrap();
        let index = media.info.video.as_ref().unwrap().index;
        let read = strip(
            &fixture,
            index,
            0,
            media.duration.as_secs_f64(),
            false,
            || false,
            |_| (),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("pictures"));
        assert!(
            store.load(&media.fingerprint, index, false).is_none(),
            "nothing kept yet"
        );
        store.save(&media.fingerprint, index, false, &read);
        let kept = store.load(&media.fingerprint, index, false).expect("kept");
        assert_eq!(kept.every, read.every);
        assert_eq!(kept.frames.len(), read.frames.len());
        for (k, r) in kept.frames.iter().zip(&read.frames) {
            assert_eq!((k.width, k.height, k.time), (r.width, r.height, r.time));
            // JPEG: close to the picture, not the same bytes.
            let off: u64 = k
                .rgba
                .iter()
                .zip(&r.rgba)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            assert!(off / (k.rgba.len() as u64) < 8, "close to what was read: {off}");
        }
        assert!(
            store.load(&media.fingerprint, index, true).is_none(),
            "taken another way"
        );
        let mut other = media.fingerprint.clone();
        other.size += 1;
        assert!(store.load(&other, index, false).is_none(), "another recording");
        assert_eq!(store.trim(u64::MAX), 0);
        assert_eq!(store.clear(), 1);
        assert!(store.load(&media.fingerprint, index, false).is_none());
    }
}
