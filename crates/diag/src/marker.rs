//! Markers kept per process, each with a lock held beside it.
//!
//! A marker says that a process is in the middle of something that may not
//! end cleanly: a run of the editor ([`session`](crate::session)), a start
//! with a graphics driver that may crash. Each process writes its own,
//! `<process id>.<extension>` in the set's folder, and holds a lock on
//! `<process id>.lock` beside it for as long as the marker is meant. The
//! system lets go of a lock when its process ends however it ends, so a
//! marker whose lock can be taken belongs to a process that crashed, was
//! killed or lost power, and one whose lock is held belongs to a process
//! still going. Several processes can each keep a marker at once without
//! reading or removing each other's.
//!
//! A process id can be given out again once its process has ended. Taking
//! the lock ([`MarkerSet::hold`]) therefore never overwrites a marker of an
//! earlier process with the same id: one left there is set aside under
//! another name first, still counted as left behind.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

const LOCK_EXTENSION: &str = "lock";

/// The markers of one kind: `<dir>/<process id>.<extension>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerSet {
    dir: PathBuf,
    extension: &'static str,
}

/// A marker found in a [`MarkerSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundMarker {
    /// The marker file.
    pub path: PathBuf,
    /// Whether its process still holds its lock: `None` when there is no
    /// lock file beside it (written by hand, or by an older version).
    pub running: Option<bool>,
}

/// The lock of one process's marker, held until [`HeldLock::release`] or
/// until the process ends.
#[derive(Debug)]
pub struct HeldLock {
    marker: PathBuf,
    lock: File,
}

impl MarkerSet {
    pub fn new(dir: impl Into<PathBuf>, extension: &'static str) -> MarkerSet {
        MarkerSet {
            dir: dir.into(),
            extension,
        }
    }

    /// The folder of the markers.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The marker of process `pid`.
    pub fn marker_path(&self, pid: u32) -> PathBuf {
        self.dir.join(format!("{pid}.{}", self.extension))
    }

    /// Takes the lock of process `pid`'s marker; the marker itself is the
    /// caller's to write (at [`HeldLock::marker`]). A marker already there
    /// with a free lock was left by an earlier process given the same id: it
    /// is renamed first, so it still counts as left behind. Fails when
    /// another holder has the lock.
    pub fn hold(&self, pid: u32) -> io::Result<HeldLock> {
        fs::create_dir_all(&self.dir)?;
        let marker = self.marker_path(pid);
        let lock_path = lock_path(&marker);
        if marker.exists() && holds_lock(&lock_path) != Some(true) {
            self.set_aside(&marker, pid)?;
        }
        let lock = File::create(&lock_path)?;
        lock.try_lock().map_err(|err| match err {
            TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{} is held by another run", lock_path.display()),
            ),
            TryLockError::Error(err) => err,
        })?;
        Ok(HeldLock { marker, lock })
    }

    /// Renames an earlier process's marker out of the way of a new one.
    fn set_aside(&self, marker: &Path, pid: u32) -> io::Result<()> {
        let free = (1u32..)
            .map(|n| self.dir.join(format!("{pid}-earlier-{n}.{}", self.extension)))
            .find(|path| !path.exists())
            .ok_or_else(|| io::Error::other("no free name for an earlier marker"))?;
        fs::rename(marker, free)?;
        let _ = remove_file(&lock_path(marker));
        Ok(())
    }

    /// Every marker of the set, sorted by name. Reading them changes nothing
    /// (not even the folder is made).
    pub fn found(&self) -> Vec<FoundMarker> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == self.extension))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| FoundMarker {
                running: holds_lock(&lock_path(&path)),
                path,
            })
            .collect()
    }
}

impl FoundMarker {
    /// The marker of the process with this id: the number its name starts with.
    pub fn pid(&self) -> Option<u32> {
        let stem = self.path.file_stem()?.to_str()?;
        stem.split('-').next()?.parse().ok()
    }

    /// Takes the marker and its lock file away, once what it says is dealt with.
    pub fn forget(&self) -> io::Result<()> {
        remove_file(&self.path)?;
        let _ = remove_file(&lock_path(&self.path));
        Ok(())
    }
}

impl HeldLock {
    /// The marker this lock stands beside.
    pub fn marker(&self) -> &Path {
        &self.marker
    }

    /// Removes the marker, lets go of the lock and removes its file: what
    /// the marker said is over.
    pub fn release(self) -> io::Result<()> {
        let HeldLock { marker, lock } = self;
        let removed = remove_file(&marker);
        drop(lock);
        let _ = remove_file(&lock_path(&marker));
        removed
    }
}

fn lock_path(marker: &Path) -> PathBuf {
    marker.with_extension(LOCK_EXTENSION)
}

/// Removes a file that may already be gone.
pub(crate) fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Whether the process whose lock file is `lock` still holds it: `None`
/// when there is no lock file.
pub(crate) fn holds_lock(lock: &Path) -> Option<bool> {
    let file = File::options().write(true).open(lock).ok()?;
    match file.try_lock() {
        Ok(()) => Some(false),
        Err(TryLockError::WouldBlock) => Some(true),
        // Can't tell: better to miss a crash than call a running process crashed.
        Err(TryLockError::Error(_)) => Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(held: &HeldLock) {
        fs::write(held.marker(), "x").unwrap();
    }

    #[test]
    fn two_holders_at_once_see_each_other_running_and_release_only_their_own() {
        let tmp = tempfile::tempdir().unwrap();
        let set = MarkerSet::new(tmp.path().join("m"), "doing");
        let (a, b) = (set.hold(100).unwrap(), set.hold(200).unwrap());
        write(&a);
        write(&b);
        let found = set.found();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|m| m.running == Some(true)));
        assert_eq!(found[0].pid(), Some(100));
        a.release().unwrap();
        let found = set.found();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].pid(), found[0].running), (Some(200), Some(true)));
        drop(b); // B ends without releasing: left behind
        assert_eq!(set.found()[0].running, Some(false));
    }

    #[test]
    fn a_marker_left_by_an_earlier_process_with_the_same_id_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let set = MarkerSet::new(tmp.path(), "doing");
        let earlier = set.hold(100).unwrap();
        write(&earlier);
        drop(earlier); // that process died
        let now = set.hold(100).unwrap();
        write(&now);
        let found = set.found();
        assert_eq!(found.len(), 2, "the earlier one is set aside, not overwritten");
        let left: Vec<_> = found.iter().filter(|m| m.running != Some(true)).collect();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].pid(), Some(100));
        assert_ne!(left[0].path, set.marker_path(100));
        now.release().unwrap();
        assert_eq!(set.found().len(), 1, "the earlier one is still to be dealt with");
    }

    #[test]
    fn a_second_holder_of_the_same_lock_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let set = MarkerSet::new(tmp.path(), "doing");
        let held = set.hold(7).unwrap();
        write(&held);
        assert!(set.hold(7).is_err());
        assert!(set.marker_path(7).exists(), "the holder's marker stays");
    }

    #[test]
    fn reading_makes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let set = MarkerSet::new(tmp.path().join("m"), "doing");
        assert!(set.found().is_empty());
        assert!(!set.dir().exists());
    }
}
