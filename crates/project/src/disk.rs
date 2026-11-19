//! Writing files so that nothing is ever half written, and one writer at a
//! time per file.
//!
//! - [`write_atomic`]: the whole text, or the old file untouched. The text
//!   goes to a temporary file of its own beside the target (a new name for
//!   every write, so two writers never share one), is put on the disk, and
//!   is then renamed over the old file. It is `diag::disk::write_atomic`,
//!   the one writer of the workspace (the updater's files use it too).
//! - [`Keeper`]: keeps one small file up to date from the interface thread
//!   without waiting for the disk. Each change is handed over and written
//!   on a thread of the keeper's own, one write at a time; a change that
//!   arrives while one is being written replaces any change still waiting,
//!   so the newest always wins and an older one never lands after it. A
//!   failure is kept until asked for.
//!
//! The project file, its backup before an upgrade, the autosave journal's
//! starting point, settings, export presets, the labels for every project,
//! templates and the timeline's layout file are all written this way.

use diag::{AppError, Explain};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub(crate) use diag::disk::strip_name;
pub use diag::disk::{temp_number, temp_path, write_atomic};

/// Copies `from` to `to` the same way: whole, or `to` untouched.
pub fn copy_atomic(from: &Path, to: &Path) -> io::Result<()> {
    write_atomic(to, &fs::read(from)?)
}

/// The most characters [`clean_file_name`] keeps of a name.
pub const MOST_NAME_CHARS: usize = 60;

/// `name` made safe for a file or folder name on every system the app runs
/// on, or `fallback` when nothing of it is left: [`clean_file_name`], then
/// a name Windows keeps for a device ("aux", "CON.txt") gets a dash after
/// its first part ("aux-", "CON-.txt"), so it is an ordinary name there.
/// Package folders, saved versions and templates all go by it.
pub fn safe_file_name(name: &str, fallback: &str) -> String {
    let cleaned = clean_file_name(name);
    let mut name = if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned
    };
    if is_device_name(&name) {
        let first = name.split('.').next().unwrap_or_default().trim_end().len();
        name.insert(first, '-');
    }
    name
}

/// `name` with what file systems refuse taken out, without the device rule
/// of [`safe_file_name`] (for callers that refuse such names with a reason,
/// or put something in front of the name): path, reserved and control
/// characters become dashes, spaces and dots go from both ends (Windows
/// drops trailing ones, and leading dots hide a file elsewhere), and at most
/// [`MOST_NAME_CHARS`] characters are kept. Empty when nothing is left.
pub fn clean_file_name(name: &str) -> String {
    let unwanted = |c: char| c.is_whitespace() || c == '.';
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let capped: String = replaced
        .trim_matches(unwanted)
        .chars()
        .take(MOST_NAME_CHARS)
        .collect();
    capped.trim_matches(unwanted).to_owned()
}

/// Whether Windows keeps `name` for a device: CON, PRN, AUX, NUL, CONIN$,
/// CONOUT$, COM0-9 and LPT0-9 (also with ¹ ² ³), in any capitals, alone or
/// with an extension ("nul.txt"), and with spaces before the extension.
pub fn is_device_name(name: &str) -> bool {
    const KEPT: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];
    let first = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_lowercase();
    let numbered = |prefix: &str| {
        first.strip_prefix(prefix).is_some_and(|rest| {
            let mut chars = rest.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('0'..='9' | '¹' | '²' | '³'), None)
            )
        })
    };
    KEPT.contains(&first.as_str()) || numbered("com") || numbered("lpt")
}

/// One change: writes the file, on the keeper's thread.
type Make = Box<dyn FnOnce(&Shared) -> diag::Result<()> + Send>;

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
        self.hand_over(Box::new(move |shared: &Shared| {
            make().and_then(|text| write_file(&shared.path, &shared.what, &text))
        }))
    }

    /// [`Keeper::keep`] for a change that writes the file itself, for a
    /// file whose writer must do more around the write (`updates.ron` is
    /// read, changed and written under a lock between launchers). `write`
    /// runs on the keeper's thread like any change; it should write whole
    /// or not at all (`write_atomic`).
    pub fn keep_written(&self, write: impl FnOnce() -> diag::Result<()> + Send + 'static) -> u64 {
        self.hand_over(Box::new(move |_: &Shared| write()))
    }

    fn hand_over(&self, make: Make) -> u64 {
        let mut state = lock(&self.shared.state);
        state.asked += 1;
        let number = state.asked;
        state.next = Some((number, make));
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
        let result = make(shared);
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
    fn unsafe_file_names_are_made_safe() {
        assert_eq!(safe_file_name("aux", "Untitled"), "aux-");
        assert_eq!(safe_file_name("Con", "Untitled"), "Con-");
        assert_eq!(safe_file_name("CON.txt", "Untitled"), "CON-.txt");
        assert_eq!(safe_file_name("nul .tar.gz", "Untitled"), "nul- .tar.gz");
        assert_eq!(safe_file_name("COM1", "Untitled"), "COM1-");
        assert_eq!(safe_file_name("lpt²", "Untitled"), "lpt²-");
        assert_eq!(safe_file_name("..", "Untitled"), "Untitled");
        assert_eq!(safe_file_name("a/b", "Untitled"), "a-b");
        assert_eq!(safe_file_name("  x. ", "Untitled"), "x");
        assert_eq!(safe_file_name("", "Untitled"), "Untitled");
        assert_eq!(safe_file_name(" .aux. ", "Untitled"), "aux-");
        assert_eq!(safe_file_name("tab\there", "Untitled"), "tab-here");
        let long = "é".repeat(300);
        assert_eq!(safe_file_name(&long, "Untitled"), "é".repeat(MOST_NAME_CHARS));
        // Capped first, then trimmed again: no dot or space left at the end.
        let dotted = format!("{}. tail", "a".repeat(MOST_NAME_CHARS - 2));
        assert_eq!(safe_file_name(&dotted, "x"), "a".repeat(MOST_NAME_CHARS - 2));
        for kept in ["Common", "console", "Auxiliary", "com10", "LPT", "nul-", "con 2"] {
            assert!(!is_device_name(kept), "{kept}");
            assert_eq!(safe_file_name(kept, "x"), kept);
        }
    }

    /// Ordinary names keep the result the three cleaners this one replaced
    /// gave them, so packages, versions and templates already on disk are
    /// still found by the names they were saved under.
    #[test]
    fn ordinary_names_keep_their_old_result() {
        for (name, kept) in [
            ("Trip", "Trip"),
            ("Trip v2", "Trip v2"),
            ("Channel intro", "Channel intro"),
            ("intro 2026", "intro 2026"),
            ("Trip: day 1", "Trip- day 1"),
            ("Before colour: a/b", "Before colour- a-b"),
            ("Cut 2: the \"best\" bits?", "Cut 2- the -best- bits-"),
            ("  Holiday 2026  ", "Holiday 2026"),
            ("v1.2 final", "v1.2 final"),
            ("Été à Paris", "Été à Paris"),
            ("家族旅行", "家族旅行"),
            ("a|b<c>d*e", "a-b-c-d-e"),
            ("Common", "Common"),
        ] {
            assert_eq!(clean_file_name(name), kept, "{name}");
            assert_eq!(safe_file_name(name, "Untitled"), kept, "{name}");
        }
    }

    #[test]
    fn the_newest_change_wins_and_writes_never_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("export-presets.ron");
        let keeper = Keeper::new(path.clone(), "Your export presets");
        // The first write is held until released, so later changes queue.
        // Wait until the writer has started it: a change still waiting may
        // rightly be replaced by a newer one before it is ever run.
        let (release, held) = mpsc::channel::<()>();
        let (started, has_started) = mpsc::channel::<()>();
        let first = keeper.keep(move || {
            started.send(()).unwrap();
            held.recv().unwrap();
            Ok("first".into())
        });
        has_started.recv().unwrap();
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

    #[test]
    fn a_change_that_writes_itself_runs_in_turn_and_reports_its_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("updates.ron");
        let keeper = Keeper::new(path.clone(), "Your update choices");
        let target = path.clone();
        let number = keeper.keep_written(move || {
            write_atomic(&target, b"written here").or_explain(|| AppError::new("no write"))
        });
        keeper.flush().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "written here");
        assert_eq!(keeper.written(), number);
        keeper.keep_written(|| Err(AppError::new("locked out")));
        assert_eq!(keeper.flush().unwrap_err().what(), "locked out");
        assert_eq!(keeper.written(), number);
    }
}
