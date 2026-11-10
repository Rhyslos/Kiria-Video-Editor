//! Noticing a run that did not close cleanly.
//!
//! A marker file is written when the app starts and removed only by
//! [`Session::end`]. If the marker is still there at the next start, the
//! previous run crashed, was killed, or lost power. This also catches cases
//! the crash handler never sees.
//!
//! Limitation: two editor windows running at once would see each other's
//! marker. Only one editor runs at a time until the launcher (Phase 2) decides
//! otherwise.

use crate::{AppError, Explain, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MARKER_FILE: &str = "session.running";

/// A running session. Call [`Session::end`] on a clean exit.
///
/// Dropping it does *not* remove the marker on purpose: a panic unwinds and
/// drops values, and that must still count as a crash.
#[derive(Debug)]
pub struct Session {
    marker: PathBuf,
}

/// The result of [`Session::start`].
#[derive(Debug)]
pub struct SessionStart {
    pub session: Session,
    pub previous_crashed: bool,
}

impl Session {
    /// Records that a run has started in `data_dir`.
    pub fn start(data_dir: &Path) -> Result<SessionStart> {
        let marker = data_dir.join(MARKER_FILE);
        let previous_crashed = marker.exists();
        let contents = format!(
            "pid {}\nstarted {}\n",
            std::process::id(),
            humantime::format_rfc3339_seconds(SystemTime::now())
        );
        fs::write(&marker, contents).or_explain(|| {
            AppError::new("The app can't record that it has started")
                .why(format!("Writing {} failed.", marker.display()))
                .fix("Check that the disk is not full and that you can write to the app's data folder.")
        })?;
        Ok(SessionStart {
            session: Session { marker },
            previous_crashed,
        })
    }

    /// Records a clean exit.
    pub fn end(self) -> Result<()> {
        match fs::remove_file(&self.marker) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::new("The app can't record a clean exit")
                .why(format!("Removing {} failed.", self.marker.display()))
                .fix("Nothing is lost; the next start may wrongly offer crash recovery.")
                .source(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_start_is_not_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let started = Session::start(tmp.path()).unwrap();
        assert!(!started.previous_crashed);
    }

    #[test]
    fn a_start_without_an_end_counts_as_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Session::start(tmp.path()).unwrap();
        drop(first); // no end(): the process "died"
        let second = Session::start(tmp.path()).unwrap();
        assert!(second.previous_crashed);
    }

    #[test]
    fn a_clean_end_is_not_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        Session::start(tmp.path()).unwrap().session.end().unwrap();
        let next = Session::start(tmp.path()).unwrap();
        assert!(!next.previous_crashed);
    }
}
