//! Noticing a run that did not close cleanly.
//!
//! Each run writes a marker of its own when it starts
//! (`sessions/<process id>.running` in the data folder, which names the
//! process and the project it has open) and holds a lock on a file beside it
//! (`<process id>.lock`) for as long as it runs; [`Session::end`] removes
//! both. The system lets go of a lock when its process ends however it
//! ends, so a marker whose lock can be taken belongs to a run that crashed,
//! was killed, or lost power. This also catches cases the crash handler
//! never sees.
//!
//! Markers are per process ([`crate::marker`]), so several editors can run
//! at once (each on its own project) without taking the others for crashed,
//! and a process id used again by another program changes nothing. A new
//! run given the id of one that crashed still reports that crash: only the
//! lock decides.
//!
//! The launcher reads the markers ([`unfinished_runs`]) to tell a crash
//! from an editor that is still running, offer to recover that project, and
//! refuse to move a project an editor has open.

use crate::marker::{HeldLock, MarkerSet, holds_lock, remove_file};
use crate::{AppError, Explain, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The folder of the markers, in the data folder.
const MARKERS_DIR: &str = "sessions";
/// The one marker earlier versions wrote, for every run: still read, so a
/// crash of an older version is noticed once after an update.
const OLD_MARKER_FILE: &str = "session.running";
const MARKER_EXTENSION: &str = "running";
const LOCK_EXTENSION: &str = "lock";

/// The editors' run markers in `data_dir`.
fn markers(data_dir: &Path) -> MarkerSet {
    MarkerSet::new(data_dir.join(MARKERS_DIR), MARKER_EXTENSION)
}

/// A running session. Call [`Session::end`] on a clean exit.
///
/// Dropping it does *not* remove the marker on purpose: a panic unwinds and
/// drops values, and that must still count as a crash (the lock goes, the
/// marker stays).
#[derive(Debug)]
pub struct Session {
    pid: u32,
    /// Held while the run lasts, beside the marker.
    lock: HeldLock,
}

/// The result of [`Session::start`].
#[derive(Debug)]
pub struct SessionStart {
    pub session: Session,
    pub previous_crashed: bool,
}

impl Session {
    /// Records that a run has started in `data_dir`. Earlier runs whose
    /// lock is free count as crashed (and their markers are taken away, so
    /// a crash is reported once); runs still going are left alone.
    pub fn start(data_dir: &Path) -> Result<SessionStart> {
        Session::start_as(data_dir, std::process::id())
    }

    /// [`Session::start`] for process `pid` (tests start several in one process).
    pub fn start_as(data_dir: &Path, pid: u32) -> Result<SessionStart> {
        let dir = data_dir.join(MARKERS_DIR);
        let cannot = |what: String| {
            AppError::new("The app can't record that it has started")
                .why(what)
                .fix("Check that the disk is not full and that you can write to the app's data folder.")
        };
        fs::create_dir_all(&dir)
            .or_explain(|| cannot(format!("The folder {} can't be made.", dir.display())))?;
        let mut previous_crashed = false;
        // The lock alone decides: a marker with this run's own id whose lock
        // is free was left by an earlier process given the same id, a crash.
        for run in unfinished_runs(data_dir) {
            if run.running == Some(true) {
                continue;
            }
            previous_crashed = true;
            run.forget()?;
        }
        let markers = markers(data_dir);
        let lock = markers.hold(pid).or_explain(|| {
            cannot(format!(
                "The lock beside {} can't be taken.",
                markers.marker_path(pid).display()
            ))
        })?;
        let session = Session { pid, lock };
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
        let mut contents = format!("pid {}\nstarted {}\n", self.pid, crate::stamp::now());
        if let Some(project) = project {
            contents.push_str(&format!("project {}\n", project.display()));
        }
        let marker = self.lock.marker();
        fs::write(marker, contents).or_explain(|| {
            AppError::new("The app can't record that it has started")
                .why(format!("Writing {} failed.", marker.display()))
                .fix("Check that the disk is not full and that you can write to the app's data folder.")
        })
    }

    /// Records a clean exit.
    pub fn end(self) -> Result<()> {
        let marker = self.lock.marker().to_path_buf();
        self.lock.release().map_err(|err| {
            AppError::new("The app can't record a clean exit")
                .why(format!("Removing {} failed.", marker.display()))
                .fix("Nothing is lost; the next start may wrongly offer crash recovery.")
                .source(err)
        })
    }
}

/// The file the editor writes once its first window is on screen (see `updater::health`).
pub const STARTED_FILE: &str = "editor.started";

/// The editor's first window is on screen: writes `<version> <process id>`, so
/// the launcher knows this version starts properly.
pub fn report_started(data_dir: &Path, version: &str) -> Result<()> {
    fs::write(data_dir.join(STARTED_FILE), format!("{version} {}\n", std::process::id())).or_explain(|| {
        AppError::new("The editor can't report that it started")
            .why(format!("Writing to {} failed.", data_dir.display()))
            .fix("Check that the disk is not full. The editor works; only the update safety check is affected.")
    })
}

/// What the marker of a run that hasn't closed says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedRun {
    /// The editor's process id.
    pub pid: Option<u32>,
    /// The project it had open, if any.
    pub project: Option<PathBuf>,
    /// Whether it is still running (it holds its lock): `None` when its
    /// marker has no lock beside it (an older version's), when the caller
    /// asks the system about `pid` instead.
    pub running: Option<bool>,
    /// The marker file.
    pub marker: PathBuf,
}

impl UnfinishedRun {
    /// Whether it is still running, asking `is_running` about its process
    /// when its marker can't say.
    pub fn is_running(&self, is_running: impl Fn(u32) -> bool) -> bool {
        self.running.unwrap_or_else(|| self.pid.is_some_and(is_running))
    }

    /// Takes its marker (and lock file) away, once its crash is dealt with.
    pub fn forget(&self) -> Result<()> {
        remove_file(&self.marker).map_err(|err| {
            AppError::new("The note about an earlier run can't be cleared")
                .why(format!("Removing {} failed.", self.marker.display()))
                .fix("Nothing is lost; the editor may mention that run once more.")
                .source(err)
        })?;
        let _ = remove_file(&self.marker.with_extension(LOCK_EXTENSION));
        Ok(())
    }
}

/// Every editor run that hasn't closed cleanly: each still running or
/// crashed. Reading them changes nothing.
pub fn unfinished_runs(data_dir: &Path) -> Vec<UnfinishedRun> {
    let old = data_dir.join(OLD_MARKER_FILE);
    let old_running = holds_lock(&old.with_extension(LOCK_EXTENSION));
    std::iter::once((old, old_running))
        .chain(markers(data_dir).found().into_iter().map(|m| (m.path, m.running)))
        .filter_map(|(marker, running)| read_marker(marker, running))
        .collect()
}

fn read_marker(marker: PathBuf, running: Option<bool>) -> Option<UnfinishedRun> {
    let text = fs::read_to_string(&marker).ok()?;
    let mut run = UnfinishedRun {
        pid: None,
        project: None,
        running,
        marker,
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

/// Removes the markers of runs that are no longer going (`is_running`
/// answers for a marker without a lock) once their crash has been dealt
/// with, so the next editor start doesn't report it again. Markers of
/// running editors stay.
pub fn forget_unfinished_runs(data_dir: &Path, is_running: impl Fn(u32) -> bool) -> Result<()> {
    for run in unfinished_runs(data_dir) {
        if !run.is_running(&is_running) {
            run.forget()?;
        }
    }
    Ok(())
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
    fn a_start_without_an_end_counts_as_a_crash_once() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Session::start_as(tmp.path(), 100).unwrap();
        drop(first); // no end(): the process "died", its lock went with it
        let second = Session::start_as(tmp.path(), 200).unwrap();
        assert!(second.previous_crashed);
        let third = Session::start_as(tmp.path(), 300).unwrap();
        assert!(
            !third.previous_crashed,
            "reported once; the one still running is no crash"
        );
    }

    #[test]
    fn a_clean_end_is_not_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        Session::start(tmp.path()).unwrap().session.end().unwrap();
        let next = Session::start(tmp.path()).unwrap();
        assert!(!next.previous_crashed);
        assert_eq!(
            fs::read_dir(tmp.path().join(MARKERS_DIR)).unwrap().count(),
            2,
            "its own two files"
        );
    }

    #[test]
    fn two_runs_at_once_do_not_take_each_other_for_crashed() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Session::start_as(tmp.path(), 100).unwrap();
        a.session.note_project(Path::new("/videos/a.x")).unwrap();
        let b = Session::start_as(tmp.path(), 200).unwrap();
        assert!(!b.previous_crashed, "A is still running");
        b.session.note_project(Path::new("/videos/b.x")).unwrap();
        let runs = unfinished_runs(tmp.path());
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].project, Some(PathBuf::from("/videos/a.x")));
        assert_eq!(runs[1].project, Some(PathBuf::from("/videos/b.x")));
        assert!(runs.iter().all(|r| r.running == Some(true)));
        // B closes cleanly; A's marker stays.
        b.session.end().unwrap();
        assert_eq!(unfinished_runs(tmp.path()).len(), 1);
        // A crashes: the next start sees it.
        drop(a);
        let c = Session::start_as(tmp.path(), 300).unwrap();
        assert!(c.previous_crashed);
    }

    #[test]
    fn the_marker_names_the_process_and_project() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(unfinished_runs(tmp.path()).is_empty());
        let started = Session::start(tmp.path()).unwrap();
        let runs = unfinished_runs(tmp.path());
        assert_eq!(runs[0].pid, Some(std::process::id()));
        assert_eq!(runs[0].project, None);

        started.session.note_project(Path::new("/videos/trip.x")).unwrap();
        let runs = unfinished_runs(tmp.path());
        assert_eq!(runs[0].project, Some(PathBuf::from("/videos/trip.x")));
        forget_unfinished_runs(tmp.path(), |_| false).unwrap();
        assert_eq!(
            unfinished_runs(tmp.path()).len(),
            1,
            "a running editor's marker stays"
        );

        drop(started); // a crash: the marker stays, its lock is free
        let runs = unfinished_runs(tmp.path());
        assert_eq!(runs[0].running, Some(false));
        forget_unfinished_runs(tmp.path(), |_| true).unwrap();
        assert!(unfinished_runs(tmp.path()).is_empty());
    }

    #[test]
    fn a_crash_is_reported_when_the_next_run_gets_the_same_process_id() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Session::start_as(tmp.path(), 100).unwrap();
        first.session.note_project(Path::new("/videos/a.x")).unwrap();
        drop(first); // crashed; the system gives its id to the next editor
        let second = Session::start_as(tmp.path(), 100).unwrap();
        assert!(second.previous_crashed);
        let runs = unfinished_runs(tmp.path());
        assert_eq!(runs.len(), 1, "only the new run's own marker");
        assert_eq!((runs[0].project.clone(), runs[0].running), (None, Some(true)));
    }

    #[test]
    fn an_older_version_s_marker_still_counts() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(OLD_MARKER_FILE), "pid 42\nproject /v/old.x\n").unwrap();
        let runs = unfinished_runs(tmp.path());
        assert_eq!((runs[0].pid, runs[0].running), (Some(42), None));
        assert!(runs[0].is_running(|pid| pid == 42), "asked about its process");
        let started = Session::start_as(tmp.path(), 7).unwrap();
        assert!(started.previous_crashed);
        assert!(!tmp.path().join(OLD_MARKER_FILE).exists());
    }
}
