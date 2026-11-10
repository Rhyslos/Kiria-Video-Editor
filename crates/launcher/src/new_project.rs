//! The New project dialog: a name, the timeline settings (Automatic or a template) and a folder.

use egui::RichText;
use project::SequenceSettings;
use std::path::PathBuf;
use ui_kit::Theme;

/// One choice in the settings list.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub detail: &'static str,
    /// `None` for Automatic: the first video imported decides.
    pub settings: Option<SequenceSettings>,
}

/// Automatic first (the default), then the optional templates.
pub const PRESETS: [Preset; 4] = [
    Preset {
        name: "Automatic",
        detail: "Size, frame rate and sound rate from the first video you import",
        settings: None,
    },
    Preset {
        name: SequenceSettings::TEMPLATES[0].0,
        detail: "1920×1080 · 60 fps",
        settings: Some(SequenceSettings::TEMPLATES[0].1),
    },
    Preset {
        name: SequenceSettings::TEMPLATES[1].0,
        detail: "2560×1440 · 144 fps",
        settings: Some(SequenceSettings::TEMPLATES[1].1),
    },
    Preset {
        name: SequenceSettings::TEMPLATES[2].0,
        detail: "1080×1920 · 60 fps",
        settings: Some(SequenceSettings::TEMPLATES[2].1),
    },
];

/// What the person decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Create {
        name: String,
        /// `None` for Automatic.
        settings: Option<SequenceSettings>,
        folder: PathBuf,
        /// Use this folder for new projects from now on.
        remember: bool,
    },
    /// Pick another folder with the system's folder dialog.
    Browse,
    Cancel,
}

/// The open dialog's state.
#[derive(Debug, Clone)]
pub struct NewProjectDialog {
    name: String,
    preset: usize,
    folder: Option<PathBuf>,
    /// The folder new projects normally go to, to tell whether `folder` differs.
    default_folder: Option<PathBuf>,
    remember: bool,
    focused: bool,
}

impl NewProjectDialog {
    /// A dialog that saves into `folder` at first. `default_folder` is the
    /// person's chosen folder for new projects.
    pub fn new(folder: Option<PathBuf>, default_folder: Option<PathBuf>) -> NewProjectDialog {
        NewProjectDialog {
            name: "Untitled project".into(),
            preset: 0,
            folder,
            default_folder,
            remember: false,
            focused: false,
        }
    }

    /// Sets the folder after Browse… picked one.
    pub fn set_folder(&mut self, folder: PathBuf) {
        self.folder = Some(folder);
    }

    /// Draws the dialog. `folders` are the project folders to choose from.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, folders: &[PathBuf]) -> Option<Choice> {
        let mut choice = None;
        let modal = ui_kit::dialog(ctx, theme, "new-project", |ui| {
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

            ui.label(RichText::new("Timeline settings").color(theme.text_secondary));
            for (i, preset) in PRESETS.iter().enumerate() {
                if i == 1 {
                    ui.label(
                        RichText::new("Or start from a template")
                            .size(11.5)
                            .color(theme.muted),
                    );
                }
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.preset, i, preset.name);
                    ui.label(RichText::new(preset.detail).size(11.5).color(theme.muted));
                });
            }
            ui.add_space(10.0);

            ui.label(RichText::new("Save in").color(theme.text_secondary));
            let shown = self.folder.as_ref().map_or_else(
                || "Choose a project folder".to_owned(),
                |f| f.display().to_string(),
            );
            ui.horizontal(|ui| {
                let browse_width = 80.0;
                egui::ComboBox::from_id_salt("new-project-folder")
                    .width(ui.available_width() - browse_width - ui.spacing().item_spacing.x)
                    .selected_text(shown)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for folder in folders {
                            ui.selectable_value(
                                &mut self.folder,
                                Some(folder.clone()),
                                folder.display().to_string(),
                            );
                        }
                    });
                if ui
                    .add_sized([browse_width, 0.0], egui::Button::new("Browse…"))
                    .on_hover_text("Save in any folder on your computer")
                    .clicked()
                {
                    choice = Some(Choice::Browse);
                }
            });
            if self.folder.is_some() && self.folder != self.default_folder {
                ui.checkbox(&mut self.remember, "Save new projects here from now on");
            }
            ui.add_space(14.0);

            let ready = self.folder.is_some() && !self.name.trim().is_empty();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let create = ui
                        .add_enabled_ui(ready, |ui| ui_kit::accent_button(ui, theme, "Create"))
                        .inner;
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (create.clicked() || (enter && ready))
                        && let Some(folder) = &self.folder
                    {
                        choice = Some(Choice::Create {
                            name: self.name.trim().to_owned(),
                            settings: PRESETS[self.preset].settings,
                            folder: folder.clone(),
                            remember: self.remember && self.folder != self.default_folder,
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
            ["Automatic", "Game clip", "High frame rate", "Vertical short"]
        );
        assert!(
            PRESETS[0].settings.is_none(),
            "Automatic is the default and has no fixed settings"
        );
        assert_eq!(PRESETS[2].settings.unwrap().frame_rate.nominal_fps(), 144);
        assert_eq!(NewProjectDialog::new(None, None).preset, 0);
    }
}
