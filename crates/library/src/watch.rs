//! Watching media folders for new recordings.
//!
//! A thread of its own looks through the media folders every couple of
//! seconds (three levels deep, hidden folders skipped) for video, sound and
//! picture files changed after a moment (`since`: when the project was last
//! saved, so "new since you last worked on it"). A file only counts once it
//! has stopped growing: the same size and modification time on two looks in
//! a row, [`SETTLE`] apart, and it opens for reading. A recording still
//! being written therefore waits until the recorder is done with it.
//!
//! Looking (polling) instead of asking the system for change events keeps
//! this to the standard library, works the same on every system and on
//! network drives, and costs a directory listing every few seconds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

/// How long a file must stay the same before it counts as finished.
pub const SETTLE: Duration = Duration::from_secs(2);

/// How deep below a media folder files are looked for.
const DEPTH: usize = 3;

/// File types looked for, as the import dialog offers them.
pub const EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "mov", "m4v", "webm", "avi", "ts", "flv", "wmv", // video
    "wav", "mp3", "flac", "ogg", "opus", "m4a", "aac", "wma", // sound
    "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", // pictures
];

/// A finished new file in a media folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// The media folder it was found in.
    pub folder: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

/// Where the watcher gets its folders: asked before every look, so a folder
/// added in the launcher meanwhile is watched from the next look on.
pub type Folders = Box<dyn Fn() -> Vec<PathBuf> + Send>;

/// What one look keeps about a file not yet settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    size: u64,
    modified: SystemTime,
}

/// The finished new files, and the files still settling, between looks.
#[derive(Debug, Default)]
pub struct Scanner {
    since: Option<SystemTime>,
    settling: HashMap<PathBuf, Seen>,
    found: Vec<Found>,
}

impl Scanner {
    /// A scanner for files changed after `since`.
    pub fn new(since: SystemTime) -> Scanner {
        Scanner {
            since: Some(since),
            ..Scanner::default()
        }
    }

    /// One look through `folders`: a file seen with the same size and time
    /// as on the look before (and that opens) becomes found; one that
    /// changed waits for the next look. Returns whether the found files
    /// changed.
    pub fn look(&mut self, folders: &[PathBuf]) -> bool {
        let since = self.since.unwrap_or(SystemTime::UNIX_EPOCH);
        let mut now: HashMap<PathBuf, (PathBuf, Seen)> = HashMap::new();
        for folder in folders {
            for (path, seen) in media_files(folder) {
                if seen.modified > since {
                    now.insert(path, (folder.clone(), seen));
                }
            }
        }
        let before = self.found.len();
        // Files gone, or changed again since they were found, are dropped.
        self.found.retain(|f| {
            now.get(&f.path)
                .is_some_and(|(_, s)| s.size == f.size && s.modified == f.modified)
        });
        let mut changed = self.found.len() != before;
        let mut settling = HashMap::new();
        for (path, (folder, seen)) in now {
            if self.found.iter().any(|f| f.path == path) {
                continue;
            }
            if self.settling.get(&path) == Some(&seen) && seen.size > 0 && opens(&path) {
                self.found.push(Found {
                    path,
                    folder,
                    size: seen.size,
                    modified: seen.modified,
                });
                changed = true;
            } else {
                settling.insert(path, seen);
            }
        }
        self.settling = settling;
        self.found
            .sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
        changed
    }

    /// The finished new files, newest first.
    pub fn found(&self) -> &[Found] {
        &self.found
    }

    /// How many files are still being written (or were changed on the last look).
    pub fn settling(&self) -> usize {
        self.settling.len()
    }
}

/// Whether the file opens for reading (a recorder that holds it open
/// to itself, as some systems allow, makes this fail until it is done).
fn opens(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

/// Media files in `folder` and up to [`DEPTH`] levels below, hidden ones
/// and hidden folders left out.
fn media_files(folder: &Path) -> Vec<(PathBuf, Seen)> {
    let mut files = Vec::new();
    let mut folders = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(meta) = entry.metadata() else { continue };
            if hidden {
                continue;
            }
            if meta.is_dir() {
                if depth < DEPTH {
                    folders.push((path, depth + 1));
                }
            } else if is_media(&path)
                && let Ok(modified) = meta.modified()
            {
                files.push((
                    path,
                    Seen {
                        size: meta.len(),
                        modified,
                    },
                ));
            }
        }
    }
    files
}

/// Whether the file's extension is one of [`EXTENSIONS`] (any case).
pub fn is_media(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

#[derive(Default)]
struct Shared {
    found: Arc<Vec<Found>>,
    settling: usize,
    looks: u64,
}

/// The scanner on a thread of its own, looking every [`SETTLE`].
pub struct Watcher {
    shared: Arc<Mutex<Shared>>,
    stop: Sender<()>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

impl Watcher {
    /// Starts watching the folders `folders` gives, for files changed after
    /// `since`; `wake` is called when the found files change.
    pub fn start(folders: Folders, since: SystemTime, wake: impl Fn() + Send + 'static) -> Watcher {
        Watcher::start_every(folders, since, SETTLE, wake)
    }

    /// [`Watcher::start`], looking every `every` (shorter in tests).
    pub fn start_every(
        folders: Folders,
        since: SystemTime,
        every: Duration,
        wake: impl Fn() + Send + 'static,
    ) -> Watcher {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (stop, stopped) = channel();
        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("media folders".into())
            .spawn(move || {
                run(
                    &folders,
                    Scanner::new(since),
                    every,
                    &stopped,
                    &worker_shared,
                    &wake,
                )
            });
        if let Err(err) = spawned {
            tracing::warn!("media folders aren't watched: the thread didn't start: {err}");
        }
        Watcher { shared, stop }
    }

    /// The finished new files, newest first. Never waits for a look.
    pub fn found(&self) -> Arc<Vec<Found>> {
        Arc::clone(&lock(&self.shared).found)
    }

    /// How many new files are still being written.
    pub fn settling(&self) -> usize {
        lock(&self.shared).settling
    }

    /// How many looks through the folders are done.
    pub fn looks(&self) -> u64 {
        lock(&self.shared).looks
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run(
    folders: &Folders,
    mut scanner: Scanner,
    every: Duration,
    stopped: &Receiver<()>,
    shared: &Mutex<Shared>,
    wake: &dyn Fn(),
) {
    loop {
        let changed = scanner.look(&folders());
        let settling = scanner.settling();
        {
            let mut s = lock(shared);
            if changed {
                s.found = Arc::new(scanner.found().to_vec());
            }
            s.settling = settling;
            s.looks += 1;
        }
        if changed {
            wake();
        }
        match stopped.recv_timeout(every) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: usize) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(&vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn a_file_counts_once_it_stops_growing() {
        let dir = tempfile::tempdir().unwrap();
        let folders = vec![dir.path().to_path_buf()];
        let mut scanner = Scanner::new(SystemTime::UNIX_EPOCH);
        let rec = dir.path().join("match.mkv");
        write(&rec, 100);
        assert!(!scanner.look(&folders), "seen once: maybe still being written");
        assert_eq!(scanner.settling(), 1);
        write(&rec, 100);
        assert!(!scanner.look(&folders), "it grew: still being written");
        assert!(scanner.look(&folders), "the same twice in a row: finished");
        assert_eq!(scanner.found().len(), 1);
        assert_eq!(scanner.found()[0].size, 200);
        assert_eq!(scanner.found()[0].folder, dir.path());
        assert!(!scanner.look(&folders), "nothing new");
        std::fs::remove_file(&rec).unwrap();
        assert!(scanner.look(&folders), "gone again");
        assert!(scanner.found().is_empty());
    }

    #[test]
    fn only_media_files_below_the_folders_and_after_the_moment_count() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("2026/11/12");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(dir.path().join(".cache")).unwrap();
        write(&deep.join("clip.MP4"), 10);
        write(&dir.path().join("notes.txt"), 10);
        write(&dir.path().join(".cache/hidden.mkv"), 10);
        write(&dir.path().join(".hidden.mkv"), 10);
        let too_deep = dir.path().join("a/b/c/d");
        std::fs::create_dir_all(&too_deep).unwrap();
        write(&too_deep.join("lost.mkv"), 10);
        write(&dir.path().join("empty.wav"), 0);
        let folders = vec![dir.path().to_path_buf()];
        let mut scanner = Scanner::new(SystemTime::UNIX_EPOCH);
        scanner.look(&folders);
        scanner.look(&folders);
        let names: Vec<_> = scanner.found().iter().map(|f| f.path.clone()).collect();
        assert_eq!(names, [deep.join("clip.MP4")], "an empty file waits too");

        let later = SystemTime::now() + Duration::from_secs(3600);
        let mut scanner = Scanner::new(later);
        scanner.look(&folders);
        scanner.look(&folders);
        assert!(
            scanner.found().is_empty(),
            "files from before the moment are not new"
        );
    }

    #[test]
    fn the_watcher_finds_new_files_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let (tx, rx) = channel();
        let watcher = Watcher::start_every(
            Box::new(move || vec![folder.clone()]),
            SystemTime::UNIX_EPOCH,
            Duration::from_millis(50),
            move || {
                let _ = tx.send(());
            },
        );
        write(&dir.path().join("new.mkv"), 64);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("woken when found");
        assert_eq!(watcher.found().len(), 1);
    }
}
