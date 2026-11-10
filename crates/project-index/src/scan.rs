//! Finding project files in folders.

use std::path::{Path, PathBuf};

/// How many folder levels below a project folder are searched. Enough for
/// "Projects/2026/Trip/trip.kiria" without wandering through a whole drive.
const MAX_DEPTH: usize = 3;

/// Every project file in `folder` and up to three levels of subfolders.
/// Hidden folders (starting with a dot) are skipped. Unreadable folders are
/// skipped quietly; the folder itself being gone gives an empty list.
pub fn find_project_files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(folder, 0, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !hidden && depth < MAX_DEPTH {
                walk(&path, depth + 1, found);
            }
        } else if !hidden && is_project_file(&path) {
            found.push(path);
        }
    }
}

/// Whether the path has the project file extension.
pub(crate) fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(project::FILE_EXTENSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_project_files_a_few_levels_down_but_not_hidden_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let ext = project::FILE_EXTENSION;
        let deep = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b/c/d")).unwrap();
        for file in [
            format!("top.{ext}"),
            format!("a/b/c/deep.{ext}"),
            format!("a/b/c/d/too-deep.{ext}"),
            format!(".hidden/secret.{ext}"),
            format!(".journal.{ext}"),
            "a/notes.txt".to_owned(),
        ] {
            std::fs::write(tmp.path().join(file), "").unwrap();
        }
        let names: Vec<String> = find_project_files(tmp.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [format!("deep.{ext}"), format!("top.{ext}")]);
    }

    #[test]
    fn a_missing_folder_has_no_projects() {
        assert!(find_project_files(Path::new("/no/such/folder/anywhere")).is_empty());
    }
}
