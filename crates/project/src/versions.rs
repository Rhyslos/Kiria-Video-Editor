//! Saved versions: named copies of a project kept beside it, in
//! `<file>.versions/`, to go back to later. Restoring one is an ordinary,
//! undoable change ([`crate::Command::Restore`]), so nothing is lost.

use crate::{FILE_EXTENSION, Project, file};
use diag::{AppError, Explain};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One saved version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub name: String,
    /// Seconds since 1970 when it was saved.
    pub saved: i64,
    /// Milliseconds since 1970: orders versions saved in the same second.
    stamp: i64,
    pub path: PathBuf,
}

/// The folder holding a project's versions.
pub fn folder(project_file: &Path) -> PathBuf {
    let mut name = project_file.as_os_str().to_owned();
    name.push(".versions");
    PathBuf::from(name)
}

/// Saves `project` as a version called `name`.
pub fn save(project: &Project, project_file: &Path, name: &str) -> diag::Result<Version> {
    let name = clean_name(name);
    if name.is_empty() {
        return Err(
            AppError::new("A version needs a name").fix("Type a few words, like \"Before the colour pass\".")
        );
    }
    let dir = folder(project_file);
    std::fs::create_dir_all(&dir).or_explain(|| {
        AppError::new("The version can't be saved")
            .why(format!("{} could not be made.", dir.display()))
            .fix("Check that the project folder can be written to.")
    })?;
    // Named by the millisecond, later than any version already there, so
    // the order is kept even for versions saved in the same moment.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let stamp = list(project_file).first().map_or(now, |v| now.max(v.stamp + 1));
    let path = dir.join(format!("{stamp} {name}.{FILE_EXTENSION}"));
    file::save(project, &path)?;
    Ok(Version {
        name,
        saved: stamp / 1000,
        stamp,
        path,
    })
}

/// The project's versions, newest first.
pub fn list(project_file: &Path) -> Vec<Version> {
    let Ok(entries) = std::fs::read_dir(folder(project_file)) else {
        return Vec::new();
    };
    let mut versions: Vec<Version> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some(FILE_EXTENSION) {
                return None;
            }
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            let (ms, name) = stem.split_once(' ')?;
            let stamp: i64 = ms.parse().ok()?;
            Some(Version {
                name: name.to_owned(),
                saved: stamp / 1000,
                stamp,
                path,
            })
        })
        .collect();
    versions.sort_by_key(|v| std::cmp::Reverse(v.stamp));
    versions
}

/// Reads a version (older formats are upgraded as they are read).
pub fn load(version: &Version) -> diag::Result<Project> {
    file::load(&version.path)
}

/// A name that is safe in a file name: no path or reserved characters,
/// trimmed, at most 60 characters.
fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .take(60)
        .collect();
    cleaned.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("p.{FILE_EXTENSION}"));
        assert!(list(&file).is_empty());
        let mut p = Project::new_automatic("Trip");
        save(&p, &file, "First cut").unwrap();
        p.name = "Trip v2".into();
        let v = save(&p, &file, "Before colour: a/b").unwrap();
        assert_eq!(v.name, "Before colour- a-b");
        let all = list(&file);
        assert_eq!(all.len(), 2);
        assert_eq!(load(&all[0]).unwrap().name, "Trip v2", "newest first");
        assert!(save(&p, &file, "  ").is_err());
    }
}
