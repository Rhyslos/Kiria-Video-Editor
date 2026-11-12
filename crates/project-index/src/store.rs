//! The SQLite file behind the index.

use crate::entry::{Folder, FolderKind, Issue, ProjectEntry};
use crate::scan::find_project_files;
use diag::{AppError, Explain};
use project::{Rate, SequenceSettings, Time};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The table layout version. Raise it and add a step to [`upgrade`] when the tables change.
const SCHEMA_VERSION: i32 = 2;

const COLUMNS: &str = "path, name, width, height, rate_num, rate_den, sample_rate, \
                       length_num, length_den, last_opened, modified, issue, automatic";

/// The list of known projects and folders.
#[derive(Debug)]
pub struct Index {
    conn: Connection,
}

/// Seconds since 1970, the unit every time in the index uses.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn unreadable() -> AppError {
    AppError::new("The project list can't be read")
        .why("The launcher's list of projects could not be opened or updated.")
        .fix("Close other copies of the app and try again. Your projects themselves are not affected.")
}

impl Index {
    /// Opens the index file, creating it if needed. Several connections (the
    /// interface and a worker) can use the same file at once.
    pub fn open(path: &Path) -> diag::Result<Index> {
        let conn = Connection::open(path).or_explain(unreadable)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))
            .or_explain(unreadable)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .or_explain(unreadable)?;
        Index::set_up(conn)
    }

    /// An index that lives only in memory, for tests.
    pub fn open_in_memory() -> diag::Result<Index> {
        Index::set_up(Connection::open_in_memory().or_explain(unreadable)?)
    }

    fn set_up(conn: Connection) -> diag::Result<Index> {
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .or_explain(unreadable)?;
        if version > SCHEMA_VERSION {
            return Err(AppError::new("The project list was written by a newer version")
                .why("A newer version of the app changed the list of projects.")
                .fix("Update the app. Your projects themselves are not affected."));
        }
        upgrade(&conn, version).or_explain(unreadable)?;
        Ok(Index { conn })
    }

    /// Every folder in the tree, in the order they were added.
    pub fn folders(&self) -> diag::Result<Vec<Folder>> {
        let mut statement = self
            .conn
            .prepare("SELECT path, kind FROM folders ORDER BY added, path")
            .or_explain(unreadable)?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .or_explain(unreadable)?;
        let mut folders = Vec::new();
        for row in rows {
            let (path, kind) = row.or_explain(unreadable)?;
            if let Some(kind) = FolderKind::from_key(&kind) {
                folders.push(Folder {
                    path: path.into(),
                    kind,
                });
            }
        }
        Ok(folders)
    }

    /// Adds a folder to the tree, or moves it to another group if it is already there.
    pub fn add_folder(&self, path: &Path, kind: FolderKind) -> diag::Result<()> {
        self.conn
            .execute(
                "INSERT INTO folders (path, kind, added) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET kind = excluded.kind",
                params![path_text(path)?, kind.key(), now()],
            )
            .or_explain(unreadable)?;
        Ok(())
    }

    /// Makes sure projects saved in `path` are listed: adds it as a `kind`
    /// folder unless it already is one, or sits inside a folder that is
    /// searched for projects anyway. A media folder chosen for projects becomes `kind`.
    pub fn add_folder_if_new(&self, path: &Path, kind: FolderKind) -> diag::Result<()> {
        let folders = self.folders()?;
        let covered = folders.iter().any(|f| {
            f.kind.holds_projects()
                && path.starts_with(&f.path)
                && path.components().count() - f.path.components().count() <= crate::scan::MAX_DEPTH
        });
        if covered {
            return Ok(());
        }
        self.add_folder(path, kind)
    }

    /// Removes a folder from the tree. Nothing on disk is touched.
    pub fn remove_folder(&self, path: &Path) -> diag::Result<()> {
        self.conn
            .execute("DELETE FROM folders WHERE path = ?1", params![path_text(path)?])
            .or_explain(unreadable)?;
        Ok(())
    }

    /// Every listed project, most recently used first.
    pub fn projects(&self) -> diag::Result<Vec<ProjectEntry>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM projects WHERE hidden = 0
             ORDER BY COALESCE(last_opened, modified, 0) DESC, name COLLATE NOCASE"
        );
        let mut statement = self.conn.prepare(&sql).or_explain(unreadable)?;
        let rows = statement.query_map([], entry_from_row).or_explain(unreadable)?;
        rows.map(|r| r.or_explain(unreadable)).collect()
    }

    /// Every known project file, listed or removed from the list: what
    /// may still use a recording (the editor's cache cleanup asks).
    pub fn known_paths(&self) -> diag::Result<Vec<PathBuf>> {
        let mut statement = self
            .conn
            .prepare("SELECT path FROM projects ORDER BY path")
            .or_explain(unreadable)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .or_explain(unreadable)?;
        rows.map(|r| r.map(PathBuf::from).or_explain(unreadable))
            .collect()
    }

    /// One project, if it is known (listed or not).
    pub fn project(&self, path: &Path) -> diag::Result<Option<ProjectEntry>> {
        let sql = format!("SELECT {COLUMNS} FROM projects WHERE path = ?1");
        self.conn
            .query_row(&sql, params![path_text(path)?], entry_from_row)
            .optional()
            .or_explain(unreadable)
    }

    /// Reads the project file's summary and lists it (again, if it was removed
    /// from the list). Used after creating or opening a project. Reads only the
    /// start of the file.
    pub fn record(&self, path: &Path) -> diag::Result<ProjectEntry> {
        self.store(path, false)?;
        self.project(path)?.ok_or_else(unreadable)
    }

    /// Notes that the project was just opened.
    pub fn mark_opened(&self, path: &Path, when: i64) -> diag::Result<()> {
        self.conn
            .execute(
                "UPDATE projects SET last_opened = ?2, hidden = 0 WHERE path = ?1",
                params![path_text(path)?, when],
            )
            .or_explain(unreadable)?;
        Ok(())
    }

    /// Takes a project off the list. The file is not touched; opening it again lists it again.
    pub fn forget(&self, path: &Path) -> diag::Result<()> {
        self.conn
            .execute(
                "UPDATE projects SET hidden = 1 WHERE path = ?1",
                params![path_text(path)?],
            )
            .or_explain(unreadable)?;
        Ok(())
    }

    /// Drops a project whose files were deleted. Unlike [`Index::forget`],
    /// nothing is remembered: if the file comes back (restored from the
    /// recycle bin into a project folder), the next refresh lists it again.
    pub fn deleted(&self, path: &Path) -> diag::Result<()> {
        self.conn
            .execute("DELETE FROM projects WHERE path = ?1", params![path_text(path)?])
            .or_explain(unreadable)?;
        Ok(())
    }

    /// Follows a project file that was moved or renamed, keeping when it was last opened.
    pub fn moved(&self, from: &Path, to: &Path) -> diag::Result<ProjectEntry> {
        let (from, to_text) = (path_text(from)?, path_text(to)?);
        self.conn
            .execute("DELETE FROM projects WHERE path = ?1", params![to_text])
            .or_explain(unreadable)?;
        self.conn
            .execute(
                "UPDATE projects SET path = ?2 WHERE path = ?1",
                params![from, to_text],
            )
            .or_explain(unreadable)?;
        self.record(to)
    }

    /// Brings the list up to date with the disk: finds project files in the
    /// project and other folders, re-reads files that changed, and marks
    /// missing ones. Slow on big folders; run it on a worker thread.
    pub fn refresh(&self) -> diag::Result<()> {
        let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
        for folder in self.folders()? {
            if folder.kind.holds_projects() {
                paths.extend(find_project_files(&folder.path));
            }
        }
        let mut statement = self
            .conn
            .prepare("SELECT path, last_opened FROM projects")
            .or_explain(unreadable)?;
        let known = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .or_explain(unreadable)?;
        let mut listed_by_mistake = Vec::new();
        for row in known {
            let (path, last_opened) = row.or_explain(unreadable)?;
            let path = PathBuf::from(path);
            // Saved versions used to be found as projects of their own.
            // Those cards go, unless the person opened one on purpose.
            if last_opened.is_none() && crate::scan::is_saved_version(&path) {
                listed_by_mistake.push(path);
            } else {
                paths.insert(path);
            }
        }
        drop(statement);
        for path in listed_by_mistake {
            self.deleted(&path)?;
        }
        for path in paths {
            self.store(&path, true)?;
        }
        Ok(())
    }

    /// Reads one file and stores what the card needs. With `only_if_changed`,
    /// a file whose modification time matches the stored one is not read again,
    /// and a project taken off the list stays off it.
    fn store(&self, path: &Path, only_if_changed: bool) -> diag::Result<()> {
        let text = path_text(path)?;
        let stored: Option<(Option<i64>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT modified, issue FROM projects WHERE path = ?1",
                params![text],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .or_explain(unreadable)?;

        let modified = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64));
        let Some(modified) = modified else {
            // The file is gone. Keep the card so the person sees what happened.
            if stored.is_some() {
                self.conn
                    .execute(
                        "UPDATE projects SET issue = ?2 WHERE path = ?1",
                        params![text, Issue::Missing.key()],
                    )
                    .or_explain(unreadable)?;
                return Ok(());
            }
            if only_if_changed {
                // A refresh found it a moment ago and it has been deleted
                // since: there is nothing to list and nothing wrong.
                return Ok(());
            }
            return Err(AppError::new("The project file can't be found")
                .why(format!("{} does not exist.", path.display()))
                .fix("Check the file name, or look for the file in another folder."));
        };
        if only_if_changed
            && let Some((Some(old), issue)) = &stored
            && *old == modified
            && issue.as_deref() != Some(Issue::Missing.key())
        {
            return Ok(());
        }

        let fallback_name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let (name, settings, length, issue, automatic) = match project::file::read_info(path) {
            Ok(info) if info.format_version > project::file::FORMAT_VERSION => {
                (fallback_name, None, None, Some(Issue::NewerVersion), false)
            }
            Ok(info) => match info.summary {
                Some(s) => (s.name, Some(s.settings), Some(s.length), None, s.automatic),
                None => (fallback_name, None, None, None, false),
            },
            Err(err) => {
                tracing::info!(path = %path.display(), "{}", err.report());
                (fallback_name, None, None, Some(Issue::Damaged), false)
            }
        };
        let hidden_reset = if only_if_changed { "" } else { ", hidden = 0" };
        let sql = format!(
            "INSERT INTO projects ({COLUMNS}, hidden)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12, 0)
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name, width = excluded.width, height = excluded.height,
               rate_num = excluded.rate_num, rate_den = excluded.rate_den,
               sample_rate = excluded.sample_rate, length_num = excluded.length_num,
               length_den = excluded.length_den, modified = excluded.modified,
               issue = excluded.issue, automatic = excluded.automatic{hidden_reset}"
        );
        self.conn
            .execute(
                &sql,
                params![
                    text,
                    name,
                    settings.map(|s| s.width),
                    settings.map(|s| s.height),
                    settings.map(|s| s.frame_rate.num()),
                    settings.map(|s| s.frame_rate.den()),
                    settings.map(|s| s.sample_rate),
                    length.map(Time::num),
                    length.map(|l| l.den() as i64),
                    modified,
                    issue.map(Issue::key),
                    automatic,
                ],
            )
            .or_explain(unreadable)?;
        Ok(())
    }
}

/// Creates or upgrades the tables, one schema version at a time.
fn upgrade(conn: &Connection, from: i32) -> rusqlite::Result<()> {
    if from < 1 {
        conn.execute_batch(
            "CREATE TABLE folders (
                path  TEXT PRIMARY KEY,
                kind  TEXT NOT NULL,
                added INTEGER NOT NULL
             );
             CREATE TABLE projects (
                path        TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                width       INTEGER,
                height      INTEGER,
                rate_num    INTEGER,
                rate_den    INTEGER,
                sample_rate INTEGER,
                length_num  INTEGER,
                length_den  INTEGER,
                last_opened INTEGER,
                modified    INTEGER,
                issue       TEXT,
                hidden      INTEGER NOT NULL DEFAULT 0
             );",
        )?;
    }
    if from < 2 {
        // Projects whose settings wait for the first video.
        conn.execute_batch("ALTER TABLE projects ADD COLUMN automatic INTEGER NOT NULL DEFAULT 0;")?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectEntry> {
    let path: String = row.get(0)?;
    let width: Option<u32> = row.get(2)?;
    let height: Option<u32> = row.get(3)?;
    let rate_num: Option<u32> = row.get(4)?;
    let rate_den: Option<u32> = row.get(5)?;
    let sample_rate: Option<u32> = row.get(6)?;
    let length_num: Option<i64> = row.get(7)?;
    let length_den: Option<i64> = row.get(8)?;
    let issue: Option<String> = row.get(11)?;
    let automatic: bool = row.get(12)?;
    let settings = match (width, height, rate_num, rate_den, sample_rate) {
        (Some(width), Some(height), Some(num), Some(den), Some(sample_rate)) => {
            Rate::new(num, den).ok().map(|frame_rate| SequenceSettings {
                width,
                height,
                frame_rate,
                sample_rate,
            })
        }
        _ => None,
    };
    let length = match (length_num, length_den) {
        (Some(num), Some(den)) => u64::try_from(den).ok().and_then(|den| Time::from_ratio(num, den)),
        _ => None,
    };
    Ok(ProjectEntry {
        path: path.into(),
        name: row.get(1)?,
        settings,
        length,
        last_opened: row.get(9)?,
        modified: row.get(10)?,
        issue: issue.as_deref().and_then(Issue::from_key),
        automatic,
    })
}

/// Paths are stored as text; the rare path that isn't valid Unicode can't be listed.
fn path_text(path: &Path) -> diag::Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        AppError::new("This file name can't be listed")
            .why(format!(
                "{} contains characters the project list can't store.",
                path.display()
            ))
            .fix("Rename the file or folder using ordinary letters and numbers.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Project, file};

    fn write_project(path: &Path, name: &str, settings: SequenceSettings) {
        file::save(&Project::new(name, settings), path).unwrap();
    }

    fn ext() -> &'static str {
        project::FILE_EXTENSION
    }

    #[test]
    fn every_known_project_is_named_also_those_removed_from_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        let (a, b) = (
            dir.path().join(format!("a.{}", ext())),
            dir.path().join(format!("b.{}", ext())),
        );
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        write_project(&b, "B", SequenceSettings::GAME_CLIP);
        index.record(&a).unwrap();
        index.record(&b).unwrap();
        index.forget(&b).unwrap();
        assert_eq!(index.projects().unwrap().len(), 1, "b is removed from the list");
        assert_eq!(index.known_paths().unwrap(), [a, b], "but still known");
    }

    #[test]
    fn folders_are_remembered_in_their_groups() {
        let index = Index::open_in_memory().unwrap();
        index.add_folder(Path::new("/p"), FolderKind::Projects).unwrap();
        index.add_folder(Path::new("/m"), FolderKind::Media).unwrap();
        index.add_folder(Path::new("/m"), FolderKind::Other).unwrap();
        let folders = index.folders().unwrap();
        assert_eq!(folders.len(), 2);
        let m = folders.iter().find(|f| f.path == Path::new("/m")).unwrap();
        assert_eq!(m.kind, FolderKind::Other, "adding again moves it");
        index.remove_folder(Path::new("/p")).unwrap();
        assert_eq!(index.folders().unwrap().len(), 1);
    }

    #[test]
    fn folders_inside_listed_folders_are_not_added_twice() {
        let index = Index::open_in_memory().unwrap();
        index.add_folder(Path::new("/p"), FolderKind::Projects).unwrap();
        index.add_folder(Path::new("/m"), FolderKind::Media).unwrap();
        index
            .add_folder_if_new(Path::new("/p/2026"), FolderKind::Projects)
            .unwrap();
        index
            .add_folder_if_new(Path::new("/p"), FolderKind::Projects)
            .unwrap();
        assert_eq!(index.folders().unwrap().len(), 2, "already searched");
        index
            .add_folder_if_new(Path::new("/m"), FolderKind::Projects)
            .unwrap();
        index
            .add_folder_if_new(Path::new("/elsewhere"), FolderKind::Projects)
            .unwrap();
        let folders = index.folders().unwrap();
        assert_eq!(folders.len(), 3);
        assert!(
            folders.iter().all(|f| f.kind == FolderKind::Projects),
            "the media folder now holds projects"
        );
    }

    #[test]
    fn recording_a_project_reads_its_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("trip.{}", ext()));
        write_project(&path, "Lofoten trip", SequenceSettings::HIGH_FRAME_RATE);
        let index = Index::open_in_memory().unwrap();
        let entry = index.record(&path).unwrap();
        assert_eq!(entry.name, "Lofoten trip");
        assert_eq!(entry.settings, Some(SequenceSettings::HIGH_FRAME_RATE));
        assert_eq!(entry.length, Some(Time::ZERO));
        assert_eq!(entry.issue, None);
        assert!(entry.modified.is_some());
    }

    #[test]
    fn automatic_projects_say_so() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("auto.{}", ext()));
        file::save(&Project::new_automatic("New"), &path).unwrap();
        let index = Index::open_in_memory().unwrap();
        let entry = index.record(&path).unwrap();
        assert!(entry.automatic);
        assert_eq!(entry.settings_label().as_deref(), Some("Automatic"));
        let fixed = tmp.path().join(format!("fixed.{}", ext()));
        write_project(&fixed, "Fixed", SequenceSettings::GAME_CLIP);
        assert_eq!(
            index.record(&fixed).unwrap().settings_label().as_deref(),
            Some("1080p 60")
        );
    }

    #[test]
    fn a_list_from_the_first_layout_is_upgraded() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("index.sqlite");
        {
            let conn = Connection::open(&db).unwrap();
            upgrade_to_1(&conn);
            conn.execute("INSERT INTO projects (path, name) VALUES ('/p/old.x', 'Old')", [])
                .unwrap();
        }
        let index = Index::open(&db).unwrap();
        let old = index.projects().unwrap();
        assert_eq!(old.len(), 1);
        assert!(!old[0].automatic, "existing projects keep their fixed settings");
    }

    /// The tables as schema version 1 made them.
    fn upgrade_to_1(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE folders (path TEXT PRIMARY KEY, kind TEXT NOT NULL, added INTEGER NOT NULL);
             CREATE TABLE projects (path TEXT PRIMARY KEY, name TEXT NOT NULL, width INTEGER, height INTEGER,
                rate_num INTEGER, rate_den INTEGER, sample_rate INTEGER, length_num INTEGER, length_den INTEGER,
                last_opened INTEGER, modified INTEGER, issue TEXT, hidden INTEGER NOT NULL DEFAULT 0);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    #[test]
    fn refresh_finds_new_projects_and_notices_missing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        index.add_folder(tmp.path(), FolderKind::Projects).unwrap();
        let a = tmp.path().join(format!("a.{}", ext()));
        let b = tmp.path().join(format!("b.{}", ext()));
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        write_project(&b, "B", SequenceSettings::GAME_CLIP);
        index.refresh().unwrap();
        assert_eq!(index.projects().unwrap().len(), 2);

        std::fs::remove_file(&b).unwrap();
        index.refresh().unwrap();
        let b_entry = index.project(&b).unwrap().unwrap();
        assert_eq!(b_entry.issue, Some(Issue::Missing));
        assert_eq!(b_entry.name, "B", "the card keeps its name");

        write_project(&b, "B again", SequenceSettings::GAME_CLIP);
        index.refresh().unwrap();
        assert_eq!(index.project(&b).unwrap().unwrap().issue, None);
    }

    #[test]
    fn media_folders_are_not_searched_for_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        index.add_folder(tmp.path(), FolderKind::Media).unwrap();
        write_project(
            &tmp.path().join(format!("a.{}", ext())),
            "A",
            SequenceSettings::GAME_CLIP,
        );
        index.refresh().unwrap();
        assert!(index.projects().unwrap().is_empty());
    }

    #[test]
    fn newer_and_damaged_files_get_issues() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        let newer = tmp.path().join(format!("newer.{}", ext()));
        let text = file::to_string(&Project::new("N", SequenceSettings::GAME_CLIP))
            .unwrap()
            .replacen(
                &format!("format_version: {}", file::FORMAT_VERSION),
                &format!("format_version: {}", file::FORMAT_VERSION + 1),
                1,
            );
        std::fs::write(&newer, text).unwrap();
        let damaged = tmp.path().join(format!("damaged.{}", ext()));
        std::fs::write(&damaged, "not a project").unwrap();
        assert_eq!(index.record(&newer).unwrap().issue, Some(Issue::NewerVersion));
        let entry = index.record(&damaged).unwrap();
        assert_eq!(entry.issue, Some(Issue::Damaged));
        assert_eq!(entry.name, "damaged", "named after the file");
    }

    #[test]
    fn forgotten_projects_stay_off_the_list_until_opened_again() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        index.add_folder(tmp.path(), FolderKind::Projects).unwrap();
        let a = tmp.path().join(format!("a.{}", ext()));
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        index.refresh().unwrap();
        index.forget(&a).unwrap();
        index.refresh().unwrap();
        assert!(index.projects().unwrap().is_empty());
        index.record(&a).unwrap();
        assert_eq!(index.projects().unwrap().len(), 1);
    }

    #[test]
    fn saved_versions_are_not_listed_as_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        index.add_folder(tmp.path(), FolderKind::Projects).unwrap();
        let a = tmp.path().join(format!("a.{}", ext()));
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        let project = project::file::load(&a).unwrap();
        let first = project::versions::save(&project, &a, "First cut").unwrap();
        let second = project::versions::save(&project, &a, "Second cut").unwrap();
        index.refresh().unwrap();
        assert_eq!(index.projects().unwrap().len(), 1, "only the project itself");

        // A list made before versions were left out has them: they go,
        // except one the person opened on purpose.
        index.record(&first.path).unwrap();
        index.record(&second.path).unwrap();
        index.mark_opened(&second.path, now()).unwrap();
        index.refresh().unwrap();
        let mut paths: Vec<PathBuf> = index.projects().unwrap().into_iter().map(|p| p.path).collect();
        paths.sort();
        let mut expected = vec![a, second.path];
        expected.sort();
        assert_eq!(paths, expected);
    }

    #[test]
    fn a_deleted_project_leaves_the_list_and_returns_if_its_file_does() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        index.add_folder(tmp.path(), FolderKind::Projects).unwrap();
        let a = tmp.path().join(format!("a.{}", ext()));
        let kept = tmp.path().join(format!("kept.{}", ext()));
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        write_project(&kept, "Kept", SequenceSettings::GAME_CLIP);
        index.refresh().unwrap();

        std::fs::remove_file(&a).unwrap();
        index.deleted(&a).unwrap();
        index.refresh().unwrap();
        assert_eq!(index.project(&a).unwrap(), None, "no card saying it is missing");
        let names: Vec<String> = index.projects().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Kept"]);
        index.deleted(&a).unwrap(); // deleting what is not listed is fine
        // A refresh that still had the file on its list is not upset either.
        index.store(&a, true).unwrap();
        assert!(
            index.record(&a).is_err(),
            "opening a file that is gone still says so"
        );

        // Restored from the recycle bin: found again like any project file.
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        index.refresh().unwrap();
        assert_eq!(index.projects().unwrap().len(), 2);
    }

    #[test]
    fn most_recently_opened_comes_first_and_moves_keep_it() {
        let tmp = tempfile::tempdir().unwrap();
        let index = Index::open_in_memory().unwrap();
        let a = tmp.path().join(format!("a.{}", ext()));
        let b = tmp.path().join(format!("b.{}", ext()));
        write_project(&a, "A", SequenceSettings::GAME_CLIP);
        write_project(&b, "B", SequenceSettings::GAME_CLIP);
        index.record(&a).unwrap();
        index.record(&b).unwrap();
        let opened = now() + 100;
        index.mark_opened(&a, opened).unwrap();
        assert_eq!(index.projects().unwrap()[0].name, "A");

        let moved = tmp.path().join(format!("renamed.{}", ext()));
        std::fs::rename(&a, &moved).unwrap();
        let entry = index.moved(&a, &moved).unwrap();
        assert_eq!(entry.last_opened, Some(opened));
        assert!(index.project(&a).unwrap().is_none());
        assert_eq!(index.projects().unwrap()[0].path, moved);
    }

    #[test]
    fn two_connections_share_one_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(crate::INDEX_FILE_NAME);
        let interface = Index::open(&file).unwrap();
        let worker = Index::open(&file).unwrap();
        worker.add_folder(Path::new("/p"), FolderKind::Projects).unwrap();
        assert_eq!(interface.folders().unwrap().len(), 1);
    }

    #[test]
    fn two_hundred_projects_list_quickly() {
        let index = Index::open_in_memory().unwrap();
        for i in 0..200 {
            index
                .conn
                .execute(
                    "INSERT INTO projects (path, name, width, height, rate_num, rate_den, sample_rate, length_num, length_den, modified)
                     VALUES (?1, ?2, 1920, 1080, 60, 1, 48000, 600, 1, ?3)",
                    params![format!("/p/{i}.x"), format!("Project {i}"), i],
                )
                .unwrap();
        }
        let started = std::time::Instant::now();
        let projects = index.projects().unwrap();
        assert_eq!(projects.len(), 200);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
    }
}
