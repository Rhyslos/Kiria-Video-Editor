//! The launcher's own choices, kept in `launcher.ron` in the data folder.

use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file the choices are kept in.
pub const FILE_NAME: &str = "launcher.ron";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherPrefs {
    /// Where New project saves unless another folder is picked. `None` until chosen
    /// (the first run picks the projects folder it makes in Documents).
    pub default_project_folder: Option<PathBuf>,
}

impl LauncherPrefs {
    /// Reads the saved choices; anything missing or unreadable falls back to the defaults.
    pub fn load(data_dir: &Path) -> LauncherPrefs {
        std::fs::read_to_string(data_dir.join(FILE_NAME))
            .ok()
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("Your choice can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full. The choice still applies until the launcher closes.")
        };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(explain)?;
        std::fs::write(data_dir.join(FILE_NAME), text).or_explain(explain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folder_survives_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(LauncherPrefs::load(tmp.path()), LauncherPrefs::default());
        let prefs = LauncherPrefs {
            default_project_folder: Some("D:/Videos/Projects".into()),
        };
        prefs.save(tmp.path()).unwrap();
        assert_eq!(LauncherPrefs::load(tmp.path()), prefs);
    }

    #[test]
    fn a_damaged_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FILE_NAME), "((((").unwrap();
        assert_eq!(LauncherPrefs::load(tmp.path()), LauncherPrefs::default());
    }
}
