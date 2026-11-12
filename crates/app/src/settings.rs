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
    /// How sharp the viewer's picture is.
    pub preview_quality: ui_kit::PreviewQuality,
    /// The timeline in the rough cut view (sound folded into the pictures).
    pub rough_cut: bool,
    /// Every track of the timeline a thin strip (the Display menu's
    /// Compact tracks). The heights tracks were given are kept per project
    /// and come back when it is off.
    pub compact_tracks: bool,
    /// No divider between the timeline's video and audio tracks: one list
    /// that scrolls together (the Display menu's divider line, off).
    pub one_track_list: bool,
    /// How the height is shared between the viewer and the timeline (the
    /// handle between them), on every page that has both.
    pub viewer_split: ui_kit::splitter::Split,
    /// The shortcuts that were changed on the keyboard shortcuts page (the
    /// usual ones are not written down).
    pub shortcuts: ui_kit::shortcuts::SavedBindings,
    /// The keyboard the shortcuts page draws.
    pub keyboard_layout: ui_kit::keyboard::LayoutId,
    /// The selected clip's outline and handles on the picture in the viewer.
    /// Off unless switched on (Q, or "On the picture" in the inspector).
    pub picture_handles: bool,
    /// The Edit page's viewer shows the selected clip's own picture (Source)
    /// beside the timeline's (Program).
    pub split_view: bool,
    /// Where the viewer's own window was last, so it opens there again.
    pub viewer_window: Option<crate::viewer_window::WindowPlace>,
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
    fn the_ways_of_looking_at_tracks_are_off_until_turned_on_and_kept() {
        let usual = Settings::default();
        assert!(
            !usual.compact_tracks && !usual.one_track_list,
            "usual tracks, with the divider"
        );
        // Settings written before they existed read them as off.
        let old: Settings = ron::from_str("(rough_cut: true)").unwrap();
        assert!(!old.compact_tracks && !old.one_track_list);
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            compact_tracks: true,
            one_track_list: true,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
    }

    #[test]
    fn handles_on_the_picture_are_off_unless_switched_on() {
        assert!(!Settings::default().picture_handles);
        // Settings written before the switch existed read it as off.
        let old: Settings = ron::from_str("(edit_at_playhead: true)").unwrap();
        assert!(!old.picture_handles);
    }

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
    fn settings_with_fewer_or_other_fields_still_load() {
        // The Graphics choice was kept here for a while; it now has its own file.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(tmp.path()),
            "(edit_at_playhead: true, graphics: Direct3D12)",
        )
        .unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(loaded.edit_at_playhead);
        assert_eq!(
            loaded.viewer_split,
            ui_kit::splitter::Split::default(),
            "settings from before the handle between viewer and timeline"
        );
        assert_eq!(
            ui_kit::ShortcutTable::restored(&loaded.shortcuts),
            ui_kit::ShortcutTable::default(),
            "settings from before shortcuts could be changed"
        );
    }

    #[test]
    fn split_view_and_the_viewers_window_are_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!Settings::default().split_view, "off until turned on");
        let settings = Settings {
            split_view: true,
            viewer_window: Some(crate::viewer_window::WindowPlace {
                x: 1930.0,
                y: 20.0,
                width: 1280.0,
                height: 720.0,
            }),
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0, settings);
        // Settings from before either was kept still load.
        std::fs::write(Settings::path(tmp.path()), "(rough_cut: true)").unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(!loaded.split_view && loaded.viewer_window.is_none());
    }

    #[test]
    fn the_viewer_and_timeline_split_is_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = Settings {
            viewer_split: ui_kit::splitter::Split(0.7),
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()).0.viewer_split.0, 0.7);
    }

    #[test]
    fn changed_shortcuts_and_the_keyboard_are_remembered() {
        use ui_kit::{Action, Shortcut, ShortcutTable};
        let tmp = tempfile::tempdir().unwrap();
        let mut table = ShortcutTable::default();
        let split = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Split)
            .unwrap();
        // Split and Save trade places, and Undo loses its shortcut.
        table.swap(split, Shortcut::ctrl(egui::Key::S));
        let undo = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Undo)
            .unwrap();
        table.clear(undo);
        let settings = Settings {
            shortcuts: table.saved(),
            keyboard_layout: ui_kit::keyboard::LayoutId::Norwegian,
            ..Settings::default()
        };
        settings.save(tmp.path()).unwrap();

        let text = std::fs::read_to_string(Settings::path(tmp.path())).unwrap();
        assert!(text.contains("\"Ctrl+S\""), "kept as readable text: {text}");
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert_eq!(loaded, settings);
        let restored = ShortcutTable::restored(&loaded.shortcuts);
        assert_eq!(restored, table);
        assert_eq!(
            restored.shortcut_for(Action::Split),
            Some(Shortcut::ctrl(egui::Key::S))
        );
        assert_eq!(
            restored.shortcut_for(Action::Save),
            Some(Shortcut::key(egui::Key::S))
        );
        assert_eq!(restored.shortcut_for(Action::Undo), None);
    }

    #[test]
    fn shortcuts_this_version_does_not_know_never_cost_the_other_settings() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            Settings::path(tmp.path()),
            "(edit_at_playhead: true, shortcuts: [(command: \"FlyToTheMoon\", nth: 0, keys: Some(\"Ctrl+M\")), (command: \"Split\", keys: Some(\"F9\"))])",
        )
        .unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert!(problem.is_none());
        assert!(loaded.edit_at_playhead);
        let table = ui_kit::ShortcutTable::restored(&loaded.shortcuts);
        assert_eq!(
            table.shortcut_for(ui_kit::Action::Split),
            Some(ui_kit::Shortcut::key(egui::Key::F9))
        );
    }

    #[test]
    fn a_missing_file_means_defaults_and_no_complaint() {
        let tmp = tempfile::tempdir().unwrap();
        let (loaded, problem) = Settings::load(tmp.path());
        assert_eq!(loaded, Settings::default());
        assert!(problem.is_none());
    }
}
