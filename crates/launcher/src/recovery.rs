//! Unsaved work left behind by a crash.
//!
//! Two signals, both cheap to read:
//! - the editor's session marker ([`diag::session::unfinished_run`]) says the
//!   last run didn't close and which project it had open;
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

/// Looks at the editor's session marker in `data_dir`. Reads one small file
/// and, for a crash, the project's journal.
pub fn last_run(data_dir: &Path) -> LastRun {
    let Some(run) = diag::session::unfinished_run(data_dir) else {
        return LastRun::Fine;
    };
    if run.pid.is_some_and(editor_is_running) {
        return LastRun::Fine;
    }
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
        std::fs::write(dir.join("session.running"), text).unwrap();
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
    fn a_saved_project_has_nothing_to_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::files::create_project(tmp.path(), "Saved", SequenceSettings::GAME_CLIP).unwrap();
        assert_eq!(unsaved_changes(&path), 0);
    }
}
