//! Noticing a run that did not close cleanly.
//!
//! A marker file is written when the app starts and removed only by
//! [`Session::end`]. If the marker is still there at the next start, the
//! previous run crashed, was killed, or lost power. This also catches cases
//! the crash handler never sees.
//!
//! The marker names the editor's process and the project it has open, so the
//! launcher can tell a crash from an editor that is still running, and offer
//! to recover that project ([`unfinished_run`]).
//!
//! Limitation: two editor windows running at once would see each other's
//! marker. Only one editor runs at a time for now.

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
        let session = Session { marker };
        session.write(None)?;
        Ok(SessionStart {
            session,
            previous_crashed,
        })
    }

    /// Records which project this run has open, so a crash can offer to recover it.
    pub fn note_project(&self, project: &Path) -> Result<()> {
        self.write(Some(project))
    }

    fn write(&self, project: Option<&Path>) -> Result<()> {
        let mut contents = format!(
            "pid {}\nstarted {}\n",
            std::process::id(),
            humantime::format_rfc3339_seconds(SystemTime::now())
        );
        if let Some(project) = project {
            contents.push_str(&format!("project {}\n", project.display()));
        }
        fs::write(&self.marker, contents).or_explain(|| {
            AppError::new("The app can't record that it has started")
                .why(format!("Writing {} failed.", self.marker.display()))
                .fix("Check that the disk is not full and that you can write to the app's data folder.")
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

/// What the marker of a run that hasn't closed says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedRun {
    /// The editor's process id; if it is still running this was no crash.
    pub pid: Option<u32>,
    /// The project it had open, if any.
    pub project: Option<PathBuf>,
}

/// The editor run that hasn't closed cleanly, if any: either still running or
/// crashed. Reading it changes nothing.
pub fn unfinished_run(data_dir: &Path) -> Option<UnfinishedRun> {
    let text = fs::read_to_string(data_dir.join(MARKER_FILE)).ok()?;
    let mut run = UnfinishedRun {
        pid: None,
        project: None,
    };
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix("pid ") {
            run.pid = pid.trim().parse().ok();
        } else if let Some(project) = line.strip_prefix("project ") {
            run.project = Some(PathBuf::from(project));
        }
    }
    Some(run)
}

/// Removes the marker of a crashed run once the crash has been dealt with,
/// so the next editor start doesn't report it again.
pub fn forget_unfinished_run(data_dir: &Path) -> Result<()> {
    match fs::remove_file(data_dir.join(MARKER_FILE)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::new("The crash notice can't be cleared")
            .why("The app's note about the last run could not be removed.")
            .fix("Nothing is lost; the editor may mention the crash once more.")
            .source(err)),
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

    #[test]
    fn the_marker_names_the_process_and_project() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(unfinished_run(tmp.path()), None);
        let started = Session::start(tmp.path()).unwrap();
        let run = unfinished_run(tmp.path()).unwrap();
        assert_eq!(run.pid, Some(std::process::id()));
        assert_eq!(run.project, None);

        started.session.note_project(Path::new("/videos/trip.x")).unwrap();
        let run = unfinished_run(tmp.path()).unwrap();
        assert_eq!(run.project, Some(PathBuf::from("/videos/trip.x")));

        drop(started); // a crash: the marker stays
        assert!(unfinished_run(tmp.path()).is_some());
        forget_unfinished_run(tmp.path()).unwrap();
        assert_eq!(unfinished_run(tmp.path()), None);
        assert!(!Session::start(tmp.path()).unwrap().previous_crashed);
    }
}
