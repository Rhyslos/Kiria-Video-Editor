//! The launcher's Settings: what it holds (`CATEGORIES`) and each row's
//! control, drawn by `ui_kit::settings` exactly as the editor's are. The
//! Graphics card is the editor's own (`graphics::SETTINGS`).

use super::{Browser, Picked};
use ui_kit::settings::{Category, Section, Setting};
use updater::Channel;

const VERSION: &str = "version";
const CHANNEL: &str = "update-channel";
const AUTOMATIC: &str = "automatic-updates";
const NEW_PROJECTS: &str = "new-projects-folder";
const THEME: &str = "theme";
const DIAGNOSTICS: &str = "diagnostics";
const LOG_FOLDER: &str = "log-folder";

/// Everything in the launcher's Settings, tab by tab.
pub const CATEGORIES: &[Category] = &[
    Category {
        id: "updates",
        label: "Updates",
        sections: &[Section {
            id: "updates-section",
            title: "Updates",
            settings: &[
                Setting::new(
                    VERSION,
                    "Version",
                    "The version installed, and whether a newer one is out.",
                )
                .keywords("update check release"),
                Setting::new(
                    CHANNEL,
                    "Versions to get",
                    "Betas are test versions, before they become stable releases.",
                )
                .keywords("beta stable channel release"),
                Setting::new(
                    AUTOMATIC,
                    "Automatic updates",
                    "Download new versions as soon as they are out.",
                )
                .keywords("download install update"),
            ],
        }],
    },
    Category {
        id: "projects",
        label: "Projects",
        sections: &[Section {
            id: "new-projects",
            title: "New projects",
            settings: &[Setting::new(
                NEW_PROJECTS,
                "Save new projects in",
                "New project suggests this folder; Browse… there saves one anywhere.",
            )
            .keywords("folder default location documents")],
        }],
    },
    Category {
        id: "look",
        label: "Look",
        sections: &[Section {
            id: "appearance",
            title: "Appearance",
            settings: &[Setting::new(
                THEME,
                "Theme",
                "Follows the editor's Settings, where it is chosen.",
            )
            .keywords("ash carbon dark colour color appearance")],
        }],
    },
    Category {
        id: "graphics",
        label: "Graphics",
        sections: &[graphics::SETTINGS],
    },
    Category {
        id: "diagnostics-tab",
        label: "Diagnostics",
        sections: &[Section {
            id: "problems",
            title: "When something goes wrong",
            settings: &[
                Setting::new(
                    DIAGNOSTICS,
                    "Diagnostics",
                    "Every problem of this session, and what this machine is using.",
                )
                .keywords("errors problems report gpu"),
                Setting::new(
                    LOG_FOLDER,
                    "Log folder",
                    "Where the launcher writes what it did, for a bug report.",
                )
                .keywords("logs path report bug"),
            ],
        }],
    },
    Category {
        id: "help",
        label: "Help",
        sections: &[ui_kit::user_guide::SETTINGS],
    },
];

impl Browser {
    /// The Settings window, when it is open.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let theme = self.theme;
        let mut open = self.show_settings;
        let mut change_default = false;
        let mut problems = Vec::new();
        let folder = self
            .default_project_folder()
            .map_or_else(|| "No folder chosen".to_owned(), |f| f.display().to_string());
        ui_kit::settings::window(ctx, &theme, &mut open, |ui| {
            self.settings_view
                .show(ui, &theme, CATEGORIES, |row| match row.id() {
                    VERSION => match &self.updates {
                        Some(updates) => {
                            let (line, colour) = updates.state_line(row.theme);
                            row.ui.label(egui::RichText::new(line).size(12.5).color(colour));
                            row.note(format!("{} {}", brand::APP_NAME, updates.version()));
                        }
                        None => {
                            ui_kit::settings::value(row.ui, row.theme, "Not checked");
                        }
                    },
                    CHANNEL => {
                        if let Some(updates) = &mut self.updates {
                            let mut channel = updates.channel();
                            ui_kit::settings::segmented(
                                row.ui,
                                row.theme,
                                &mut channel,
                                &[(Channel::Stable, "Stable releases"), (Channel::Beta, "Betas too")],
                            );
                            updates.set_channel(row.ui.ctx(), channel);
                        }
                    }
                    AUTOMATIC => {
                        if let Some(updates) = &mut self.updates {
                            let mut automatic = updates.automatic();
                            if ui_kit::settings::toggle(row.ui, row.theme, &mut automatic).changed() {
                                updates.set_automatic(row.ui.ctx(), automatic);
                            }
                        }
                    }
                    NEW_PROJECTS => {
                        if ui_kit::settings::action(row.ui, "Change…").clicked() {
                            change_default = true;
                        }
                        row.note(folder.clone());
                    }
                    THEME => {
                        ui_kit::settings::value(row.ui, row.theme, self.theme_name.label());
                    }
                    DIAGNOSTICS => {
                        if ui_kit::settings::action(row.ui, "Open…").clicked() {
                            self.show_diagnostics = true;
                        }
                    }
                    LOG_FOLDER => {
                        let logs = self.data_dir.join("logs").display().to_string();
                        if ui_kit::settings::action(row.ui, "Copy path").clicked() {
                            row.ui.ctx().copy_text(logs.clone());
                        }
                        row.note(logs);
                    }
                    ui_kit::user_guide::ROW => {
                        if let Some(err) = self.user_guide.settings_row(row) {
                            problems.push(err);
                        }
                    }
                    _ => {
                        if let Some(err) = self.graphics.settings_row(row) {
                            problems.push(err);
                        }
                    }
                })
        });
        for err in &problems {
            self.errors.push(err);
        }
        self.show_settings = open;
        if change_default {
            self.pick_any_folder(ctx, "Save new projects in", Picked::DefaultFolder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_its_own_id_and_opens() {
        assert!(ui_kit::settings::repeated_ids(CATEGORIES).is_empty());
        let mut view = ui_kit::settings::SettingsView::default();
        for id in [
            CHANNEL,
            NEW_PROJECTS,
            "graphics-system",
            LOG_FOLDER,
            "updates",
            "help",
        ] {
            assert!(view.open_at(CATEGORIES, id), "{id}");
        }
        assert!(
            ui_kit::settings::search(CATEGORIES, "beta")
                .iter()
                .any(|p| p.setting(CATEGORIES).is_some_and(|s| s.id == CHANNEL))
        );
        for words in ["help", "guide", "manual", "docs"] {
            assert!(
                ui_kit::settings::search(CATEGORIES, words).iter().any(|p| p
                    .setting(CATEGORIES)
                    .is_some_and(|s| s.id == ui_kit::user_guide::ROW)),
                "{words}"
            );
        }
    }
}
