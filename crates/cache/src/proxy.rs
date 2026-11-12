//! Proxies on disk: small copies of recordings that are cheap to jump
//! around in (`docs/research/caching.md`, approach C).
//!
//! A proxy belongs to a recording, not to a project: it is kept in the
//! app's cache folder under the recording's fingerprint (and picture
//! stream), so every project that uses the recording shares it, and a
//! recording that was moved or renamed keeps its proxy. It can always be
//! made again, so the oldest are dropped first when the disk budget is
//! reached ([`Shelf::trim`]), and nothing about proxies is stored in a
//! project file.
//!
//! [`Shelf`] is shared between the interface (which builds and deletes
//! proxies) and the preview worker (which reads them); the mode (Off, On,
//! Auto) and a counter that changes whenever a proxy comes or goes are
//! atomics, so neither ever waits for the other.

use diag::{AppError, Explain};
use media::proxy::{Steps, Written};
use media::{Decoding, VideoDecoder};
use project::Fingerprint;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Whether proxies are made and used. A setting, the same for every project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProxyMode {
    /// No proxies are made or used.
    Off,
    /// A proxy for every video file, and the viewer shows proxy frames only.
    On,
    /// A proxy for files a jump in which can't make 50 ms ([`judge`]); the
    /// viewer shows a proxy frame first and the exact frame when the
    /// playhead rests.
    #[default]
    Auto,
}

impl ProxyMode {
    pub const ALL: [ProxyMode; 3] = [ProxyMode::Auto, ProxyMode::On, ProxyMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            ProxyMode::Off => "Off",
            ProxyMode::On => "On",
            ProxyMode::Auto => "Auto",
        }
    }

    /// What the setting does, in a sentence.
    pub fn explained(self) -> &'static str {
        match self {
            ProxyMode::Off => "No proxies: the viewer always decodes the recordings themselves.",
            ProxyMode::On => {
                "A proxy for every video file; the viewer shows proxy frames only (smooth, a little soft). Export always uses the originals."
            }
            ProxyMode::Auto => {
                "A proxy for files that are slow to jump around in. The viewer shows its frame at once and the exact frame when the playhead rests. Export always uses the originals."
            }
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            ProxyMode::Off => 0,
            ProxyMode::On => 1,
            ProxyMode::Auto => 2,
        }
    }

    fn from_bits(bits: u8) -> ProxyMode {
        match bits {
            0 => ProxyMode::Off,
            1 => ProxyMode::On,
            _ => ProxyMode::Auto,
        }
    }
}

/// A jump should put a picture on screen within this long (Checkpoint 4A).
pub const JUMP_TARGET: Duration = Duration::from_millis(50);
/// Frames decoded in a row to time a file's decoding.
const TIMED_FRAMES: u32 = 30;
/// A build that has not written for this long was left by a program that
/// stopped: its file is removed.
const LEFT_BEHIND: Duration = Duration::from_secs(10 * 60);
/// How often, and how far apart, deleting a proxy that is still open is tried.
const DELETE_TRIES: u32 = 50;
const DELETE_RETRY: Duration = Duration::from_millis(100);
const EXTENSION: &str = "mp4";
const BUILDING: &str = "building";

/// The folder of proxies, shared by the interface and the preview worker.
#[derive(Debug)]
pub struct Shelf {
    folder: PathBuf,
    mode: AtomicU8,
    /// Changes whenever a proxy is added or removed, or the mode changes.
    generation: AtomicU64,
    /// Proxies being deleted: no longer offered, though their file may
    /// still be there while a preview worker lets go of it.
    retiring: Mutex<HashSet<PathBuf>>,
    /// Held while the list of cleared proxies is read and written.
    marks: Mutex<()>,
}

/// The list of proxies cleared on purpose, in the cache folder: one name
/// a line, as [`Shelf::file_for`] names the proxy without its extension.
const CLEARED_LIST: &str = "cleared.txt";

/// What a clearing removed.
#[derive(Debug, Default)]
pub struct Cleared {
    pub usage: Usage,
    /// The proxies removed.
    pub paths: Vec<PathBuf>,
    /// Why some proxy could not be removed, when one could not (the others
    /// are gone).
    pub problem: Option<AppError>,
}

/// What some proxies take on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub bytes: u64,
    /// How many proxies.
    pub files: usize,
}

/// How a file was judged for a proxy ([`judge`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Judgement {
    /// The longest stretch between key frames, in seconds.
    pub longest_gap: f64,
    /// How long a typical jump takes on this machine.
    pub typical_jump: Duration,
    /// Whether Auto builds a proxy for it.
    pub needs_proxy: bool,
}

impl Shelf {
    /// The proxies in `folder` (made when the first proxy is written).
    pub fn new(folder: impl Into<PathBuf>, mode: ProxyMode) -> Arc<Shelf> {
        Arc::new(Shelf {
            folder: folder.into(),
            mode: AtomicU8::new(mode.to_bits()),
            generation: AtomicU64::new(0),
            retiring: Mutex::new(HashSet::new()),
            marks: Mutex::new(()),
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn mode(&self) -> ProxyMode {
        ProxyMode::from_bits(self.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(&self, mode: ProxyMode) {
        if self.mode.swap(mode.to_bits(), Ordering::Relaxed) != mode.to_bits() {
            self.changed();
        }
    }

    /// Changes whenever a proxy is added or removed or the mode changes:
    /// whoever remembers which proxies exist looks again then.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Where the proxy of picture stream `stream` of the recording with
    /// this fingerprint is kept.
    pub fn file_for(&self, fingerprint: &Fingerprint, stream: u32) -> PathBuf {
        self.folder.join(format!(
            "{}-{}-{stream}.{EXTENSION}",
            fingerprint.size, fingerprint.sample_hash
        ))
    }

    /// The proxy, if it has been built. Looks at the disk: not for every
    /// frame of the interface (the preview worker remembers the answer
    /// until [`Shelf::generation`] changes).
    pub fn ready(&self, fingerprint: &Fingerprint, stream: u32) -> Option<PathBuf> {
        let path = self.file_for(fingerprint, stream);
        (path.is_file() && !self.retiring().contains(&path)).then_some(path)
    }

    fn retiring(&self) -> std::sync::MutexGuard<'_, HashSet<PathBuf>> {
        self.retiring
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Notes that a proxy was used now: the ones not used for longest are
    /// dropped first. Its file's time of change says when.
    pub fn used(&self, proxy: &Path) {
        let touched = std::fs::File::options()
            .write(true)
            .open(proxy)
            .and_then(|file| file.set_modified(SystemTime::now()));
        if let Err(err) = touched {
            tracing::debug!(file = %proxy.display(), "a proxy's last use could not be noted: {err}");
        }
    }

    /// Builds the proxy of picture stream `stream` of `original` (`length`
    /// seconds long), decoding the original `decoding`'s way. Slow (the
    /// whole file is decoded): run it on a worker. The file appears only
    /// when complete; stopped or failed, nothing is left.
    pub fn build(
        &self,
        original: &Path,
        stream: u32,
        length: f64,
        fingerprint: &Fingerprint,
        decoding: Decoding,
        steps: &Steps<'_>,
    ) -> diag::Result<Written> {
        std::fs::create_dir_all(&self.folder).or_explain(|| {
            AppError::new("The proxy can't be made")
                .why(format!(
                    "The cache folder {} can't be made.",
                    self.folder.display()
                ))
                .fix("Check that the disk is not full and that you can write there.")
        })?;
        // Built on purpose: no longer one that was cleared.
        if let Err(err) = self.unmark_cleared(fingerprint, stream) {
            tracing::warn!("a cleared proxy's mark could not be taken off: {}", err.report());
        }
        let done = self.file_for(fingerprint, stream);
        let building = done.with_extension(format!("{}.{BUILDING}", std::process::id()));
        let written = media::proxy::write(original, stream, length, &building, decoding, false, steps)?;
        std::fs::rename(&building, &done).or_explain(|| {
            let _ = std::fs::remove_file(&building);
            AppError::new("The proxy can't be made")
                .why(format!("{} could not be put in place.", done.display()))
                .fix("Check that the disk is not full and that you can write there.")
        })?;
        self.retiring().remove(&done);
        self.changed();
        Ok(written)
    }

    /// Deletes the proxy, if there is one. Returns whether there was. It
    /// is no longer offered at once; its file may take a moment to go,
    /// while a preview worker that has it open lets go of it (a file that
    /// is open can't be removed on every system), so run this on a worker.
    pub fn delete(&self, fingerprint: &Fingerprint, stream: u32) -> diag::Result<bool> {
        let path = self.file_for(fingerprint, stream);
        if !path.exists() {
            return Ok(false);
        }
        self.retiring().insert(path.clone());
        self.changed();
        let removed = remove_waiting(&path).map_err(|err| {
            AppError::new("The proxy can't be deleted")
                .why(format!("Removing {} failed: {err}.", path.display()))
                .fix("It is not used any more. Close other programs that may have it open, then try again.")
        });
        self.retiring().remove(&path);
        removed?;
        // Deleted on purpose: not built again by itself, only by hand.
        if let Err(err) = self.mark_cleared(proxy_name(&path).into_iter()) {
            tracing::warn!("a deleted proxy could not be noted: {}", err.report());
        }
        Ok(true)
    }

    /// What the proxies take on disk: those at `only` (a project's, from
    /// [`Shelf::file_for`]), or all of them. Reads the folder: run it on a
    /// worker.
    pub fn usage(&self, only: Option<&[PathBuf]>) -> Usage {
        self.listed()
            .into_iter()
            .filter(|(path, _, _)| only.is_none_or(|only| only.contains(path)))
            .fold(Usage::default(), |sum, (_, bytes, _)| Usage {
                bytes: sum.bytes + bytes,
                files: sum.files + 1,
            })
    }

    /// Deletes the proxies at `only` (a project's), or every proxy and the
    /// files of builds a program left behind. Only proxy files in the
    /// cache folder are ever removed. Each is no longer offered at once,
    /// and may take a moment to go while a preview worker lets go of it:
    /// run this on a worker. Each one removed is marked as cleared on
    /// purpose ([`Shelf::is_cleared`]), so it is not built again by itself,
    /// until a build asked for by hand. Returns what was removed, and why
    /// some proxy could not be.
    pub fn clear(&self, only: Option<&[PathBuf]>) -> Cleared {
        if only.is_none() {
            self.remove_left_behind();
        }
        let going: Vec<(PathBuf, u64)> = self
            .listed()
            .into_iter()
            .filter(|(path, _, _)| only.is_none_or(|only| only.contains(path)))
            .map(|(path, bytes, _)| (path, bytes))
            .collect();
        let mut cleared = Cleared::default();
        if going.is_empty() {
            return cleared;
        }
        self.retiring().extend(going.iter().map(|(path, _)| path.clone()));
        self.changed();
        let mut failed = None;
        for (path, bytes) in &going {
            match remove_waiting(path) {
                Ok(()) => {
                    cleared.usage.bytes += bytes;
                    cleared.usage.files += 1;
                    cleared.paths.push(path.clone());
                }
                Err(err) => {
                    tracing::warn!(file = %path.display(), "a proxy could not be cleared: {err}");
                    failed.get_or_insert((path.clone(), err));
                }
            }
            self.retiring().remove(path);
        }
        let names = cleared.paths.iter().filter_map(|p| proxy_name(p));
        if let Err(err) = self.mark_cleared(names) {
            tracing::warn!("cleared proxies could not be noted: {}", err.report());
        }
        cleared.problem = failed.map(|(path, err)| {
            AppError::new("Some of the cache could not be cleared")
                .why(format!(
                    "{} of {} proxies were removed; removing {} failed: {err}.",
                    cleared.usage.files,
                    going.len(),
                    path.display()
                ))
                .fix("Close other programs that may have it open, then clear again.")
        });
        cleared
    }

    /// Whether this recording's proxy was cleared on purpose: then it is
    /// not built again by itself (only when asked for by hand, which takes
    /// the mark off). Reads a small file: run it on a worker.
    pub fn is_cleared(&self, fingerprint: &Fingerprint, stream: u32) -> bool {
        let _held = self.marks();
        let name = self.name_for(fingerprint, stream);
        self.cleared_names().contains(&name)
    }

    /// Takes the "cleared on purpose" mark off a recording's proxy.
    pub fn unmark_cleared(&self, fingerprint: &Fingerprint, stream: u32) -> diag::Result<()> {
        let _held = self.marks();
        let mut names = self.cleared_names();
        if names.remove(&self.name_for(fingerprint, stream)) {
            self.write_cleared(&names)?;
        }
        Ok(())
    }

    fn mark_cleared(&self, new: impl Iterator<Item = String>) -> diag::Result<()> {
        let _held = self.marks();
        let mut names = self.cleared_names();
        let before = names.len();
        names.extend(new);
        if names.len() != before {
            self.write_cleared(&names)?;
        }
        Ok(())
    }

    fn marks(&self) -> std::sync::MutexGuard<'_, ()> {
        self.marks.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The name a proxy's file has without its extension.
    fn name_for(&self, fingerprint: &Fingerprint, stream: u32) -> String {
        proxy_name(&self.file_for(fingerprint, stream)).unwrap_or_default()
    }

    /// The proxies cleared on purpose, as the list in the folder says
    /// (none when there is no list).
    fn cleared_names(&self) -> HashSet<String> {
        std::fs::read_to_string(self.folder.join(CLEARED_LIST))
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Writes the list whole: a file of its own first, then put in place.
    fn write_cleared(&self, names: &HashSet<String>) -> diag::Result<()> {
        let mut lines: Vec<&str> = names.iter().map(String::as_str).collect();
        lines.sort_unstable();
        let path = self.folder.join(CLEARED_LIST);
        let writing = path.with_extension(format!("{}.writing", std::process::id()));
        let cant = || {
            AppError::new("The list of cleared proxies can't be saved")
                .why(format!("Writing {} failed.", path.display()))
                .fix("Check that the disk is not full and that you can write there.")
        };
        std::fs::create_dir_all(&self.folder).or_explain(cant)?;
        std::fs::write(&writing, lines.join("\n")).or_explain(cant)?;
        std::fs::rename(&writing, &path).or_explain(|| {
            let _ = std::fs::remove_file(&writing);
            cant()
        })
    }

    /// The proxies there are, with their size and last use, oldest first.
    fn listed(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return Vec::new();
        };
        let mut found: Vec<(PathBuf, u64, SystemTime)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let meta = entry.metadata().ok()?;
                let proxy = meta.is_file() && path.extension().is_some_and(|e| e == EXTENSION);
                proxy.then(|| {
                    (
                        path,
                        meta.len(),
                        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    )
                })
            })
            .collect();
        found.sort_by_key(|(_, _, used)| *used);
        found
    }

    /// Bytes the proxies take.
    pub fn bytes(&self) -> u64 {
        self.listed().iter().map(|(_, bytes, _)| bytes).sum()
    }

    /// Drops the proxies used longest ago until they take at most `budget`
    /// bytes, leaving `keep` alone (the ones the open project uses).
    /// Also removes files of builds that a program left behind. Returns
    /// how many proxies were dropped.
    pub fn trim(&self, budget: u64, keep: &[PathBuf]) -> usize {
        self.remove_left_behind();
        let listed = self.listed();
        let mut total: u64 = listed.iter().map(|(_, bytes, _)| bytes).sum();
        let mut dropped = 0;
        for (path, bytes, _) in listed {
            if total <= budget {
                break;
            }
            if keep.contains(&path) {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    total = total.saturating_sub(bytes);
                    dropped += 1;
                    tracing::info!(file = %path.display(), "a proxy was dropped to stay inside the disk budget");
                }
                Err(err) => tracing::warn!(file = %path.display(), "a proxy could not be dropped: {err}"),
            }
        }
        if dropped > 0 {
            self.changed();
        }
        dropped
    }

    fn remove_left_behind(&self) {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|changed| changed.elapsed().ok())
                .is_some_and(|age| age > LEFT_BEHIND);
            if stale && path.extension().is_some_and(|e| e == BUILDING) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// A proxy's name without its extension: what the list of cleared ones
/// keeps.
fn proxy_name(path: &Path) -> Option<String> {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

/// Removes the file at `path`, waiting a while for whoever still has it
/// open to let go (an open file can't be removed on every system). A file
/// that is gone already counts as removed.
fn remove_waiting(path: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) if tries < DELETE_TRIES => {
                tracing::debug!(file = %path.display(), "a proxy is still in use: {err}");
                tries += 1;
                std::thread::sleep(DELETE_RETRY);
            }
            Err(err) => return Err(err),
        }
    }
}

/// Whether a jump in picture stream `stream` of `path` (`length` seconds)
/// can make [`JUMP_TARGET`] without a proxy: from the file's longest stretch
/// between key frames and a decode timed here, on the GPU's decoder when
/// `decoding` allows, as the preview decodes. Not from the codec's name:
/// the same codec jumps quickly with key frames close together and slowly
/// with them far apart. Slow (opens the file and decodes a few dozen
/// frames): run it on a worker.
pub fn judge(path: &Path, stream: u32, length: f64, decoding: Decoding) -> diag::Result<Judgement> {
    let mut decoder = VideoDecoder::open(path, stream, 0, decoding)?;
    let longest_gap = decoder.key_frames().longest_gap(length);
    let Some(cost) = decoder.decode_cost(TIMED_FRAMES)? else {
        // Too short to time: nothing to jump around in.
        return Ok(Judgement {
            longest_gap,
            typical_jump: Duration::ZERO,
            needs_proxy: false,
        });
    };
    let typical_jump = cost.typical_jump(longest_gap);
    Ok(Judgement {
        longest_gap,
        typical_jump,
        needs_proxy: typical_jump > JUMP_TARGET,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    fn print(n: u64) -> Fingerprint {
        Fingerprint {
            size: n,
            sample_hash: format!("{n:032x}"),
        }
    }

    fn quietly() -> Steps<'static> {
        Steps {
            cancelled: &|| false,
            progress: &|_| {},
            pause: &|| {},
        }
    }

    #[test]
    fn the_mode_and_its_changes_are_shared() {
        let shelf = Shelf::new("/nowhere", ProxyMode::Auto);
        assert_eq!(shelf.mode(), ProxyMode::Auto);
        let before = shelf.generation();
        shelf.set_mode(ProxyMode::On);
        assert_eq!(shelf.mode(), ProxyMode::On);
        assert!(shelf.generation() > before);
        let now = shelf.generation();
        shelf.set_mode(ProxyMode::On);
        assert_eq!(shelf.generation(), now, "no change, no news");
        assert_eq!(ProxyMode::default(), ProxyMode::Auto);
    }

    #[test]
    fn a_proxy_is_kept_by_fingerprint_and_stream() {
        let shelf = Shelf::new("/cache/proxies", ProxyMode::Auto);
        let a = shelf.file_for(&print(1), 0);
        assert_ne!(a, shelf.file_for(&print(2), 0));
        assert_ne!(a, shelf.file_for(&print(1), 1));
        assert!(a.starts_with("/cache/proxies"));
        assert!(shelf.ready(&print(1), 0).is_none());
    }

    #[test]
    fn built_used_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path().join("proxies"), ProxyMode::Auto);
        let original = fixture("vfr-60.mkv");
        let fingerprint = media::fingerprint(&original).unwrap();
        let before = shelf.generation();
        let written = shelf
            .build(&original, 0, 2.0, &fingerprint, Decoding::Software, &quietly())
            .unwrap();
        assert!(written.frames > 0);
        let path = shelf.ready(&fingerprint, 0).expect("built");
        assert!(shelf.generation() > before, "the preview looks again");
        // Only the finished proxy is there.
        let files: Vec<_> = std::fs::read_dir(shelf.folder()).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        shelf.used(&path);
        assert!(shelf.bytes() > 0);
        assert!(shelf.delete(&fingerprint, 0).unwrap());
        assert!(shelf.ready(&fingerprint, 0).is_none());
        assert!(!shelf.delete(&fingerprint, 0).unwrap());
    }

    #[test]
    fn the_proxies_used_longest_ago_go_first() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path(), ProxyMode::Auto);
        let now = SystemTime::now();
        // Four proxies of 100 bytes, used one, two, three and four hours ago.
        for n in 1..=4u64 {
            let path = shelf.file_for(&print(n), 0);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(3600 * n)).unwrap();
        }
        let oldest = shelf.file_for(&print(4), 0);
        // Room for two, and the oldest is in use by the open project.
        assert_eq!(shelf.trim(200, std::slice::from_ref(&oldest)), 2);
        assert!(oldest.exists(), "kept: in use");
        assert!(shelf.ready(&print(1), 0).is_some(), "the newest stays");
        assert!(shelf.ready(&print(2), 0).is_none() && shelf.ready(&print(3), 0).is_none());
        assert_eq!(shelf.trim(1000, &[]), 0, "inside the budget: nothing goes");
    }

    #[test]
    fn a_short_small_file_needs_no_proxy() {
        let judged = judge(&fixture("game-3-audio.mkv"), 0, 3.0, Decoding::Software).unwrap();
        assert!(judged.longest_gap > 0.0);
        assert!(!judged.needs_proxy, "{judged:?}");
    }

    #[test]
    fn a_projects_proxies_or_all_are_measured_and_cleared_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path(), ProxyMode::Auto);
        let print = |n: u64| Fingerprint {
            size: n,
            sample_hash: format!("{n:x}"),
        };
        let ours = shelf.file_for(&print(1), 0);
        let theirs = shelf.file_for(&print(2), 0);
        std::fs::write(&ours, vec![0u8; 1000]).unwrap();
        std::fs::write(&theirs, vec![0u8; 4000]).unwrap();
        // Not proxies: never measured, never removed.
        let other = dir.path().join("notes.txt");
        std::fs::write(&other, "keep").unwrap();
        let building = dir.path().join("3-c-0.mp4.77.building");
        std::fs::write(&building, "half").unwrap();
        let project = [ours.clone()];
        assert_eq!(
            shelf.usage(Some(&project)),
            Usage {
                bytes: 1000,
                files: 1
            }
        );
        assert_eq!(
            shelf.usage(None),
            Usage {
                bytes: 5000,
                files: 2
            }
        );
        let generation = shelf.generation();
        assert_eq!(
            shelf.clear(Some(&project)).usage,
            Usage {
                bytes: 1000,
                files: 1
            }
        );
        assert!(shelf.generation() > generation, "workers look again");
        assert!(!ours.exists() && theirs.exists());
        assert_eq!(shelf.ready(&print(1), 0), None);
        assert_eq!(
            shelf.clear(Some(&project)).usage,
            Usage::default(),
            "nothing left of ours"
        );
        assert_eq!(
            shelf.clear(None).usage,
            Usage {
                bytes: 4000,
                files: 1
            }
        );
        assert!(!theirs.exists());
        assert!(other.exists(), "only proxies go");
        assert!(building.exists(), "a build under way is left alone");
        assert_eq!(shelf.usage(None), Usage::default());
    }

    #[test]
    fn a_cleared_proxy_stays_marked_until_built_on_purpose() {
        let dir = tempfile::tempdir().unwrap();
        let print = |n: u64| Fingerprint {
            size: n,
            sample_hash: format!("{n:x}"),
        };
        let shelf = Shelf::new(dir.path(), ProxyMode::On);
        assert!(!shelf.is_cleared(&print(1), 0), "nothing cleared yet");
        std::fs::write(shelf.file_for(&print(1), 0), b"proxy").unwrap();
        std::fs::write(shelf.file_for(&print(2), 0), b"proxy").unwrap();
        shelf.clear(Some(&[shelf.file_for(&print(1), 0)]));
        assert!(shelf.is_cleared(&print(1), 0));
        assert!(!shelf.is_cleared(&print(2), 0), "only what was cleared");
        assert!(!shelf.is_cleared(&print(1), 1), "per picture stream");
        // Kept for good: a shelf opened later (the next start) knows it.
        let later = Shelf::new(dir.path(), ProxyMode::Auto);
        assert!(later.is_cleared(&print(1), 0));
        // Clearing everything marks every proxy it removed.
        later.clear(None);
        assert!(later.is_cleared(&print(2), 0));
        assert_eq!(later.usage(None), Usage::default(), "the list is not a proxy");
        // Built on purpose ("Build proxy now"): the mark comes off.
        later.unmark_cleared(&print(1), 0).unwrap();
        assert!(!later.is_cleared(&print(1), 0));
        assert!(later.is_cleared(&print(2), 0));
        // "Delete proxy" is on purpose too: the same mark.
        std::fs::write(later.file_for(&print(1), 0), b"proxy").unwrap();
        assert!(later.delete(&print(1), 0).unwrap());
        assert!(later.is_cleared(&print(1), 0));
    }
}
