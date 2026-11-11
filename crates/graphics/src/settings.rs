//! The Graphics setting: one small file in the data folder that the launcher
//! and the editor both read at start and write when the choice changes.

use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "graphics.ron";
/// The editor's own settings file, which held the choice before it was shared.
const EARLIER_FILE_NAME: &str = "settings.ron";

/// What draws the windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GraphicsChoice {
    /// The app decides: on Windows Vulkan, and Direct3D 12 if Vulkan doesn't start.
    #[default]
    Automatic,
    Vulkan,
    Direct3D12,
}

impl GraphicsChoice {
    pub const ALL: [GraphicsChoice; 3] = [
        GraphicsChoice::Automatic,
        GraphicsChoice::Vulkan,
        GraphicsChoice::Direct3D12,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GraphicsChoice::Automatic => "Automatic",
            GraphicsChoice::Vulkan => "Vulkan",
            GraphicsChoice::Direct3D12 => "Direct3D 12",
        }
    }
}

/// The contents of `graphics.ron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub choice: GraphicsChoice,
}

impl GraphicsSettings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the setting. Without the file, the choice the editor kept in its
    /// own settings before is used; a missing or unreadable file means Automatic.
    pub fn load(data_dir: &Path) -> GraphicsSettings {
        match std::fs::read_to_string(GraphicsSettings::path(data_dir)) {
            Ok(text) => ron::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!("the graphics setting can't be read ({err}); using Automatic");
                GraphicsSettings::default()
            }),
            Err(_) => GraphicsSettings::earlier(data_dir).unwrap_or_default(),
        }
    }

    /// The choice from the editor's settings file, if this file doesn't exist yet.
    pub fn earlier(data_dir: &Path) -> Option<GraphicsSettings> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct EditorSettings {
            graphics: GraphicsChoice,
        }
        if GraphicsSettings::path(data_dir).exists() {
            return None;
        }
        let text = std::fs::read_to_string(data_dir.join(EARLIER_FILE_NAME)).ok()?;
        let earlier: EditorSettings = ron::from_str(&text).ok()?;
        Some(GraphicsSettings {
            choice: earlier.graphics,
        })
    }

    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("The graphics setting could not be saved")
                .why(format!(
                    "Writing {} failed.",
                    GraphicsSettings::path(data_dir).display()
                ))
                .fix("Check that the disk is not full.")
        };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(explain)?;
        std::fs::write(GraphicsSettings::path(data_dir), text).or_explain(explain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_round_trips_and_a_missing_or_damaged_file_means_automatic() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(GraphicsSettings::load(tmp.path()), GraphicsSettings::default());
        let chosen = GraphicsSettings {
            choice: GraphicsChoice::Direct3D12,
        };
        chosen.save(tmp.path()).unwrap();
        assert_eq!(GraphicsSettings::load(tmp.path()), chosen);
        std::fs::write(GraphicsSettings::path(tmp.path()), "not a setting").unwrap();
        assert_eq!(
            GraphicsSettings::load(tmp.path()).choice,
            GraphicsChoice::Automatic
        );
    }

    #[test]
    fn the_choice_the_editor_kept_before_is_used_until_the_shared_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let editor_file = tmp.path().join(EARLIER_FILE_NAME);
        std::fs::write(&editor_file, "(edit_at_playhead: true, graphics: Direct3D12)").unwrap();
        assert_eq!(
            GraphicsSettings::load(tmp.path()).choice,
            GraphicsChoice::Direct3D12
        );
        assert!(GraphicsSettings::earlier(tmp.path()).is_some());

        GraphicsSettings {
            choice: GraphicsChoice::Vulkan,
        }
        .save(tmp.path())
        .unwrap();
        assert_eq!(GraphicsSettings::load(tmp.path()).choice, GraphicsChoice::Vulkan);
        assert_eq!(GraphicsSettings::earlier(tmp.path()), None);

        // An editor settings file without the choice is nothing to adopt either.
        std::fs::remove_file(GraphicsSettings::path(tmp.path())).unwrap();
        std::fs::write(&editor_file, "(edit_at_playhead: true)").unwrap();
        assert_eq!(
            GraphicsSettings::load(tmp.path()).choice,
            GraphicsChoice::Automatic
        );
    }
}
