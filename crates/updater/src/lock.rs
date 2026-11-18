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

use diag::{AppError, Explain};
use std::fs::{File, TryLockError};
use std::path::Path;

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
}
