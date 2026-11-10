//! Where the app keeps its own files.

use crate::{AppError, Explain, Result};
use std::path::{Path, PathBuf};

/// The app's own folders.
///
/// On Windows they live under `%LOCALAPPDATA%\<app name>`, on Linux under
/// `~/.local/share/<app name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Root of everything the app stores for this user.
    pub data: PathBuf,
    pub logs: PathBuf,
    pub crashes: PathBuf,
}

impl AppDirs {
    /// Finds the folders for the current user. Does not create them.
    pub fn locate() -> Result<Self> {
        let base = directories::BaseDirs::new().ok_or_else(|| {
            AppError::new("The app can't find your user folder")
                .why("The operating system did not report a home or app-data folder for this user.")
                .fix("Check that your user account has a home folder, then start the app again.")
        })?;
        Ok(Self::under(base.data_local_dir().join(brand::DATA_DIR_NAME)))
    }

    /// The layout under a given root folder (used directly by tests).
    pub fn under(root: impl AsRef<Path>) -> Self {
        let data = root.as_ref().to_path_buf();
        Self {
            logs: data.join("logs"),
            crashes: data.join("crash-reports"),
            data,
        }
    }

    /// Creates any folder that is missing.
    pub fn create(&self) -> Result<()> {
        for dir in [&self.data, &self.logs, &self.crashes] {
            std::fs::create_dir_all(dir).or_explain(|| {
                AppError::new("The app can't create its data folder")
                    .why(format!("Creating {} failed.", dir.display()))
                    .fix("Check that the disk is not full and that you can write to this folder.")
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_sit_under_the_root() {
        let dirs = AppDirs::under("/root");
        assert_eq!(dirs.data, PathBuf::from("/root"));
        assert_eq!(dirs.logs, PathBuf::from("/root/logs"));
        assert_eq!(dirs.crashes, PathBuf::from("/root/crash-reports"));
    }

    #[test]
    fn create_makes_every_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(tmp.path().join("app"));
        dirs.create().unwrap();
        assert!(dirs.logs.is_dir() && dirs.crashes.is_dir());
    }
}
