//! Settings: what people change in the Settings window, saved between runs.

use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ui_kit::ThemeName;

/// Everything in the Settings window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeName,
    pub resources: jobs::Budgets,
    /// Merge the red edit needle into the playhead.
    pub edit_at_playhead: bool,
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.ron")
    }

    /// Reads settings; missing or unreadable settings fall back to the defaults.
    pub fn load(data_dir: &Path) -> (Settings, Option<AppError>) {
        let path = Settings::path(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str(&text) {
                Ok(settings) => (settings, None),
                Err(err) => (
                    Settings::default(),
                    Some(
                        AppError::new("Your settings could not be read")
                            .why("The settings file is damaged.")
                            .fix("The defaults are in use. Change any setting to write a fresh file.")
                            .source(err),
                    ),
                ),
            },
            Err(_) => (Settings::default(), None),
        }
    }

    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .or_explain(|| AppError::new("Your settings could not be saved"))?;
        std::fs::write(Settings::path(data_dir), text).or_explain(|| {
            AppError::new("Your settings could not be saved")
                .why("Writing the settings file failed.")
                .fix("Check that the disk is not full.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_damage_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            theme: ThemeName::Carbon,
            edit_at_playhead: true,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);

        std::fs::write(Settings::path(tmp.path()), "not settings").unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert_eq!(loaded, Settings::default());
        assert!(problem.is_some());
    }

    #[test]
    fn a_missing_file_means_defaults_and_no_complaint() {
        let tmp = tempfile::tempdir().unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert_eq!(loaded, Settings::default());
        assert!(problem.is_none());
    }
}
