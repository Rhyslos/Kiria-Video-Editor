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
const SCHEMA_VERSION: i32 = 1;

const COLUMNS: &str = "path, name, width, height, rate_num, rate_den, sample_rate, \
                       length_num, length_den, last_opened, modified, issue";

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
            .prepare("SELECT path FROM projects")
            .or_explain(unreadable)?;
        let known = statement
            .query_map([], |row| row.get::<_, String>(0))
            .or_explain(unreadable)?;
        for path in known {
            paths.insert(path.or_explain(unreadable)?.into());
        }
        drop(statement);
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
        let (name, settings, length, issue) = match project::file::read_info(path) {
            Ok(info) if info.format_version > project::file::FORMAT_VERSION => {
                (fallback_name, None, None, Some(Issue::NewerVersion))
            }
            Ok(info) => match info.summary {
                Some(s) => (s.name, Some(s.settings), Some(s.length), None),
                None => (fallback_name, None, None, None),
            },
            Err(err) => {
                tracing::info!(path = %path.display(), "{}", err.report());
                (fallback_name, None, None, Some(Issue::Damaged))
            }
        };
        let hidden_reset = if only_if_changed { "" } else { ", hidden = 0" };
        let sql = format!(
            "INSERT INTO projects ({COLUMNS}, hidden)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, 0)
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name, width = excluded.width, height = excluded.height,
               rate_num = excluded.rate_num, rate_den = excluded.rate_den,
               sample_rate = excluded.sample_rate, length_num = excluded.length_num,
               length_den = excluded.length_den, modified = excluded.modified,
               issue = excluded.issue{hidden_reset}"
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
