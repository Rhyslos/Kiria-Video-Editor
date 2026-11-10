//! Creating, renaming and moving project files on disk.
//!
//! Renames and moves read and write whole files, so the browser runs them on
//! a worker thread. A project's autosave journal travels with it.

use diag::{AppError, Explain};
use project::{Document, Project, SequenceSettings};
use std::path::{Path, PathBuf};

/// A file name for a project called `name`: characters that file systems
/// refuse become dashes, and an empty name becomes "Untitled".
pub fn file_name_for(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    let stem = if trimmed.is_empty() { "Untitled" } else { trimmed };
    format!("{stem}.{}", project::FILE_EXTENSION)
}

/// `folder/file_name`, or "name 2", "name 3"… if that is taken. `keep` is a
/// path that may be reused (the file being renamed).
pub fn free_path(folder: &Path, file_name: &str, keep: Option<&Path>) -> PathBuf {
    let first = folder.join(file_name);
    if !first.exists() || keep == Some(first.as_path()) {
        return first;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map_or_else(String::new, |e| e.to_string_lossy().into_owned());
    (2..)
        .map(|n| folder.join(format!("{stem} {n}.{ext}")))
        .find(|p| !p.exists() || keep == Some(p.as_path()))
        .unwrap_or(first)
}

/// Saves a new, empty project and returns where it went.
pub fn create_project(folder: &Path, name: &str, settings: SequenceSettings) -> diag::Result<PathBuf> {
    std::fs::create_dir_all(folder).or_explain(|| {
        AppError::new("The project can't be created")
            .why(format!("The folder {} could not be made.", folder.display()))
            .fix("Choose another folder, or check that you can write to this one.")
    })?;
    let name = if name.trim().is_empty() {
        "Untitled"
    } else {
        name.trim()
    };
    let path = free_path(folder, &file_name_for(name), None);
    project::file::save(&Project::new(name, settings), &path)?;
    Ok(path)
}

/// Gives a project a new name, inside the file and on disk. Returns the new path.
pub fn rename_project(path: &Path, new_name: &str) -> diag::Result<PathBuf> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::new("A project needs a name").fix("Type a name, then press Rename."));
    }
    refuse_unsaved_work(path)?;
    let mut project = project::file::load(path)?;
    project.name = new_name.to_owned();
    let folder = path.parent().unwrap_or(Path::new("."));
    let target = free_path(folder, &file_name_for(new_name), Some(path));
    project::file::save(&project, &target)?;
    if target != path {
        move_journal(path, &target)?;
        std::fs::remove_file(path).or_explain(|| {
            AppError::new("The old project file is still there")
                .why(format!(
                    "The renamed copy was saved, but {} could not be removed.",
                    path.display()
                ))
                .fix("Delete the old file yourself; the renamed project is complete.")
        })?;
    }
    Ok(target)
}

/// Moves a project file into another folder. Returns the new path.
pub fn move_project(path: &Path, folder: &Path) -> diag::Result<PathBuf> {
    refuse_unsaved_work(path)?;
    let file_name = path
        .file_name()
        .map_or_else(|| file_name_for("Untitled"), |n| n.to_string_lossy().into_owned());
    let target = free_path(folder, &file_name, Some(path));
    if target == path {
        return Ok(target);
    }
    let explain = || {
        AppError::new("The project can't be moved")
            .why(format!("Moving it to {} failed.", folder.display()))
            .fix("Check that the folder exists and that you can write to it.")
    };
    std::fs::create_dir_all(folder).or_explain(explain)?;
    if std::fs::rename(path, &target).is_err() {
        // Another drive: copy, then remove the original once the copy is complete.
        std::fs::copy(path, &target).or_explain(explain)?;
        std::fs::remove_file(path).or_explain(explain)?;
    }
    move_journal(path, &target)?;
    Ok(target)
}

/// Renaming or moving a project with unrecovered crash work would separate it
/// from that work, so it must be opened (and recovered) first.
fn refuse_unsaved_work(path: &Path) -> diag::Result<()> {
    if Document::check(path)?.unsaved_changes > 0 {
        return Err(AppError::new("This project has unsaved work from a crash")
            .why("Changes made before the app closed unexpectedly are waiting to be recovered.")
            .fix("Open the project once so the work is recovered, then rename or move it."));
    }
    Ok(())
}

fn move_journal(from: &Path, to: &Path) -> diag::Result<()> {
    let (old, new) = (
        project::document::journal_path(from),
        project::document::journal_path(to),
    );
    if old.exists() {
        std::fs::rename(&old, &new)
            .or_else(|_| std::fs::copy(&old, &new).and_then(|_| std::fs::remove_file(&old)))
            .or_explain(|| {
                AppError::new("The project's autosave file was left behind")
                    .why(format!("{} could not be moved.", old.display()))
                    .fix("Nothing is lost: the project itself was saved in full.")
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext() -> &'static str {
        project::FILE_EXTENSION
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name_for("Trip: day 1/2"), format!("Trip- day 1-2.{}", ext()));
        assert_eq!(file_name_for("  "), format!("Untitled.{}", ext()));
        assert_eq!(
            file_name_for("ends with dots..."),
            format!("ends with dots.{}", ext())
        );
    }

    #[test]
    fn taken_names_get_a_number() {
        let tmp = tempfile::tempdir().unwrap();
        let name = file_name_for("Clip");
        let first = free_path(tmp.path(), &name, None);
        std::fs::write(&first, "").unwrap();
        let second = free_path(tmp.path(), &name, None);
        assert_eq!(second, tmp.path().join(format!("Clip 2.{}", ext())));
        assert_eq!(
            free_path(tmp.path(), &name, Some(&first)),
            first,
            "a file may keep its own name"
        );
    }

    #[test]
    fn create_rename_and_move() {
        let tmp = tempfile::tempdir().unwrap();
        let path = create_project(tmp.path(), "Highlights", SequenceSettings::VERTICAL_SHORT).unwrap();
        assert_eq!(
            project::file::read_summary(&path).unwrap().unwrap().name,
            "Highlights"
        );

        let renamed = rename_project(&path, "Best of week").unwrap();
        assert!(!path.exists());
        assert_eq!(
            renamed.file_name().unwrap().to_string_lossy(),
            format!("Best of week.{}", ext())
        );
        let loaded = project::file::load(&renamed).unwrap();
        assert_eq!(loaded.name, "Best of week");
        assert_eq!(
            loaded.active().unwrap().settings,
            SequenceSettings::VERTICAL_SHORT
        );

        let elsewhere = tmp.path().join("archive");
        let moved = move_project(&renamed, &elsewhere).unwrap();
        assert!(!renamed.exists());
        assert_eq!(moved.parent().unwrap(), elsewhere);
        assert_eq!(project::file::load(&moved).unwrap().name, "Best of week");
    }

    #[test]
    fn crash_work_blocks_renaming_until_recovered() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("p.{}", ext()));
        let mut doc = Document::create(&path, Project::new("P", SequenceSettings::GAME_CLIP)).unwrap();
        let sequence = doc.project().active_sequence;
        doc.apply(project::Command::SetEditNeedle {
            sequence,
            at: project::Time::ZERO,
        })
        .unwrap();
        drop(doc); // as if the app crashed: the change is only in the journal
        let err = rename_project(&path, "Other").unwrap_err();
        assert!(err.what().contains("unsaved work"), "{}", err.report());
    }
}
