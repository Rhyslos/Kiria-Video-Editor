//! The files that belong to a project besides the project file itself.
//!
//! A project is more than one file on disk: its autosave file, the copies
//! kept before upgrades, its picture for the project list, how its timeline
//! was laid out and its saved versions all sit beside it, under names made
//! from the project file's name. This is the one list of them, so moving,
//! renaming and deleting a project all carry the same files.
//!
//! Imported recordings are not on the list and never will be: a project
//! only points at them.
//!
//! A new kind of file kept beside projects gets a [`SideFile`] here (and a
//! line in [`existing`]); everything that moves or deletes projects then
//! handles it.

use crate::{document, file, versions};
use std::path::{Path, PathBuf};

/// One kind of file (or folder) kept beside a project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SideFile {
    /// `<file>.journal`: every change since the last full save.
    Journal,
    /// `<file>.journal.discarded`: unsaved work set aside by "Open last save".
    DiscardedJournal,
    /// `.<file>.saving`: a save that a crash or power cut interrupted.
    UnfinishedSave,
    /// `<file>.backup-v<N>`: the file as it was before an upgrade from format `N`.
    Backup(u32),
    /// `<file>.thumbnail.png`: the picture on the project's card.
    Thumbnail,
    /// `<file>.view`: track heights and the video/audio divider.
    View,
    /// `<file>.versions/`: the folder of saved versions.
    Versions,
}

impl SideFile {
    /// Where this file is for the project at `project_file`.
    pub fn path(self, project_file: &Path) -> PathBuf {
        match self {
            SideFile::Journal => document::journal_path(project_file),
            SideFile::DiscardedJournal => document::discarded_journal_path(project_file),
            SideFile::UnfinishedSave => file::temp_path(project_file),
            SideFile::Backup(format) => file::backup_path(project_file, format),
            SideFile::Thumbnail => document::thumbnail_path(project_file),
            SideFile::View => document::view_path(project_file),
            SideFile::Versions => versions::folder(project_file),
        }
    }

    /// What it is, in a few words a list can show.
    pub fn label(self) -> &'static str {
        match self {
            SideFile::Journal => "Autosave file",
            SideFile::DiscardedJournal => "Unsaved work set aside after a crash",
            SideFile::UnfinishedSave => "Unfinished save",
            SideFile::Backup(_) => "Backup from before an upgrade",
            SideFile::Thumbnail => "Picture for the project list",
            SideFile::View => "Timeline layout",
            SideFile::Versions => "Saved versions",
        }
    }
}

/// A side file that is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub kind: SideFile,
    pub path: PathBuf,
}

/// The side files of the project at `project_file` that exist, whether or
/// not the project file itself does. Looks at the names in the project's
/// folder and opens nothing; still, run it on a worker for a folder on a
/// slow disk.
pub fn existing(project_file: &Path) -> Vec<Found> {
    let mut kinds = vec![
        SideFile::Journal,
        SideFile::DiscardedJournal,
        SideFile::UnfinishedSave,
    ];
    kinds.extend(backups(project_file).into_iter().map(SideFile::Backup));
    kinds.extend([SideFile::Thumbnail, SideFile::View, SideFile::Versions]);
    kinds
        .into_iter()
        .map(|kind| Found {
            kind,
            path: kind.path(project_file),
        })
        // A link counts even when what it points at is gone.
        .filter(|found| found.path.symlink_metadata().is_ok())
        .collect()
}

/// The formats there is a backup copy for, oldest first. Their names hold a
/// number, so the folder is listed to find them.
fn backups(project_file: &Path) -> Vec<u32> {
    let (Some(folder), Some(name)) = (project_file.parent(), project_file.file_name()) else {
        return Vec::new();
    };
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let mut formats: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| backup_format(&entry.file_name().to_string_lossy(), &name))
        .collect();
    formats.sort_unstable();
    formats.dedup();
    formats
}

/// The format in `candidate` when it is `<project name>.backup-v<format>`
/// and nothing more. Windows file names ignore case, so the match does too
/// there.
fn backup_format(candidate: &str, project_name: &str) -> Option<u32> {
    let split = project_name.len();
    if !candidate.is_char_boundary(split) {
        return None;
    }
    let (start, rest) = candidate.split_at(split);
    let same = if cfg!(windows) {
        start.eq_ignore_ascii_case(project_name)
    } else {
        start == project_name
    };
    let digits = rest.strip_prefix(".backup-v").filter(|_| same)?;
    // Only the name `backup_path` writes: no sign, no leading zero.
    let format: u32 = digits.parse().ok()?;
    (format.to_string() == digits).then_some(format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FILE_EXTENSION, Project};
    use std::fs;

    fn project_in(dir: &Path, stem: &str) -> PathBuf {
        let path = dir.join(format!("{stem}.{FILE_EXTENSION}"));
        file::save(&Project::new_automatic(stem), &path).unwrap();
        path
    }

    /// Writes every kind of side file the way the code that owns it does.
    fn fill(project_file: &Path) {
        fs::write(document::journal_path(project_file), "").unwrap();
        fs::write(document::discarded_journal_path(project_file), "").unwrap();
        fs::write(file::temp_path(project_file), "half a save").unwrap();
        file::back_up_before_upgrade(project_file, 2).unwrap();
        file::back_up_before_upgrade(project_file, 11).unwrap();
        fs::write(document::thumbnail_path(project_file), "png").unwrap();
        fs::write(document::view_path(project_file), "()").unwrap();
        versions::save(&Project::new_automatic("v"), project_file, "First cut").unwrap();
    }

    #[test]
    fn a_new_project_has_no_side_files() {
        let tmp = tempfile::tempdir().unwrap();
        let path = project_in(tmp.path(), "p");
        assert!(existing(&path).is_empty());
        assert!(
            existing(&tmp.path().join("no such file")).is_empty(),
            "a project that is gone has none either"
        );
    }

    #[test]
    fn every_file_written_beside_a_project_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = project_in(tmp.path(), "p");
        fill(&path);
        let found = existing(&path);
        let kinds: Vec<SideFile> = found.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [
                SideFile::Journal,
                SideFile::DiscardedJournal,
                SideFile::UnfinishedSave,
                SideFile::Backup(2),
                SideFile::Backup(11),
                SideFile::Thumbnail,
                SideFile::View,
                SideFile::Versions,
            ]
        );
        assert!(found.iter().all(|f| f.path == f.kind.path(&path)));

        // Nothing else is in the folder: the list is complete.
        let mut on_disk: Vec<PathBuf> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| *p != path)
            .collect();
        on_disk.sort();
        let mut listed: Vec<PathBuf> = found.into_iter().map(|f| f.path).collect();
        listed.sort();
        assert_eq!(listed, on_disk);
    }

    #[test]
    fn a_document_that_crashed_and_was_reopened_leaves_only_listed_files() {
        use crate::{Command, Document, Recovery, SequenceSettings, Time};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("p.{FILE_EXTENSION}"));
        let mut doc = Document::create(&path, Project::new("P", SequenceSettings::GAME_CLIP)).unwrap();
        let sequence = doc.project().active_sequence;
        doc.apply(Command::SetEditNeedle {
            sequence,
            at: Time::ZERO,
        })
        .unwrap();
        drop(doc); // a crash: the change is only in the journal
        drop(Document::open(&path, Recovery::OpenLastSave).unwrap());

        let listed: Vec<PathBuf> = existing(&path).into_iter().map(|f| f.path).collect();
        for entry in fs::read_dir(tmp.path()).unwrap() {
            let entry = entry.unwrap().path();
            assert!(
                entry == path || listed.contains(&entry),
                "{} is beside the project but not on the list",
                entry.display()
            );
        }
        assert!(listed.contains(&document::discarded_journal_path(&path)));
    }

    #[test]
    fn other_files_in_the_folder_are_never_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = project_in(tmp.path(), "p");
        let neighbour = project_in(tmp.path(), "p 2");
        fill(&neighbour);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let others = [
            "recording.mkv".to_owned(),
            format!("{name}.mkv"),
            format!("{name}.backup-video.mkv"),
            format!("{name}.backup-v2.mkv"),
            format!("{name}.backup-v"),
            format!("{name}.backup-v02"),
            format!("{name}.backup-v+3"),
            format!("x{name}.backup-v3"),
            format!("{name}.thumbnail.png.bak"),
        ];
        for other in &others {
            fs::write(tmp.path().join(other), "not ours").unwrap();
        }
        assert!(existing(&path).is_empty(), "{:?}", existing(&path));
    }

    #[test]
    fn backup_names_are_matched_exactly() {
        assert_eq!(backup_format("p.x.backup-v3", "p.x"), Some(3));
        assert_eq!(backup_format("p.x.backup-v12", "p.x"), Some(12));
        assert_eq!(backup_format("p.x.backup-v3 copy", "p.x"), None);
        assert_eq!(backup_format("p.x", "p.x"), None);
        assert_eq!(
            backup_format("aé.backup-v3", "ab"),
            None,
            "never cuts a letter in half"
        );
        assert_eq!(backup_format("p", "p.x"), None, "shorter than the project's name");
        assert_eq!(
            backup_format("P.X.backup-v3", "p.x"),
            cfg!(windows).then_some(3),
            "case only matters where file names keep it apart"
        );
    }

    #[test]
    fn paths_follow_the_project_file() {
        let old = Path::new("a").join("old.x");
        let new = Path::new("b").join("new.x");
        assert_eq!(SideFile::View.path(&new), Path::new("b").join("new.x.view"));
        assert_eq!(
            SideFile::Backup(4).path(&new),
            Path::new("b").join("new.x.backup-v4")
        );
        assert_eq!(
            SideFile::UnfinishedSave.path(&old),
            Path::new("a").join(".old.x.saving")
        );
        assert_eq!(
            SideFile::DiscardedJournal.path(&old),
            Path::new("a").join("old.x.journal.discarded")
        );
    }
}
