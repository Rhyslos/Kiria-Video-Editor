//! Unsaved work left behind by a crash.
//!
//! Two signals, both cheap to read:
//! - the editors' session markers ([`diag::session::unfinished_runs`], one
//!   per editor process) say a run didn't close and which project it had open;
//! - a project's autosave journal with changes newer than its last save
//!   ([`project::Document::check`]) says that project has work to recover.
//!
//! The launcher offers the choice the editor can't: recover the work, or open
//! the project as last saved (the work is kept aside, never deleted).

use std::path::{Path, PathBuf};

/// What to tell the person at start-up about the last editor run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastRun {
    /// Closed normally, or still running: nothing to say.
    Fine,
    /// Crashed with no project open, or with nothing unsaved.
    CrashedNothingLost,
    /// Crashed with unsaved changes in this project.
    CrashedWithWork { project: PathBuf, unsaved: usize },
}

/// Looks at the editors' session markers in `data_dir`: the first run whose
/// editor is no longer going (editors still running are fine). Reads a few
/// small files and, for a crash, the project's journal.
pub fn last_run(data_dir: &Path) -> LastRun {
    let Some(run) = diag::session::unfinished_runs(data_dir)
        .into_iter()
        .find(|run| !run.is_running(editor_is_running))
    else {
        return LastRun::Fine;
    };
    match run.project {
        Some(project) => match unsaved_changes(&project) {
            0 => LastRun::CrashedNothingLost,
            unsaved => LastRun::CrashedWithWork { project, unsaved },
        },
        None => LastRun::CrashedNothingLost,
    }
}

/// How many changes a crash left unsaved in this project (0 when none, or
/// when the project can't be read: opening it explains why).
pub fn unsaved_changes(project: &Path) -> usize {
    if !project::document::journal_path(project).exists() {
        return 0;
    }
    project::Document::check(project).map_or(0, |check| check.unsaved_changes)
}

/// Refuses with an explanation while the editor has `project` open: its
/// files are in use, and the editor would go on saving to the old place.
/// `doing` finishes "can't be … right now" ("renamed", "moved", "deleted").
/// Asks the system about one process: run it on a worker.
pub fn refuse_if_open_in_editor(data_dir: &Path, project: &Path, doing: &str) -> diag::Result<()> {
    if open_in_editor(data_dir, project, editor_is_running) {
        return Err(in_use(doing));
    }
    Ok(())
}

fn in_use(doing: &str) -> diag::AppError {
    diag::AppError::new("This project is open in the editor")
        .why(format!(
            "The editor is working on its files, so it can't be {doing} right now."
        ))
        .fix("Close the editor, then try again.")
}

/// Whether an editor's session marker names `project` and that editor is
/// still running (`is_running` answers for its process id). An editor that
/// crashed leaves its marker too, and is not in the way.
fn open_in_editor(data_dir: &Path, project: &Path, is_running: impl Fn(u32) -> bool) -> bool {
    diag::session::unfinished_runs(data_dir).into_iter().any(|run| {
        run.project
            .as_deref()
            .is_some_and(|open| same_file(open, project))
            && run.is_running(&is_running)
    })
}

/// Whether two paths name one file, also when they are spelled differently
/// (another letter case, a link on the way).
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Removes the markers of editors that crashed, once the crash has been
/// dealt with; those of editors still running stay.
pub fn forget_crashed_runs(data_dir: &Path) -> diag::Result<()> {
    diag::session::forget_unfinished_runs(data_dir, editor_is_running)
}

/// Whether a process with this id is running and is the editor.
fn editor_is_running(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(mut system) = System::new() else {
        // Can't tell: assume it's running rather than offer a false recovery.
        return true;
    };
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(pid)
        .is_some_and(|p| p.name().to_string_lossy().to_lowercase().starts_with("editor"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Document, Project, SequenceSettings, Time};

    /// A project with one change in its journal, as a crash leaves it.
    fn crashed_project(dir: &Path) -> PathBuf {
        let path = dir.join(format!("p.{}", project::FILE_EXTENSION));
        let mut doc = Document::create(&path, Project::new("P", SequenceSettings::GAME_CLIP)).unwrap();
        let sequence = doc.project().active_sequence;
        doc.apply(Command::SetEditNeedle {
            sequence,
            at: Time::ZERO,
        })
        .unwrap();
        drop(doc);
        path
    }

    fn write_marker(dir: &Path, pid: u32, project: Option<&Path>) {
        let mut text = format!("pid {pid}\nstarted 2026-11-10T20:00:00Z\n");
        if let Some(p) = project {
            text.push_str(&format!("project {}\n", p.display()));
        }
        let markers = dir.join("sessions");
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join(format!("{pid}.running")), text).unwrap();
    }

    /// A process id that is never in use.
    const NO_SUCH_PROCESS: u32 = u32::MAX - 7;

    #[test]
    fn no_marker_means_nothing_to_say() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(last_run(tmp.path()), LastRun::Fine);
    }

    #[test]
    fn a_crash_with_unsaved_work_names_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = crashed_project(tmp.path());
        assert_eq!(unsaved_changes(&project), 1);
        write_marker(tmp.path(), NO_SUCH_PROCESS, Some(&project));
        assert_eq!(
            last_run(tmp.path()),
            LastRun::CrashedWithWork { project, unsaved: 1 }
        );
    }

    #[test]
    fn a_crash_without_a_project_lost_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), NO_SUCH_PROCESS, None);
        assert_eq!(last_run(tmp.path()), LastRun::CrashedNothingLost);
    }

    #[test]
    fn a_running_process_is_not_a_crash() {
        // This test's own process is running, but it isn't the editor, so it
        // only counts when the name matches: the marker is treated as a crash.
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), std::process::id(), None);
        assert_eq!(last_run(tmp.path()), LastRun::CrashedNothingLost);
    }

    #[test]
    fn a_project_is_open_only_while_its_editor_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let project = crate::files::create_project(tmp.path(), "Open", None).unwrap();
        let other = crate::files::create_project(tmp.path(), "Other", None).unwrap();
        let (running, stopped) = (|_| true, |_| false);
        assert!(!open_in_editor(tmp.path(), &project, running), "no editor at all");

        write_marker(tmp.path(), 4242, Some(&project));
        assert!(open_in_editor(tmp.path(), &project, |pid| pid == 4242));
        assert!(!open_in_editor(tmp.path(), &other, running), "another project");
        assert!(!open_in_editor(tmp.path(), &project, stopped), "it crashed");
        // The same file, spelled another way.
        let roundabout = tmp.path().join(".").join(project.file_name().unwrap());
        assert!(open_in_editor(tmp.path(), &roundabout, running));

        write_marker(tmp.path(), 4242, None);
        assert!(!open_in_editor(tmp.path(), &project, running), "no project open");
    }

    #[test]
    fn an_open_project_is_refused_with_an_explanation() {
        let tmp = tempfile::tempdir().unwrap();
        let project = crate::files::create_project(tmp.path(), "Open", None).unwrap();
        assert!(refuse_if_open_in_editor(tmp.path(), &project, "deleted").is_ok());
        // This test's own process is running but is not the editor.
        write_marker(tmp.path(), std::process::id(), Some(&project));
        assert!(refuse_if_open_in_editor(tmp.path(), &project, "deleted").is_ok());
        let err = in_use("deleted");
        assert!(err.what().contains("open in the editor"));
        assert!(err.why_text().is_some_and(|w| w.contains("can't be deleted")));
    }

    #[test]
    fn a_saved_project_has_nothing_to_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let path =
            crate::files::create_project(tmp.path(), "Saved", Some(SequenceSettings::GAME_CLIP)).unwrap();
        assert_eq!(unsaved_changes(&path), 0);
    }
}
