//! The render cache: timeline frames put together ahead of time (a region
//! pre-rendered), kept on disk by what they show, so playing them again
//! takes reading a picture instead of decoding and putting together every
//! layer (`docs/research/render-cache.md`).
//!
//! A frame's key is a hash of everything that makes its picture (the
//! engine works it out: `engine::prerender::frame_key`), never its place
//! on the timeline: a frame moved along with its clip is still found, and
//! any change to what it shows makes a new key, so a stale frame is never
//! shown. Each pre-render writes one pack: its frames as JPEGs one after
//! another (`<n>.frames`) and where each is (`<n>.index`, written whole,
//! again every so often while it runs, so what was done survives a crash).
//! Packs used longest ago go first to stay inside the budget; nothing about
//! them is in a project file. The export never reads them.

use media::Frame;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The JPEG quality frames are kept at: 90 looks like the picture put
/// together, at a tenth of its bytes.
pub const QUALITY: u8 = 90;

/// How many frames a pack takes before its index is written again.
const INDEX_EVERY: usize = 60;

/// The first line of an index file.
const INDEX_HEADER: &str = "render-index 1";

/// What makes a frame's picture, hashed: the key it is kept by.
pub type Key = u128;

/// Where a frame is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    pack: u32,
    offset: u64,
    len: u32,
}

#[derive(Debug, Default)]
struct Index {
    frames: HashMap<Key, Place>,
    /// The next pack's number.
    next: u32,
}

/// The folder of pre-rendered frames, shared by the interface, the
/// pre-render and the preview worker.
#[derive(Debug)]
pub struct Store {
    folder: PathBuf,
    index: Mutex<Index>,
}

impl Store {
    /// The store in `folder` (made when needed), with what is there.
    pub fn open(folder: PathBuf) -> Store {
        let mut index = Index::default();
        if let Ok(entries) = std::fs::read_dir(&folder) {
            for path in entries.flatten().map(|e| e.path()) {
                let Some(pack) = pack_number(&path, "index") else {
                    continue;
                };
                index.next = index.next.max(pack + 1);
                if path.with_extension("frames").is_file() {
                    read_index(&path, pack, &mut index.frames);
                }
            }
        }
        Store {
            folder,
            index: Mutex::new(index),
        }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether no frame is kept at all.
    pub fn is_empty(&self) -> bool {
        self.lock().frames.is_empty()
    }

    /// Whether the frame `key` is kept.
    pub fn has(&self, key: Key) -> bool {
        self.lock().frames.contains_key(&key)
    }

    /// The frame `key`, read and decoded (RGBA), when it is kept and can be
    /// read. Marks its pack as used.
    pub fn get(&self, key: Key) -> Option<Frame> {
        let place = *self.lock().frames.get(&key)?;
        let path = self.folder.join(format!("{}.frames", place.pack));
        let mut bytes = vec![0u8; place.len as usize];
        let read = File::open(&path).and_then(|mut f| {
            f.seek(SeekFrom::Start(place.offset))?;
            f.read_exact(&mut bytes)
        });
        if read.is_err() {
            // Gone or cut short: forget it rather than try every time.
            self.lock().frames.remove(&key);
            return None;
        }
        crate::disk::touch(&path);
        let picture = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).ok()?;
        let rgba = picture.into_rgba8();
        let (width, height) = rgba.dimensions();
        Some(Frame {
            width,
            height,
            rgba: rgba.into_raw(),
            time: 0.0,
        })
    }

    /// Starts a pack for a pre-render's frames.
    pub fn writer(&self) -> diag::Result<Writer<'_>> {
        std::fs::create_dir_all(&self.folder).map_err(|err| cannot(&self.folder, &err))?;
        let pack = {
            let mut index = self.lock();
            let pack = index.next;
            index.next += 1;
            pack
        };
        let path = self.folder.join(format!("{pack}.frames"));
        let file = File::create(&path).map_err(|err| cannot(&path, &err))?;
        Ok(Writer {
            store: self,
            pack,
            file,
            written: 0,
            frames: Vec::new(),
            unsaved: 0,
        })
    }

    /// Bytes kept, and how many frames.
    pub fn usage(&self) -> (u64, usize) {
        let bytes = packs(&self.folder).iter().map(|p| p.bytes).sum();
        (bytes, self.lock().frames.len())
    }

    /// Removes the packs used longest ago until the rest take at most
    /// `budget` bytes (0: all), never the one being written. Returns how
    /// many went.
    pub fn trim(&self, budget: u64) -> usize {
        diag::disk::remove_old_temps(&self.folder, crate::disk::UNFINISHED_LEFT_FOR);
        let mut found = packs(&self.folder);
        found.sort_by_key(|p| std::cmp::Reverse(p.used));
        let writing = self.lock().next.saturating_sub(1);
        let mut kept = 0u64;
        let mut removed = 0;
        for pack in found {
            kept += pack.bytes;
            if kept <= budget || (pack.number == writing && pack.recent) {
                continue;
            }
            self.lock().frames.retain(|_, place| place.pack != pack.number);
            let frames = self.folder.join(format!("{}.frames", pack.number));
            let _ = std::fs::remove_file(self.folder.join(format!("{}.index", pack.number)));
            if std::fs::remove_file(&frames).is_ok() || !frames.exists() {
                removed += 1;
            }
        }
        removed
    }

    /// Removes every pack. Returns how many went.
    pub fn clear(&self) -> usize {
        let removed = self.trim(0);
        // A pack being written is kept by `trim`; clearing takes it too
        // once it is done (it is no longer offered).
        self.lock().frames.clear();
        removed
    }
}

/// A pack on disk.
#[derive(Debug)]
struct Pack {
    number: u32,
    bytes: u64,
    used: std::time::SystemTime,
    /// Changed in the last minute: maybe still being written.
    recent: bool,
}

fn packs(folder: &Path) -> Vec<Pack> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now();
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let number = pack_number(&path, "frames")?;
            let meta = e.metadata().ok()?;
            let used = meta.modified().ok()?;
            let index = std::fs::metadata(path.with_extension("index")).map_or(0, |m| m.len());
            Some(Pack {
                number,
                bytes: meta.len() + index,
                used,
                recent: now.duration_since(used).map_or(true, |d| d.as_secs() < 60),
            })
        })
        .collect()
}

/// The number of a pack's file `<n>.<extension>`.
fn pack_number(path: &Path, extension: &str) -> Option<u32> {
    if path.extension()? != extension {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
}

fn read_index(path: &Path, pack: u32, frames: &mut HashMap<Key, Place>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut lines = text.lines();
    if lines.next() != Some(INDEX_HEADER) {
        return;
    }
    for line in lines {
        let mut parts = line.split(' ');
        let (Some(key), Some(offset), Some(len)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(key), Ok(offset), Ok(len)) = (Key::from_str_radix(key, 16), offset.parse(), len.parse())
        else {
            continue;
        };
        frames.insert(key, Place { pack, offset, len });
    }
}

fn cannot(path: &Path, err: &std::io::Error) -> diag::AppError {
    diag::AppError::new("The region can't be pre-rendered")
        .why(format!("{} can't be written ({err}).", path.display()))
        .fix("Check that the disk has room, or clear the cache in Settings › Cache.")
}

/// Adds a pre-render's frames to the store: each is offered as soon as it
/// is written.
#[derive(Debug)]
pub struct Writer<'a> {
    store: &'a Store,
    pack: u32,
    file: File,
    written: u64,
    frames: Vec<(Key, Place)>,
    unsaved: usize,
}

impl Writer<'_> {
    /// Keeps the RGBA picture `rgba` of `size` as the frame `key`.
    pub fn add(&mut self, key: Key, rgba: &[u8], size: (u32, u32)) -> diag::Result<()> {
        let mut jpeg = Vec::new();
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
            .encode(&rgb, size.0, size.1, image::ExtendedColorType::Rgb8)
            .map_err(|err| {
                diag::AppError::new("The region can't be pre-rendered")
                    .why(format!("A frame could not be compressed ({err})."))
                    .fix("Report it, with the log.")
            })?;
        let path = self.store.folder.join(format!("{}.frames", self.pack));
        self.file.write_all(&jpeg).map_err(|err| cannot(&path, &err))?;
        let place = Place {
            pack: self.pack,
            offset: self.written,
            len: jpeg.len() as u32,
        };
        self.written += jpeg.len() as u64;
        self.frames.push((key, place));
        self.store.lock().frames.insert(key, place);
        self.unsaved += 1;
        if self.unsaved >= INDEX_EVERY {
            self.save()?;
        }
        Ok(())
    }

    /// How many frames it holds.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Writes the index whole, so the frames so far survive a crash.
    fn save(&mut self) -> diag::Result<()> {
        let path = self.store.folder.join(format!("{}.index", self.pack));
        self.file.flush().map_err(|err| cannot(&path, &err))?;
        let mut text = String::from(INDEX_HEADER);
        for (key, place) in &self.frames {
            text.push_str(&format!("\n{key:032x} {} {}", place.offset, place.len));
        }
        diag::disk::write_atomic(&path, text.as_bytes()).map_err(|err| cannot(&path, &err))?;
        self.unsaved = 0;
        Ok(())
    }

    /// Writes the index for the last time; an empty pack is removed.
    pub fn finish(mut self) -> diag::Result<()> {
        if self.frames.is_empty() {
            let _ = std::fs::remove_file(self.store.folder.join(format!("{}.frames", self.pack)));
            return Ok(());
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(shade: u8) -> Vec<u8> {
        (0..32 * 16).flat_map(|_| [shade, shade, shade, 255]).collect()
    }

    #[test]
    fn frames_are_kept_by_key_and_found_again_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("renders"));
        assert!(!store.has(7));
        let mut writer = store.writer().unwrap();
        writer.add(7, &picture(200), (32, 16)).unwrap();
        writer.add(9, &picture(40), (32, 16)).unwrap();
        assert!(store.has(7), "offered as soon as written");
        writer.finish().unwrap();

        let again = Store::open(dir.path().join("renders"));
        let frame = again.get(9).unwrap();
        assert_eq!((frame.width, frame.height), (32, 16));
        assert!(
            frame.rgba[..3].iter().all(|v| v.abs_diff(40) <= 3),
            "{:?}",
            &frame.rgba[..4]
        );
        assert!(again.get(7).unwrap().rgba[0].abs_diff(200) <= 3);
        assert_eq!(again.usage().1, 2);
        assert!(again.get(8).is_none());
    }

    #[test]
    fn packs_used_longest_ago_go_first_and_clearing_takes_them_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        for key in [1, 2] {
            let mut writer = store.writer().unwrap();
            writer.add(key, &picture(100), (32, 16)).unwrap();
            writer.finish().unwrap();
        }
        let old = SystemTimeAgo(3600);
        old.set(&dir.path().join("0.frames"));
        old.set(&dir.path().join("1.frames"));
        // Pack 1 used again just now.
        assert!(store.get(2).is_some());
        let (bytes, _) = store.usage();
        assert_eq!(store.trim(bytes - 1), 1);
        assert!(!store.has(1) && store.has(2));
        assert!(!dir.path().join("0.frames").exists() && !dir.path().join("0.index").exists());
        SystemTimeAgo(3600).set(&dir.path().join("1.frames"));
        assert_eq!(store.clear(), 1);
        assert_eq!(store.usage(), (0, 0));
    }

    #[test]
    fn a_pack_cut_short_keeps_the_frames_its_index_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let mut writer = store.writer().unwrap();
        for key in 0..(INDEX_EVERY as u128 + 5) {
            writer.add(key, &picture(key as u8), (32, 16)).unwrap();
        }
        // A crash: never finished.
        std::mem::forget(writer);
        let again = Store::open(dir.path().to_path_buf());
        assert_eq!(again.usage().1, INDEX_EVERY);
        assert!(again.get(0).is_some());
        // A new pack doesn't write over it.
        let writer = again.writer().unwrap();
        assert_eq!(writer.pack, 1);
    }

    /// Sets a file's time of change some seconds ago.
    struct SystemTimeAgo(u64);

    impl SystemTimeAgo {
        fn set(&self, path: &Path) {
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(self.0);
            File::options()
                .append(true)
                .open(path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
    }
}
