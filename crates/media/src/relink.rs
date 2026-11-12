//! Finding moved or renamed media again.
//!
//! A file is recognised by its fingerprint (size plus a hash of both ends),
//! so it is found under any name. Only files of the right size are hashed,
//! which keeps a search through a big folder quick.
//!
//! Two searches use this: one in a folder the person picks ("Find them…",
//! [`find`]), and the automatic one when a project opens with files
//! missing, through the folders the app already knows ([`find_in_known`]),
//! which is kept short: not too deep, not too many files, not too long.

use crate::fingerprint;
use project::{Fingerprint, MediaId, Project};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How far a search may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Folders deeper than this below a starting folder are not opened.
    pub depth: usize,
    /// Stop after looking at this many entries in all.
    pub entries: usize,
    /// Stop after this long.
    pub time: Duration,
}

impl Limits {
    /// A folder the person picked: as deep as it goes, up to 200 000
    /// entries (so a search of a whole drive ends), for as long as it takes.
    pub const PICKED: Limits = Limits {
        depth: usize::MAX,
        entries: 200_000,
        time: Duration::MAX,
    };

    /// The automatic search when a project opens: eight folders deep,
    /// 100 000 entries, 20 seconds, so it never runs on for long in the
    /// background.
    pub const AUTOMATIC: Limits = Limits {
        depth: 8,
        entries: 100_000,
        time: Duration::from_secs(20),
    };
}

/// The project's media files that are not where the project says.
/// Touches the disk: run it on a worker.
pub fn missing(project: &Project) -> Vec<MediaId> {
    let mut ids: Vec<MediaId> = project
        .media
        .values()
        .filter(|m| !m.path.exists())
        .map(|m| m.id)
        .collect();
    ids.sort();
    ids
}

/// Looks through `folder` and everything under it for the `wanted` files.
/// Returns each one found with where it is now. `cancelled` is asked now
/// and then; when it says yes, what was found so far comes back.
pub fn find(
    folder: &Path,
    wanted: &[(MediaId, Fingerprint)],
    cancelled: impl Fn() -> bool,
) -> Vec<(MediaId, PathBuf)> {
    search(&[folder.to_path_buf()], wanted, Limits::PICKED, cancelled)
}

/// The automatic search: through `folders` (the project's own folder,
/// the launcher's media and project folders), within [`Limits::AUTOMATIC`].
/// A folder inside another one on the list is looked through once.
pub fn find_in_known(
    folders: &[PathBuf],
    wanted: &[(MediaId, Fingerprint)],
    cancelled: impl Fn() -> bool,
) -> Vec<(MediaId, PathBuf)> {
    search(&outermost(folders), wanted, Limits::AUTOMATIC, cancelled)
}

/// `folders` without the ones inside another on the list, nor repeats, in
/// their order.
pub fn outermost(folders: &[PathBuf]) -> Vec<PathBuf> {
    let full: Vec<PathBuf> = folders
        .iter()
        .map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()))
        .collect();
    let mut kept: Vec<PathBuf> = Vec::new();
    for (i, folder) in full.iter().enumerate() {
        let inside_another = full
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && folder.starts_with(other) && (folder != other || j < i));
        if !inside_another {
            kept.push(folder.clone());
        }
    }
    kept
}

/// Walks `folders` (each with everything under it, hidden folders left
/// out), hashing only files whose size is one of the wanted ones.
pub fn search(
    folders: &[PathBuf],
    wanted: &[(MediaId, Fingerprint)],
    limits: Limits,
    cancelled: impl Fn() -> bool,
) -> Vec<(MediaId, PathBuf)> {
    let started = Instant::now();
    let sizes: HashSet<u64> = wanted.iter().map(|(_, f)| f.size).collect();
    let mut found: Vec<(MediaId, PathBuf)> = Vec::new();
    let mut seen = 0usize;
    // Depth first, each starting folder in turn, so the first on the list
    // (the project's own folder) is looked through first.
    let mut stack: Vec<(PathBuf, usize)> = folders.iter().rev().map(|f| (f.clone(), 0)).collect();
    let over = |seen: usize| seen >= limits.entries || started.elapsed() >= limits.time;
    while let Some((dir, depth)) = stack.pop() {
        if found.len() == wanted.len() || over(seen) || cancelled() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // unreadable folders are skipped
        };
        for entry in entries.flatten() {
            seen += 1;
            if over(seen) {
                break;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < limits.depth && !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let Ok(size) = entry.metadata().map(|m| m.len()) else {
                continue;
            };
            if !sizes.contains(&size) {
                continue;
            }
            let candidates: Vec<&(MediaId, Fingerprint)> = wanted
                .iter()
                .filter(|(id, f)| f.size == size && !found.iter().any(|(g, _)| g == id))
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let Ok(print) = fingerprint(&path) else { continue };
            if let Some((id, _)) = candidates.iter().find(|(_, f)| *f == print) {
                found.push((*id, path));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_moved_and_renamed_files_in_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mkv");
        let b = dir.path().join("b.mkv");
        std::fs::write(&a, b"first recording").unwrap();
        std::fs::write(&b, b"second recording!").unwrap();
        let wanted = vec![
            (MediaId(1), fingerprint(&a).unwrap()),
            (MediaId(2), fingerprint(&b).unwrap()),
        ];

        let elsewhere = dir.path().join("new place/deeper");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::rename(&a, elsewhere.join("renamed.mkv")).unwrap();
        // Same size, different contents: not a match.
        std::fs::write(dir.path().join("new place/decoy.mkv"), b"second recordinX!").unwrap();
        std::fs::remove_file(&b).unwrap();

        let found = find(&dir.path().join("new place"), &wanted, || false);
        assert_eq!(found, vec![(MediaId(1), elsewhere.join("renamed.mkv"))]);
        assert!(find(dir.path(), &wanted, || true).is_empty(), "cancelled at once");
    }

    #[test]
    fn the_automatic_search_looks_through_every_known_folder_and_stops_at_its_depth() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, media) = (dir.path().join("Projects"), dir.path().join("Recordings"));
        let a = dir.path().join("a.mkv");
        let b = dir.path().join("b.mkv");
        std::fs::write(&a, b"one recording").unwrap();
        std::fs::write(&b, b"another recording").unwrap();
        let wanted = vec![
            (MediaId(1), fingerprint(&a).unwrap()),
            (MediaId(2), fingerprint(&b).unwrap()),
        ];
        std::fs::create_dir_all(projects.join("Trip")).unwrap();
        std::fs::rename(&a, projects.join("Trip/a.mkv")).unwrap();
        let deep = media.join("1/2/3/4/5/6/7/8/9");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::rename(&b, deep.join("b.mkv")).unwrap();

        let known = [projects.clone(), media.clone()];
        let found = find_in_known(&known, &wanted, || false);
        assert_eq!(
            found,
            vec![(MediaId(1), projects.join("Trip/a.mkv"))],
            "b is nine folders deep"
        );

        let shallow = media.join("b.mkv");
        std::fs::rename(deep.join("b.mkv"), &shallow).unwrap();
        let found = find_in_known(&known, &wanted, || false);
        assert_eq!(found.len(), 2);
        assert!(found.contains(&(MediaId(2), shallow)));
    }

    #[test]
    fn only_files_of_a_wanted_size_are_read_and_the_limits_end_a_search() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.mkv");
        std::fs::write(&file, b"twelve bytes").unwrap();
        let wanted = vec![(MediaId(1), fingerprint(&file).unwrap())];
        let few = Limits {
            depth: 8,
            entries: 0,
            time: Duration::from_secs(20),
        };
        assert!(search(&[dir.path().to_path_buf()], &wanted, few, || false).is_empty());
        let no_time = Limits {
            time: Duration::ZERO,
            ..Limits::AUTOMATIC
        };
        assert!(search(&[dir.path().to_path_buf()], &wanted, no_time, || false).is_empty());
        assert_eq!(
            search(&[dir.path().to_path_buf()], &wanted, Limits::AUTOMATIC, || false).len(),
            1
        );
    }

    #[test]
    fn folders_inside_others_are_looked_through_once() {
        let dir = tempfile::tempdir().unwrap();
        let (a, c) = (dir.path().join("a"), dir.path().join("c"));
        let folders = [a.join("b"), a.clone(), c.clone(), a.clone()];
        assert_eq!(outermost(&folders), [a, c]);
    }
}
