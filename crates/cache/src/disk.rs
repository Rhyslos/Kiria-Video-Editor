//! What the stores kept on disk share: a file's last use is its time of
//! change ([`touch`]), and a folder is kept within a budget by removing the
//! files used longest ago first ([`trim`]).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Marks a kept file as just used: its time of change becomes now, which
/// is what [`trim`] keeps the newest of. Failing only makes it look older.
pub(crate) fn touch(path: &Path) {
    let _ = std::fs::File::options()
        .append(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// Removes the files in `folder` with one of `extensions` that were used
/// longest ago, until those left take at most `budget` bytes (a budget of
/// 0 removes them all). Other files in the folder are never touched.
/// Returns how many went.
pub(crate) fn trim(folder: &Path, extensions: &[&str], budget: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| extensions.iter().any(|ours| x == *ours))
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut kept = 0u64;
    let mut removed = 0;
    for (_, len, path) in files {
        kept += len;
        if kept > budget && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Writes `bytes` bytes to `name`, last used `age_s` seconds ago.
    fn kept(folder: &Path, name: &str, bytes: usize, age_s: u64) -> PathBuf {
        let path = folder.join(name);
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_s);
        std::fs::File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }

    #[test]
    fn the_files_used_longest_ago_go_first_until_the_rest_fit() {
        let dir = tempfile::tempdir().unwrap();
        let oldest = kept(dir.path(), "a.peaks", 100, 300);
        let older = kept(dir.path(), "b.peaks", 100, 200);
        let newer = kept(dir.path(), "c.peaks", 100, 100);
        let newest = kept(dir.path(), "d.peaks", 100, 0);

        assert_eq!(trim(dir.path(), &["peaks"], 250), 2);
        assert!(!oldest.exists() && !older.exists());
        assert!(newer.exists() && newest.exists());

        // Within the budget: nothing goes.
        assert_eq!(trim(dir.path(), &["peaks"], 200), 0);
    }

    #[test]
    fn touching_a_file_keeps_it_over_newer_ones() {
        let dir = tempfile::tempdir().unwrap();
        let used_again = kept(dir.path(), "a.peaks", 100, 300);
        let other = kept(dir.path(), "b.peaks", 100, 100);
        touch(&used_again);

        assert_eq!(trim(dir.path(), &["peaks"], 100), 1);
        assert!(used_again.exists());
        assert!(!other.exists());
    }

    #[test]
    fn only_files_with_the_extensions_named_are_counted_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let strip = kept(dir.path(), "a.pictures", 100, 200);
        let cover = kept(dir.path(), "a.cover", 100, 100);
        let stranger = kept(dir.path(), "notes.txt", 1000, 300);
        std::fs::create_dir(dir.path().join("folder.pictures")).unwrap();

        assert_eq!(trim(dir.path(), &["pictures", "cover"], 0), 2);
        assert!(!strip.exists() && !cover.exists());
        assert!(stranger.exists());
        assert!(dir.path().join("folder.pictures").is_dir());
    }

    #[test]
    fn a_missing_folder_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(trim(&dir.path().join("none"), &["peaks"], 0), 0);
    }
}
