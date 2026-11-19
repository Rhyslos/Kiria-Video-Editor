//! One launcher updates at a time.
//!
//! Opening two projects at once starts two launchers. Downloading and
//! installing share files (the `.part` download, the `.partial` folder,
//! `current.new`, the launcher being replaced, the version folders), so
//! each of those steps runs only while its launcher holds [`UpdateLock`]:
//! an exclusive lock on `update.lock` in the data folder, the same kind of
//! lock the editor's session markers use (`diag::session`). The system lets
//! go of it when the process ends, however it ends, so a crash never leaves
//! updates locked. After a good start, swapping in the new launcher and
//! removing old versions also run only under it.
//!
//! Taking it never waits: a download holds it for minutes, so a launcher
//! that finds it taken skips that step this time and starts normally.
//!
//! [`StartCheckLock`] is a short lock of its own (`start-check.lock`) held
//! while the outcome of an editor's start is recorded in `health.ron` and a
//! version that keeps failing is rolled back ([`crate::health::after_start`]),
//! so that step is never skipped because another launcher is downloading.
//! Taking it waits, but only a few seconds ([`START_CHECK_WAIT`]).
//!
//! [`ChoicesLock`] is a second, short lock (`updates.ron.lock`) held only
//! while `updates.ron` is read, changed and written ([`crate::UpdatePrefs::change`],
//! the rollback in [`crate::health::after_start`]), so two launchers' changes
//! never land on the same old copy and a version marked bad is never written
//! away. The writes are tiny, so taking it waits, but only a few seconds
//! ([`CHOICES_WAIT`]); it is never taken on an interface thread.

use diag::{AppError, Explain};
use std::fs::{File, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

/// The lock file, in the data folder. It stays there; only the lock on it
/// comes and goes (removing it could let two launchers lock two files).
pub const LOCK_FILE: &str = "update.lock";

/// Held while one launcher downloads or installs, or after a good start
/// swaps in the new launcher and removes old versions. Dropping it lets go.
#[derive(Debug)]
pub struct UpdateLock {
    _file: File,
}

impl UpdateLock {
    /// Takes the lock without waiting: `Ok(None)` when another launcher
    /// (or another holder in this process) has it.
    pub fn try_take(data_dir: &Path) -> diag::Result<Option<UpdateLock>> {
        let path = data_dir.join(LOCK_FILE);
        let cannot = || {
            AppError::new("Updates can't be made safe from a second window")
                .why(format!("The file {} can't be opened.", path.display()))
                .fix("Check that the disk is not full and that you can write to the app's data folder.")
        };
        let file = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .or_explain(cannot)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(UpdateLock { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(err)) => Err(cannot().source(err)),
        }
    }

    /// [`UpdateLock::try_take`] for a step that is simply skipped without
    /// it: says in the log why `step` is skipped, and shows nothing.
    pub fn for_step(data_dir: &Path, step: &str) -> Option<UpdateLock> {
        match UpdateLock::try_take(data_dir) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                tracing::info!("another launcher is updating: {step} is skipped this time");
                None
            }
            Err(err) => {
                tracing::warn!("{step} is skipped this time: {}", err.report());
                None
            }
        }
    }
}

/// The lock file of [`ChoicesLock`], in the data folder; it stays there.
pub const CHOICES_LOCK_FILE: &str = "updates.ron.lock";

/// The longest [`ChoicesLock::wait`] waits.
pub const CHOICES_WAIT: Duration = Duration::from_secs(3);

/// Held while `updates.ron` is read, changed and written. Dropping it lets go.
#[derive(Debug)]
pub struct ChoicesLock {
    _file: File,
}

impl ChoicesLock {
    /// Takes the lock, waiting up to [`CHOICES_WAIT`] for another holder
    /// (another launcher, or another thread here) to let go. `None` when it
    /// couldn't be had: the reason is logged and the caller goes ahead
    /// without it, since a holder stuck for seconds is something gone wrong
    /// and losing the change (a version marked bad above all) is worse.
    pub fn wait(data_dir: &Path) -> Option<ChoicesLock> {
        Self::wait_for(data_dir, CHOICES_WAIT)
    }

    fn wait_for(data_dir: &Path, longest: Duration) -> Option<ChoicesLock> {
        wait_on(
            &data_dir.join(CHOICES_LOCK_FILE),
            longest,
            "the update choices are changed",
        )
        .map(|file| ChoicesLock { _file: file })
    }
}

/// The lock file of [`StartCheckLock`], in the data folder; it stays there.
pub const START_CHECK_LOCK_FILE: &str = "start-check.lock";

/// The longest [`StartCheckLock::wait`] waits.
pub const START_CHECK_WAIT: Duration = Duration::from_secs(5);

/// Held while the outcome of an editor's start is recorded (and a version
/// rolled back). Dropping it lets go.
#[derive(Debug)]
pub struct StartCheckLock {
    _file: File,
}

impl StartCheckLock {
    /// Takes the lock, waiting up to [`START_CHECK_WAIT`] for another
    /// launcher recording its own start to let go. `None` when it couldn't
    /// be had: the reason is logged and the caller goes ahead without it,
    /// since a start that is never counted (a rollback missed) is worse.
    /// Never called on an interface thread.
    pub fn wait(data_dir: &Path) -> Option<StartCheckLock> {
        Self::wait_for(data_dir, START_CHECK_WAIT)
    }

    fn wait_for(data_dir: &Path, longest: Duration) -> Option<StartCheckLock> {
        wait_on(
            &data_dir.join(START_CHECK_LOCK_FILE),
            longest,
            "the editor's start is recorded",
        )
        .map(|file| StartCheckLock { _file: file })
    }
}

/// Locks the file at `path`, waiting up to `longest`. `None` (logged, with
/// `step` saying what goes ahead without it) when it can't be had.
fn wait_on(path: &Path, longest: Duration, step: &str) -> Option<File> {
    let file = match File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
    {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(file = %path.display(), "{step} without its lock ({err})");
            return None;
        }
    };
    let deadline = Instant::now() + longest;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(TryLockError::WouldBlock) => {
                tracing::warn!(file = %path.display(), "still locked after {longest:?}: {step} anyway");
                return None;
            }
            Err(TryLockError::Error(err)) => {
                tracing::warn!(file = %path.display(), "{step} without its lock ({err})");
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_until_the_first_lets_go() {
        let tmp = tempfile::tempdir().unwrap();
        let first = UpdateLock::try_take(tmp.path()).unwrap();
        assert!(first.is_some(), "a free lock is taken");
        assert!(
            UpdateLock::try_take(tmp.path()).unwrap().is_none(),
            "a second holder is refused"
        );
        assert!(UpdateLock::for_step(tmp.path(), "installing").is_none());
        drop(first);
        let again = UpdateLock::try_take(tmp.path()).unwrap();
        assert!(again.is_some(), "free again once the first is dropped");
        assert!(tmp.path().join(LOCK_FILE).is_file());
    }

    #[test]
    fn a_folder_that_cannot_hold_the_file_is_an_explained_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no such folder");
        let err = UpdateLock::try_take(&missing).unwrap_err();
        assert!(err.report().contains(LOCK_FILE));
        assert!(UpdateLock::for_step(&missing, "downloading").is_none());
    }

    #[test]
    fn the_choices_lock_waits_for_its_holder_and_gives_up_in_time() {
        let tmp = tempfile::tempdir().unwrap();
        let held = ChoicesLock::wait(tmp.path()).unwrap();
        let started = Instant::now();
        assert!(ChoicesLock::wait_for(tmp.path(), Duration::from_millis(50)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(50), "it waited first");
        // Let go while another waits: that one gets it.
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| ChoicesLock::wait(tmp.path()).is_some());
            std::thread::sleep(Duration::from_millis(30));
            drop(held);
            assert!(waiter.join().unwrap());
        });
        // Not the update lock: one doesn't keep the other.
        let _choices = ChoicesLock::wait(tmp.path()).unwrap();
        assert!(UpdateLock::try_take(tmp.path()).unwrap().is_some());
    }

    #[test]
    fn the_start_check_lock_is_free_while_another_launcher_downloads() {
        let tmp = tempfile::tempdir().unwrap();
        let _downloading = UpdateLock::try_take(tmp.path()).unwrap().unwrap();
        let held = StartCheckLock::wait(tmp.path()).unwrap();
        // A second launcher's start waits for the first, then gets it.
        let started = Instant::now();
        assert!(StartCheckLock::wait_for(tmp.path(), Duration::from_millis(50)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(50), "it waited first");
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| StartCheckLock::wait(tmp.path()).is_some());
            std::thread::sleep(Duration::from_millis(30));
            drop(held);
            assert!(waiter.join().unwrap());
        });
        assert!(tmp.path().join(START_CHECK_LOCK_FILE).is_file());
    }
}
