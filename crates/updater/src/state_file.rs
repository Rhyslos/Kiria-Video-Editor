//! The updater's small state files (`updates.ron`, `health.ron`,
//! `rollback.ron` and the install's `current`): written whole or not at
//! all, and an unreadable one is reported and kept aside, never quietly
//! replaced.
//!
//! - Writing: `diag::disk::write_atomic`, the one writer of the workspace
//!   (the launcher's version panel writes `updates.ron` through it too): a
//!   temporary file of this write's own
//!   beside the target, put on the disk, then renamed over the old file, so
//!   a crash at any point leaves the old file or the new one, never half.
//! - [`read_ron`]: a missing file is simply absent. A file that can't be
//!   read or understood is logged and copied to `<name>.unreadable` beside
//!   it before the caller falls back to defaults, so what it held (the
//!   versions that failed to start, above all) can still be found.

use serde::de::DeserializeOwned;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What an unreadable file's copy is called: `<name>.unreadable`.
pub const UNREADABLE_SUFFIX: &str = "unreadable";

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
