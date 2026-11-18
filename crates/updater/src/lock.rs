//! One launcher updates at a time.
//!
//! Opening two projects at once starts two launchers. Downloading,
//! installing and the check after a start all share files (the `.part`
//! download, the `.partial` folder, `current.new`, the launcher being
//! replaced, `health.ron`), so each of those steps runs only while its
//! launcher holds [`UpdateLock`]: an exclusive lock on `update.lock` in the
//! data folder, the same kind of lock the editor's session markers use
//! (`diag::session`). The system lets go of it when the process ends,
//! however it ends, so a crash never leaves updates locked.
//!
//! Taking it never waits: a launcher that finds it taken skips that step
//! this time and starts normally.
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

/// Held while one launcher downloads, installs or checks a start. Dropping
/// it lets go.
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
        let path = data_dir.join(CHOICES_LOCK_FILE);
        let file = match File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
        {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!(file = %path.display(), "the update choices are changed without their lock ({err})");
                return None;
            }
        };
        let deadline = Instant::now() + longest;
        loop {
            match file.try_lock() {
                Ok(()) => return Some(ChoicesLock { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TryLockError::WouldBlock) => {
                    tracing::warn!(
                        "the update choices are still locked after {longest:?}; changing them anyway"
                    );
                    return None;
                }
                Err(TryLockError::Error(err)) => {
                    tracing::warn!(file = %path.display(), "the update choices are changed without their lock ({err})");
                    return None;
                }
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
}
