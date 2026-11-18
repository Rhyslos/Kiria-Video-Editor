//! Writing small files whole or not at all: the one way every program and
//! crate does it (project files, settings, presets, labels, templates, the
//! updater's state files).
//!
//! - [`write_atomic`]: the whole text, or the old file untouched. The text
//!   goes to a temporary file of its own beside the target (a new name for
//!   every write, so two writers never share one), is put on the disk, and
//!   is then renamed over the old file.
//! - [`remove_old_temps`]: a crash or power cut in the middle of a write
//!   leaves its temporary file behind; those older than
//!   [`TEMP_LEFT_FOR`] are removed when found (at every start for the data
//!   folder), long after any write still under way would have finished.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// How old a leftover temporary file must be before it is removed: a day,
/// far longer than any write of a small file takes.
pub const TEMP_LEFT_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// Writes `bytes` to `path` so that a crash never leaves half a file: to a
/// temporary file of this write's own beside it, put on the disk, then
/// renamed over the old one. On a failure the temporary file is taken away
/// again and the old file is as it was.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temp, mut file) = create_temp(path)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::rename(&temp, path)
        });
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written?;
    sync_folder(path);
    Ok(())
}

/// Asks the disk to keep the rename too (the folder's entry). Only Unix
/// can open a folder for this; Windows keeps the rename with the file.
fn sync_folder(path: &Path) {
    #[cfg(unix)]
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(folder) = File::open(folder)
    {
        let _ = folder.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Numbers temporary files within this program; with the process id in
/// front, no two writes anywhere share one.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// A new temporary file beside `path`, made with a name nobody else has
/// (`.name.<number>.saving`, see [`temp_path`]).
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    let process = u64::from(std::process::id()) << 32;
    loop {
        let number = process | (NEXT_TEMP.fetch_add(1, Ordering::Relaxed) & 0xFFFF_FFFF);
        let temp = temp_path(path, Some(number));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            // Left by an earlier run with the same process id: next number.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
}

/// A temporary file of a write to `path`: `.name.<number>.saving` in the
/// same folder, so the rename stays on one disk. `None` is the single name
/// earlier versions used (`.name.saving`), still listed so a leftover one
/// is recognised.
pub fn temp_path(path: &Path, number: Option<u64>) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match number {
        Some(number) => path.with_file_name(format!(".{name}.{number}.saving")),
        None => path.with_file_name(format!(".{name}.saving")),
    }
}

/// The number in `candidate` when it is a temporary file of a write to the
/// file called `name` (see [`temp_path`]): `Some(None)` for the old single
/// name.
pub fn temp_number(candidate: &str, name: &str) -> Option<Option<u64>> {
    let rest = candidate.strip_prefix('.')?;
    let rest = strip_name(rest, name)?.strip_suffix(".saving")?;
    if rest.is_empty() {
        return Some(None);
    }
    let digits = rest.strip_prefix('.')?;
    let number: u64 = digits.parse().ok()?;
    (number.to_string() == digits).then_some(Some(number))
}

/// `candidate` without `name` in front. Windows file names ignore case, so
/// the match does too there.
pub fn strip_name<'a>(candidate: &'a str, name: &str) -> Option<&'a str> {
    let split = name.len();
    if !candidate.is_char_boundary(split) {
        return None;
    }
    let (start, rest) = candidate.split_at(split);
    let same = if cfg!(windows) {
        start.eq_ignore_ascii_case(name)
    } else {
        start == name
    };
    same.then_some(rest)
}

/// Whether `name` is the name of a temporary file of some write
/// (`.<anything>.saving`, see [`temp_path`]).
fn is_temp_name(name: &str) -> bool {
    name.len() >= ".x.saving".len() && name.starts_with('.') && name.ends_with(".saving")
}

/// Removes the temporary files of writes ([`temp_path`]) directly in
/// `folder` that were last changed more than `older_than` ago: what
/// crashes and power cuts left behind. Younger ones are left alone, since
/// another program may be writing them right now. Returns how many went;
/// a file that can't be removed is logged and passed over.
pub fn remove_old_temps(folder: &Path, older_than: Duration) -> usize {
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        if !is_temp_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|changed| now.duration_since(changed).ok())
            .is_some_and(|age| age > older_than);
        if !meta.is_file() || !old {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {
                tracing::info!(file = %entry.path().display(), "removed a write left unfinished");
                removed += 1;
            }
            Err(err) => {
                tracing::warn!(file = %entry.path().display(), "a write left unfinished can't be removed ({err})");
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_replaces_the_file_whole_and_leaves_nothing_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.ron");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(names_in(tmp.path()), ["settings.ron"]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("labels.ron");
        write_atomic(&path, b"old").unwrap();
        // A folder where the file should go: the rename fails.
        let blocked = tmp.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("inside"), "x").unwrap();
        assert!(write_atomic(&blocked, b"new").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(names_in(tmp.path()), ["blocked", "labels.ron"]);
    }

    #[test]
    fn writers_at_the_same_moment_never_share_a_temporary_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let texts: Vec<String> = (0..8).map(|i| format!("{i}").repeat(200_000)).collect();
        std::thread::scope(|scope| {
            for text in &texts {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..5 {
                        write_atomic(path, text.as_bytes()).unwrap();
                    }
                });
            }
        });
        let text = fs::read_to_string(&path).unwrap();
        assert!(texts.contains(&text), "the file is one whole write, never a mix");
        assert_eq!(names_in(tmp.path()), ["p.project"]);
    }

    #[test]
    fn temporary_names_are_known_again() {
        let path = Path::new("a").join("p.x");
        let numbered = temp_path(&path, Some(42));
        assert_eq!(numbered, Path::new("a").join(".p.x.42.saving"));
        assert_eq!(temp_number(".p.x.42.saving", "p.x"), Some(Some(42)));
        assert_eq!(temp_number(".p.x.saving", "p.x"), Some(None));
        assert_eq!(temp_number(".p.x.042.saving", "p.x"), None);
        assert_eq!(temp_number(".p.x.4a.saving", "p.x"), None);
        assert_eq!(temp_number(".p.x.y.saving", "p.x"), None);
        assert_eq!(temp_number("p.x.42.saving", "p.x"), None);
        assert_eq!(temp_number(".p.x 2.saving", "p.x"), None);
    }

    #[test]
    fn old_leftover_writes_are_removed_and_young_ones_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let old = temp_path(&tmp.path().join("updates.ron"), Some(7));
        let older_style = temp_path(&tmp.path().join("settings.ron"), None);
        let young = temp_path(&tmp.path().join("health.ron"), Some(8));
        for path in [&old, &older_style, &young] {
            fs::write(path, "half a write").unwrap();
        }
        // Files that only look a little alike stay, however old.
        let kept = ["settings.ron", "notes.saving", ".saving", "a.saving.txt"];
        for name in kept {
            fs::write(tmp.path().join(name), "x").unwrap();
        }
        fs::create_dir(tmp.path().join(".folder.saving")).unwrap();
        let two_days_ago = SystemTime::now() - 2 * TEMP_LEFT_FOR;
        for name in kept
            .iter()
            .map(|n| tmp.path().join(n))
            .chain([old.clone(), older_style.clone()])
        {
            File::options()
                .write(true)
                .open(&name)
                .unwrap()
                .set_modified(two_days_ago)
                .unwrap();
        }

        assert_eq!(remove_old_temps(tmp.path(), TEMP_LEFT_FOR), 2);
        assert!(!old.exists() && !older_style.exists(), "old leftovers go");
        assert!(young.exists(), "a write that may be under way stays");
        for name in kept {
            assert!(tmp.path().join(name).exists(), "{name} stays");
        }
        assert!(tmp.path().join(".folder.saving").is_dir());
        assert_eq!(remove_old_temps(&tmp.path().join("missing"), TEMP_LEFT_FOR), 0);
    }
}
