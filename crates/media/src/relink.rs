//! Finding moved or renamed media again.
//!
//! A file is recognised by its fingerprint (size plus a hash of both ends),
//! so it is found under any name. Only files of the right size are hashed,
//! which keeps a search through a big folder quick.

use crate::fingerprint;
use project::{Fingerprint, MediaId, Project};
use std::path::{Path, PathBuf};

/// Stop after looking at this many files, so a search of a whole drive ends.
const MOST_FILES: usize = 200_000;

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
    let mut found: Vec<(MediaId, PathBuf)> = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = folders.pop() {
        if found.len() == wanted.len() || seen >= MOST_FILES || cancelled() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // unreadable folders are skipped
        };
        for entry in entries.flatten() {
            seen += 1;
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    folders.push(path);
                }
                continue;
            }
            let Ok(size) = entry.metadata().map(|m| m.len()) else {
                continue;
            };
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
}
