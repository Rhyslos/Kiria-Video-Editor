//! Writing files so that nothing is ever half written, and one writer at a
//! time per file.
//!
//! - [`write_atomic`]: the whole text, or the old file untouched. The text
//!   goes to a temporary file of its own beside the target (a new name for
//!   every write, so two writers never share one), is put on the disk, and
//!   is then renamed over the old file.
//! - [`Keeper`]: keeps one small file up to date from the interface thread
//!   without waiting for the disk. Each change is handed over and written
//!   on a thread of the keeper's own, one write at a time; a change that
//!   arrives while one is being written replaces any change still waiting,
//!   so the newest always wins and an older one never lands after it. A
//!   failure is kept until asked for.
//!
//! The project file, the autosave journal's starting point, settings,
//! export presets, the labels for every project, templates and the
//! timeline's layout file are all written this way.

use diag::{AppError, Explain};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

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

/// Copies `from` to `to` the same way: whole, or `to` untouched.
pub fn copy_atomic(from: &Path, to: &Path) -> io::Result<()> {
    write_atomic(to, &fs::read(from)?)
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
/// moves and goes with its project.
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
pub(crate) fn strip_name<'a>(candidate: &'a str, name: &str) -> Option<&'a str> {
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

/// Makes the text to write; runs on the keeper's thread.
type Make = Box<dyn FnOnce() -> diag::Result<String> + Send>;

/// Keeps one file up to date, written on a thread of its own: see the
/// module's notes. Each file needs its one keeper, so that writes to it
/// never overlap.
pub struct Keeper {
    shared: Arc<Shared>,
}

struct Shared {
    path: PathBuf,
    /// What the file holds, for errors ("Your settings").
    what: String,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// The newest change not yet started, with its number.
    next: Option<(u64, Make)>,
    /// How many changes were handed over.
    asked: u64,
    /// The newest change that reached the disk.
    written: u64,
    /// True while the keeper's thread runs.
    running: bool,
    /// The newest failure, until asked for.
    failed: Option<AppError>,
}

impl std::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keeper").field("path", &self.shared.path).finish()
    }
}

impl Keeper {
    /// A keeper for the file at `path`. `what` names it in errors, as in
    /// "Your settings can't be saved".
    pub fn new(path: PathBuf, what: impl Into<String>) -> Keeper {
        Keeper {
            shared: Arc::new(Shared {
                path,
                what: what.into(),
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// Hands over a change and returns at once: `make` gives the file's new
    /// text and runs on the keeper's thread. A change still waiting is
    /// dropped for this one. Returns the change's number (see
    /// [`Keeper::written`]).
    pub fn keep(&self, make: impl FnOnce() -> diag::Result<String> + Send + 'static) -> u64 {
        let mut state = lock(&self.shared.state);
        state.asked += 1;
        let number = state.asked;
        state.next = Some((number, Box::new(make)));
        if state.running {
            return number;
        }
        state.running = true;
        drop(state);
        let shared = Arc::clone(&self.shared);
        let started = std::thread::Builder::new()
            .name("file keeper".into())
            .spawn(move || run(&shared));
        if let Err(err) = started {
            // Rare enough that writing here, once, is better than losing it.
            tracing::warn!("the file keeper's thread did not start ({err}); writing on the spot");
            run(&self.shared);
        }
        number
    }

    /// The number of the newest change that reached the disk (0 for none).
    pub fn written(&self) -> u64 {
        lock(&self.shared.state).written
    }

    /// True while a change waits or is being written.
    pub fn busy(&self) -> bool {
        lock(&self.shared.state).running
    }

    /// Why a write failed, once, since the last time this was asked.
    pub fn take_failure(&self) -> Option<AppError> {
        lock(&self.shared.state).failed.take()
    }

    /// Waits until every change handed over is written (for closing, and
    /// tests). Returns a failure not yet asked for.
    pub fn flush(&self) -> diag::Result<()> {
        let mut state = lock(&self.shared.state);
        while state.running {
            state = self.shared.changed.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        state.failed.take().map_or(Ok(()), Err)
    }
}

/// The keeper's thread: writes the newest change until none waits.
fn run(shared: &Shared) {
    loop {
        let mut state = lock(&shared.state);
        let Some((number, make)) = state.next.take() else {
            state.running = false;
            drop(state);
            shared.changed.notify_all();
            return;
        };
        drop(state);
        let result = make().and_then(|text| write_file(&shared.path, &shared.what, &text));
        let mut state = lock(&shared.state);
        match result {
            Ok(()) => state.written = state.written.max(number),
            Err(err) => {
                tracing::warn!("{}", err.report());
                state.failed = Some(err);
            }
        }
    }
}

fn write_file(path: &Path, what: &str, text: &str) -> diag::Result<()> {
    let explain = || {
        AppError::new(format!("{what} can't be saved"))
            .why(format!("Writing {} failed.", path.display()))
            .fix("Check that the disk is not full and that the folder can be written to.")
    };
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(folder).or_explain(explain)?;
    }
    write_atomic(path, text.as_bytes()).or_explain(explain)
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
    fn the_newest_change_wins_and_writes_never_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("export-presets.ron");
        let keeper = Keeper::new(path.clone(), "Your export presets");
        // The first write is held until released, so later changes queue.
        let (release, held) = mpsc::channel::<()>();
        let first = keeper.keep(move || {
            held.recv().unwrap();
            Ok("first".into())
        });
        let mut last = first;
        for i in 0..50 {
            last = keeper.keep(move || Ok(format!("change {i}")));
        }
        assert!(keeper.busy());
        release.send(()).unwrap();
        keeper.flush().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "change 49");
        assert_eq!(keeper.written(), last);
        assert!(!keeper.busy());
        assert_eq!(names_in(tmp.path()), ["export-presets.ron"]);
    }

    #[test]
    fn a_failure_is_reported_once_and_the_next_change_still_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let keeper = Keeper::new(tmp.path().join("labels.ron"), "Labels for every project");
        keeper.keep(|| Err(AppError::new("no text")));
        let err = keeper.flush().unwrap_err();
        assert_eq!(err.what(), "no text");
        assert!(keeper.take_failure().is_none(), "reported once");
        assert_eq!(keeper.written(), 0);

        // A folder in the way: the write itself fails, explained.
        fs::create_dir(tmp.path().join("labels.ron")).unwrap();
        fs::write(tmp.path().join("labels.ron").join("x"), "").unwrap();
        keeper.keep(|| Ok("labels".into()));
        let err = keeper.flush().unwrap_err();
        assert!(
            err.what().contains("Labels for every project"),
            "{}",
            err.report()
        );

        fs::remove_dir_all(tmp.path().join("labels.ron")).unwrap();
        let number = keeper.keep(|| Ok("labels".into()));
        keeper.flush().unwrap();
        assert_eq!(keeper.written(), number);
    }

    #[test]
    fn a_missing_folder_is_made() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("data").join("settings.ron");
        let keeper = Keeper::new(path.clone(), "Your settings");
        keeper.keep(|| Ok("()".into()));
        keeper.flush().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "()");
    }
}
