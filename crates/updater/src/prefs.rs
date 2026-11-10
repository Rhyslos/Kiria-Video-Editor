//! What the person chose about updates.

use crate::manifest::Channel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Download and install new versions without asking (installed on the next start).
    pub automatic: bool,
    pub channel: Channel,
    /// A version the person chose to skip; newer versions are still offered.
    pub skipped_version: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs {
            automatic: true,
            channel: Channel::Stable,
            skipped_version: None,
        }
    }
}

/// The file the choices are kept in, inside the app's data folder.
pub const PREFS_FILE_NAME: &str = "updates.ron";

impl UpdatePrefs {
    /// Reads the saved choices; anything missing or unreadable falls back to the defaults.
    pub fn load(data_dir: &std::path::Path) -> UpdatePrefs {
        std::fs::read_to_string(data_dir.join(PREFS_FILE_NAME))
            .ok()
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves the choices.
    pub fn save(&self, data_dir: &std::path::Path) -> diag::Result<()> {
        use diag::Explain;
        let explain = || {
            diag::AppError::new("Your update choices can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full. The choice still applies until the app closes.")
        };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(explain)?;
        std::fs::write(data_dir.join(PREFS_FILE_NAME), text).or_explain(explain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_survive_a_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), UpdatePrefs::default());
        let prefs = UpdatePrefs {
            automatic: false,
            channel: Channel::Beta,
            skipped_version: Some("0.2.0".into()),
        };
        prefs.save(tmp.path()).unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), prefs);
    }
}
