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

use crate::{disk, document, file, versions};
use std::path::{Path, PathBuf};

/// One kind of file (or folder) kept beside a project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SideFile {
    /// `<file>.journal`: every change since the project was opened.
    Journal,
    /// `<file>.journal.start`: the project as it was when the journal
    /// started, which recovery replays the journal onto.
    JournalStart,
    /// `<file>.journal.discarded-<YYYYMMDD-HHMMSS>`: unsaved work set aside
    /// by "Open last save", one file each time, named by when (UTC; the
    /// number is the stamp's digits). `None` is the single name earlier
    /// versions used, `<file>.journal.discarded`.
    DiscardedJournal(Option<u64>),
    /// `.<file>.<number>.saving`: a save that a crash or power cut
    /// interrupted (`None`: `.<file>.saving`, the name earlier versions used).
    UnfinishedSave(Option<u64>),
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
            SideFile::JournalStart => document::journal_start_path(project_file),
            SideFile::DiscardedJournal(stamp) => document::discarded_journal_path(project_file, stamp),
            SideFile::UnfinishedSave(number) => disk::temp_path(project_file, number),
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
            SideFile::JournalStart => "Autosave starting point",
            SideFile::DiscardedJournal(_) => "Unsaved work set aside after a crash",
            SideFile::UnfinishedSave(_) => "Unfinished save",
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
    let mut kinds = vec![SideFile::Journal, SideFile::JournalStart];
    let mut numbered = numbered(project_file);
    numbered.sort_unstable();
    numbered.dedup();
    kinds.extend(numbered);
    kinds.extend([SideFile::Thumbnail, SideFile::View, SideFile::Versions]);
    kinds.sort_unstable();
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

/// The side files whose names hold a number (set-aside journals,
/// unfinished saves, backups), found by listing the folder.
fn numbered(project_file: &Path) -> Vec<SideFile> {
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
    entries
        .flatten()
        .filter_map(|entry| numbered_kind(&entry.file_name().to_string_lossy(), &name))
        .collect()
}

/// What `candidate` is, when it is one of the numbered side files of the
/// project file called `project_name`, by exactly the name its writer gives.
fn numbered_kind(candidate: &str, project_name: &str) -> Option<SideFile> {
    if let Some(format) = backup_format(candidate, project_name) {
        return Some(SideFile::Backup(format));
    }
    if let Some(number) = disk::temp_number(candidate, project_name) {
        return Some(SideFile::UnfinishedSave(number));
    }
    document::discarded_stamp(candidate, project_name).map(SideFile::DiscardedJournal)
}

/// The format in `candidate` when it is `<project name>.backup-v<format>`
/// and nothing more. Windows file names ignore case, so the match does too
/// there.
fn backup_format(candidate: &str, project_name: &str) -> Option<u32> {
    let digits = disk::strip_name(candidate, project_name)?.strip_prefix(".backup-v")?;
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
        fs::write(document::journal_start_path(project_file), "").unwrap();
        fs::write(document::discarded_journal_path(project_file, None), "").unwrap();
        fs::write(
            document::discarded_journal_path(project_file, Some(20261113153000)),
            "",
        )
        .unwrap();
        fs::write(disk::temp_path(project_file, None), "half a save").unwrap();
        fs::write(disk::temp_path(project_file, Some(77)), "half a save").unwrap();
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
                SideFile::JournalStart,
                SideFile::DiscardedJournal(None),
                SideFile::DiscardedJournal(Some(20261113153000)),
                SideFile::UnfinishedSave(None),
                SideFile::UnfinishedSave(Some(77)),
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
        assert!(
            listed
                .iter()
                .any(|p| p.to_string_lossy().contains(".journal.discarded-")),
            "{listed:?}"
        );
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
            format!("{name}.journal.discarded-2026111-090502"),
            format!("{name}.journal.discarded-20261113-0905"),
            format!("{name}.journal.discarded-20261113-090502.txt"),
            format!("{name}.journal.discarded-"),
            format!(".{name}.x.saving"),
            format!(".{name}.007.saving"),
            format!(".{name} 2.saving"),
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
            SideFile::UnfinishedSave(None).path(&old),
            Path::new("a").join(".old.x.saving")
        );
        assert_eq!(
            SideFile::UnfinishedSave(Some(9)).path(&old),
            Path::new("a").join(".old.x.9.saving")
        );
        assert_eq!(
            SideFile::DiscardedJournal(None).path(&old),
            Path::new("a").join("old.x.journal.discarded")
        );
        assert_eq!(
            SideFile::DiscardedJournal(Some(20261113090502)).path(&old),
            Path::new("a").join("old.x.journal.discarded-20261113-090502")
        );
        assert_eq!(
            SideFile::JournalStart.path(&old),
            Path::new("a").join("old.x.journal.start")
        );
    }
}
