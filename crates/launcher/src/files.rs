//! Creating, renaming and moving project files on disk.
//!
//! Renames and moves read and write whole files, so the browser runs them on
//! a worker thread. Everything that belongs to a project travels with it:
//! the autosave file, backups, its picture, the timeline layout and saved
//! versions (the list is `project::side_files`).

use diag::{AppError, Explain};
use project::{Document, Project, SequenceSettings};
use std::path::{Path, PathBuf};

/// A project that was renamed or moved.
#[derive(Debug)]
pub struct Moved {
    /// Where the project file is now.
    pub to: PathBuf,
    /// Set when some of the project's other files could not follow it. The
    /// project itself moved in full; this says what to pick up by hand.
    pub left_behind: Option<AppError>,
}

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
///
/// A name also counts as taken while files that would belong to a project of
/// that name are there (left by a project file removed by hand): a project
/// put there would take them for its own, or overwrite them.
pub fn free_path(folder: &Path, file_name: &str, keep: Option<&Path>) -> PathBuf {
    let free = |p: &Path| keep == Some(p) || !(p.exists() || has_side_files(p));
    let first = folder.join(file_name);
    if free(&first) {
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
        .find(|p| free(p))
        .unwrap_or(first)
}

fn has_side_files(project_file: &Path) -> bool {
    !project::side_files::existing(project_file).is_empty()
}

/// Saves a new, empty project and returns where it went.
/// `settings` of `None` makes an Automatic project: the first video imported sets them.
pub fn create_project(
    folder: &Path,
    name: &str,
    settings: Option<SequenceSettings>,
) -> diag::Result<PathBuf> {
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
    let project = match settings {
        Some(settings) => Project::new(name, settings),
        None => Project::new_automatic(name),
    };
    project::file::save(&project, &path)?;
    Ok(path)
}

/// Gives a project a new name, inside the file and on disk. The files that
/// belong to it are renamed to match.
pub fn rename_project(path: &Path, new_name: &str) -> diag::Result<Moved> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::new("A project needs a name").fix("Type a name, then press Rename."));
    }
    refuse_unsaved_work(path)?;
    // The autosave file comes along, so the renamed file must say the same
    // as the old one about how much of it is already saved.
    let (mut project, journal_seq) = project::file::load_with_journal_seq(path)?;
    project.name = new_name.to_owned();
    let folder = path.parent().unwrap_or(Path::new("."));
    let target = free_path(folder, &file_name_for(new_name), Some(path));
    project::file::save_with_journal_seq(&project, &target, journal_seq)?;
    let mut left_behind = None;
    if target != path {
        left_behind = carry_side_files(path, &target);
        std::fs::remove_file(path).or_explain(|| {
            AppError::new("The old project file is still there")
                .why(format!(
                    "The renamed copy was saved, but {} could not be removed.",
                    path.display()
                ))
                .fix("Delete the old file yourself; the renamed project is complete.")
        })?;
    }
    Ok(Moved {
        to: target,
        left_behind,
    })
}

/// Moves a project file, and the files that belong to it, into another folder.
pub fn move_project(path: &Path, folder: &Path) -> diag::Result<Moved> {
    refuse_unsaved_work(path)?;
    let file_name = path
        .file_name()
        .map_or_else(|| file_name_for("Untitled"), |n| n.to_string_lossy().into_owned());
    let target = free_path(folder, &file_name, Some(path));
    if target == path {
        return Ok(Moved {
            to: target,
            left_behind: None,
        });
    }
    let explain = || {
        AppError::new("The project can't be moved")
            .why(format!("Moving it to {} failed.", folder.display()))
            .fix("Check that the folder exists and that you can write to it.")
    };
    std::fs::create_dir_all(folder).or_explain(explain)?;
    move_path(path, &target).or_explain(explain)?;
    Ok(Moved {
        left_behind: carry_side_files(path, &target),
        to: target,
    })
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

/// Moves every file that belongs to the project that was at `from` to
/// where the project at `to` expects it. The project file itself has moved
/// already, so a file that won't move is no reason to stop: the others still
/// go, and what stayed is reported.
fn carry_side_files(from: &Path, to: &Path) -> Option<AppError> {
    let mut stayed = Vec::new();
    for found in project::side_files::existing(from) {
        if let Err(err) = move_path(&found.path, &found.kind.path(to)) {
            tracing::warn!(path = %found.path.display(), "a project's file could not follow it: {err}");
            stayed.push(found.path);
        }
    }
    if stayed.is_empty() {
        return None;
    }
    let names: Vec<String> = stayed
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let folder = from.parent().unwrap_or(Path::new("."));
    Some(
        AppError::new("Some of the project's files were left behind")
            .why(format!(
                "The project is at {} now, but these could not follow it from {}: {}.",
                to.display(),
                folder.display(),
                names.join(", ")
            ))
            .fix("The project itself is complete. To keep what stayed (saved versions, backups, the timeline layout), move it beside the project in the file manager, named after the project file."),
    )
}

/// Moves a file or folder, never over something that is already there. Onto
/// another drive it is copied, and the original removed once the copy is
/// complete.
fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.symlink_metadata().is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} is in the way", to.display()),
        ));
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        copy_folder(from, to)?;
        std::fs::remove_dir_all(from)
    } else {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)
    }
}

fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
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
        let path = create_project(tmp.path(), "Highlights", Some(SequenceSettings::VERTICAL_SHORT)).unwrap();
        assert_eq!(
            project::file::read_summary(&path).unwrap().unwrap().name,
            "Highlights"
        );

        let renamed = rename_project(&path, "Best of week").unwrap();
        assert!(renamed.left_behind.is_none());
        let renamed = renamed.to;
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
        assert!(moved.left_behind.is_none());
        let moved = moved.to;
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

    /// One of every file kept beside a project, written by the code that owns it.
    fn give_side_files(path: &Path) {
        use project::SideFile;
        for kind in [
            SideFile::DiscardedJournal,
            SideFile::UnfinishedSave,
            SideFile::Thumbnail,
            SideFile::View,
        ] {
            std::fs::write(kind.path(path), kind.label()).unwrap();
        }
        project::file::back_up_before_upgrade(path, 3).unwrap();
        let project = project::file::load(path).unwrap();
        project::versions::save(&project, path, "First cut").unwrap();
        // Opening and closing leaves the (empty) autosave file.
        drop(Document::open(path, project::Recovery::Recover).unwrap());
    }

    /// The names in a folder, sorted, with folders marked by a slash.
    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let slash = if e.path().is_dir() { "/" } else { "" };
                format!("{}{slash}", e.file_name().to_string_lossy())
            })
            .collect();
        names.sort();
        names
    }

    fn side_file_names(stem: &str) -> Vec<String> {
        let file = format!("{stem}.{}", ext());
        let mut names = vec![
            format!(".{file}.saving"),
            file.clone(),
            format!("{file}.backup-v3"),
            format!("{file}.journal"),
            format!("{file}.journal.discarded"),
            format!("{file}.thumbnail.png"),
            format!("{file}.versions/"),
            format!("{file}.view"),
        ];
        names.sort();
        names
    }

    #[test]
    fn renaming_and_moving_carry_everything_that_belongs_to_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (tmp.path().join("here"), tmp.path().join("there"));
        let path = create_project(&here, "Clip", None).unwrap();
        give_side_files(&path);
        std::fs::write(here.join("recording.mkv"), "a recording").unwrap();
        assert_eq!(
            names_in(&here),
            [side_file_names("Clip"), vec!["recording.mkv".into()]].concat()
        );
        let view = std::fs::read(project::view_path(&path)).unwrap();

        let renamed = rename_project(&path, "Best of").unwrap();
        assert!(renamed.left_behind.is_none());
        let renamed = renamed.to;
        assert_eq!(
            names_in(&here),
            [side_file_names("Best of"), vec!["recording.mkv".into()]].concat(),
            "nothing keeps the old name, and the recording is not touched"
        );
        assert_eq!(std::fs::read(project::view_path(&renamed)).unwrap(), view);
        assert_eq!(project::versions::list(&renamed).len(), 1);

        let moved = move_project(&renamed, &there).unwrap();
        assert!(moved.left_behind.is_none());
        assert_eq!(names_in(&there), side_file_names("Best of"));
        assert_eq!(names_in(&here), ["recording.mkv"], "only the recording stays");
        let versions = project::versions::list(&moved.to);
        assert_eq!(versions.len(), 1);
        assert_eq!(project::versions::load(&versions[0]).unwrap().name, "Clip");
        assert_eq!(Document::check(&moved.to).unwrap().unsaved_changes, 0);
    }

    #[test]
    fn a_name_whose_side_files_are_still_there_is_not_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = create_project(tmp.path(), "Clip", None).unwrap();
        give_side_files(&gone);
        std::fs::remove_file(&gone).unwrap(); // removed by hand: its files stay behind
        let versions_before = names_in(&project::versions::folder(&gone));

        let other = create_project(tmp.path(), "Other", None).unwrap();
        give_side_files(&other);
        let renamed = rename_project(&other, "Clip").unwrap();
        assert!(renamed.left_behind.is_none());
        assert_eq!(
            renamed.to,
            tmp.path().join(format!("Clip 2.{}", ext())),
            "the name is taken while its files are there"
        );
        assert_eq!(
            names_in(&project::versions::folder(&gone)),
            versions_before,
            "the other project's saved versions are not overwritten"
        );
        assert_eq!(
            create_project(tmp.path(), "Clip", None).unwrap(),
            tmp.path().join(format!("Clip 3.{}", ext())),
            "a new project doesn't adopt them either"
        );
    }

    #[test]
    fn a_file_that_cannot_follow_is_reported_and_the_rest_still_moves() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (tmp.path().join("here"), tmp.path().join("there"));
        let path = create_project(&here, "Clip", None).unwrap();
        give_side_files(&path);
        // Something is in the way where the saved versions should go.
        std::fs::create_dir_all(&there).unwrap();
        let target = there.join(path.file_name().unwrap());
        let blocked = project::versions::folder(&target);
        std::fs::create_dir_all(blocked.join("in the way")).unwrap();
        std::fs::write(blocked.join("in the way").join("file"), "x").unwrap();
        // The check for a free name is what normally prevents this.
        let moved = Moved {
            left_behind: {
                move_path(&path, &target).unwrap();
                carry_side_files(&path, &target)
            },
            to: target.clone(),
        };
        let problem = moved.left_behind.expect("the versions folder could not move");
        assert!(
            problem.why_text().unwrap().contains(".versions"),
            "{}",
            problem.report()
        );
        assert!(project::versions::folder(&path).is_dir(), "it stays, complete");
        assert!(project::view_path(&target).exists(), "the others moved");
        assert!(project::thumbnail_path(&target).exists());
        assert!(!project::view_path(&path).exists());
    }

    #[test]
    fn folders_are_copied_whole_when_they_cannot_be_renamed() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        std::fs::create_dir_all(from.join("deeper")).unwrap();
        std::fs::write(from.join("a"), "a").unwrap();
        std::fs::write(from.join("deeper").join("b"), "b").unwrap();
        let to = tmp.path().join("to");
        copy_folder(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("a")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(to.join("deeper").join("b")).unwrap(), "b");
    }

    #[test]
    fn renaming_keeps_track_of_what_the_autosave_file_already_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("p.{}", ext()));
        let mut doc = Document::create(&path, Project::new("P", SequenceSettings::GAME_CLIP)).unwrap();
        let sequence = doc.project().active_sequence;
        doc.apply(project::Command::SetEditNeedle {
            sequence,
            at: project::Time::from_ratio(1, 2).unwrap(),
        })
        .unwrap();
        doc.prepare_save().run().unwrap();
        drop(doc); // closed after the save, before the autosave file was emptied
        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 0);

        let renamed = rename_project(&path, "Other").unwrap().to;
        assert_eq!(
            Document::check(&renamed).unwrap().unsaved_changes,
            0,
            "saved changes are not offered for recovery again"
        );
    }
}
