//! The New project dialog: a name, a preset and a folder.

use egui::RichText;
use project::SequenceSettings;
use std::path::PathBuf;
use ui_kit::Theme;

/// One choice in the preset list.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub detail: &'static str,
    /// `None` when the settings come from media (not possible yet).
    pub settings: Option<SequenceSettings>,
}

pub const PRESETS: [Preset; 4] = [
    Preset {
        name: "Game clip",
        detail: "1920×1080 · 60 fps",
        settings: Some(SequenceSettings::GAME_CLIP),
    },
    Preset {
        name: "High frame rate",
        detail: "2560×1440 · 144 fps",
        settings: Some(SequenceSettings::HIGH_FRAME_RATE),
    },
    Preset {
        name: "Vertical short",
        detail: "1080×1920 · 60 fps",
        settings: Some(SequenceSettings::VERTICAL_SHORT),
    },
    Preset {
        name: "Match the first clip",
        detail: "Size and frame rate of the first clip you add",
        settings: None,
    },
];

/// What the person decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Create {
        name: String,
        settings: SequenceSettings,
        folder: PathBuf,
    },
    Cancel,
}

/// The open dialog's state.
#[derive(Debug, Clone)]
pub struct NewProjectDialog {
    name: String,
    preset: usize,
    folder: Option<PathBuf>,
    focused: bool,
}

impl NewProjectDialog {
    /// A dialog that saves into `folder` by default.
    pub fn new(folder: Option<PathBuf>) -> NewProjectDialog {
        NewProjectDialog {
            name: "Untitled project".into(),
            preset: 0,
            folder,
            focused: false,
        }
    }

    /// Draws the dialog. `folders` are the project folders to choose from.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, folders: &[PathBuf]) -> Option<Choice> {
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("new-project")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new("New project")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Name").color(theme.text_secondary));
            let field = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));
            if !self.focused {
                field.request_focus();
                self.focused = true;
            }
            ui.add_space(10.0);

            ui.label(RichText::new("Preset").color(theme.text_secondary));
            for (i, preset) in PRESETS.iter().enumerate() {
                ui.add_enabled_ui(preset.settings.is_some(), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.preset, i, preset.name)
                            .on_disabled_hover_text("Available once importing clips arrives.");
                        ui.label(RichText::new(preset.detail).size(11.5).color(theme.muted));
                    });
                });
            }
            ui.add_space(10.0);

            ui.label(RichText::new("Save in").color(theme.text_secondary));
            let shown = self.folder.as_ref().map_or_else(
                || "Choose a project folder".to_owned(),
                |f| f.display().to_string(),
            );
            egui::ComboBox::from_id_salt("new-project-folder")
                .width(ui.available_width())
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for folder in folders {
                        ui.selectable_value(
                            &mut self.folder,
                            Some(folder.clone()),
                            folder.display().to_string(),
                        );
                    }
                });
            ui.add_space(14.0);

            let ready = self.folder.is_some()
                && !self.name.trim().is_empty()
                && PRESETS[self.preset].settings.is_some();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let create = ui
                        .add_enabled_ui(ready, |ui| ui_kit::accent_button(ui, theme, "Create"))
                        .inner;
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (create.clicked() || (enter && ready))
                        && let (Some(folder), Some(settings)) = (&self.folder, PRESETS[self.preset].settings)
                    {
                        choice = Some(Choice::Create {
                            name: self.name.trim().to_owned(),
                            settings,
                            folder: folder.clone(),
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(Choice::Cancel);
                    }
                });
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(Choice::Cancel);
        }
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_the_plan() {
        let names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "Game clip",
                "High frame rate",
                "Vertical short",
                "Match the first clip"
            ]
        );
        assert_eq!(PRESETS[1].settings.unwrap().frame_rate.nominal_fps(), 144);
        assert!(PRESETS[3].settings.is_none(), "needs media first");
    }
}
