//! The updater's small state files (`updates.ron`, `health.ron`,
//! `rollback.ron` and the install's `current`): written whole or not at
//! all, and an unreadable one is reported and kept aside, never quietly
//! replaced.
//!
//! - [`write_whole`]: the text goes to a temporary file of this write's own
//!   beside the target (`.name.<number>.saving`), is put on the disk, and is
//!   then renamed over the old file. A crash at any point leaves either the
//!   old file or the new one, never half of one (the same approach as the
//!   project crate's files, which this crate does not depend on).
//! - [`read_ron`]: a missing file is simply absent. A file that can't be
//!   read or understood is logged and copied to `<name>.unreadable` beside
//!   it before the caller falls back to defaults, so what it held (the
//!   versions that failed to start, above all) can still be found.

use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// What an unreadable file's copy is called: `<name>.unreadable`.
pub const UNREADABLE_SUFFIX: &str = "unreadable";

/// Writes `bytes` to `path` so that a crash never leaves half a file. On a
/// failure the temporary file is taken away again and the old file is as
/// it was.
pub fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temp, mut file) = create_temp(path)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::rename(&temp, path)
        });
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written?;
    sync_folder(path);
    Ok(())
}

/// Reads a RON file. `None` when it is missing, or when it can't be read or
/// understood: then the reason is logged and the file is copied to
/// [`unreadable_path`] (replacing an older copy), so nothing it held is lost
/// when the caller writes a fresh one.
pub fn read_ron<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(file = %path.display(), "can't read an update state file ({err}); using defaults");
            keep_aside(path);
            return None;
        }
    };
    match ron::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(file = %path.display(), "can't understand an update state file ({err}); using defaults");
            keep_aside(path);
            None
        }
    }
}

/// Where an unreadable `path` is kept: `<name>.unreadable` beside it.
pub fn unreadable_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(UNREADABLE_SUFFIX);
    path.with_file_name(name)
}

/// Copies an unreadable file aside (a copy, so a file that was only locked
/// for a moment is still there next time).
fn keep_aside(path: &Path) {
    let aside = unreadable_path(path);
    match fs::copy(path, &aside) {
        Ok(_) => tracing::warn!(copy = %aside.display(), "the unreadable file was kept"),
        Err(err) => {
            tracing::warn!(file = %path.display(), "the unreadable file could not be kept aside ({err})")
        }
    }
}

/// Asks the disk to keep the rename too (the folder's entry). Only Unix
/// can open a folder for this; Windows keeps the rename with the file.
fn sync_folder(path: &Path) {
    #[cfg(unix)]
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(folder) = File::open(folder)
    {
        let _ = folder.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Numbers temporary files within this program; with the process id in
/// front, no two writes anywhere share one.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// A new temporary file beside `path` with a name nobody else has.
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let process = u64::from(std::process::id()) << 32;
    loop {
        let number = process | (NEXT_TEMP.fetch_add(1, Ordering::Relaxed) & 0xFFFF_FFFF);
        let temp = path.with_file_name(format!(".{name}.{number}.saving"));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            // Left by an earlier run with the same process id: next number.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_replaces_the_file_whole_and_leaves_nothing_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("updates.ron");
        write_whole(&path, b"one").unwrap();
        write_whole(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(names_in(tmp.path()), ["updates.ron"]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("health.ron");
        write_whole(&path, b"old").unwrap();
        // A folder where the file should go: the rename fails.
        let blocked = tmp.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("inside"), "x").unwrap();
        assert!(write_whole(&blocked, b"new").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(names_in(tmp.path()), ["blocked", "health.ron"]);
    }

    #[test]
    fn a_missing_file_is_absent_and_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollback.ron");
        assert_eq!(read_ron::<Vec<String>>(&path), None);
        assert!(names_in(tmp.path()).is_empty());
    }

    #[test]
    fn an_unreadable_file_is_kept_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("updates.ron");
        fs::write(&path, "(bad_versions: [\"0.2").unwrap();
        assert_eq!(read_ron::<Vec<String>>(&path), None);
        assert_eq!(
            fs::read_to_string(unreadable_path(&path)).unwrap(),
            "(bad_versions: [\"0.2"
        );
        assert_eq!(names_in(tmp.path()), ["updates.ron", "updates.ron.unreadable"]);
    }
}
