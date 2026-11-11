//! The small window for one timeline label: its name, its colour, or
//! removing it. Opened by double-clicking the label.

use crate::timeline_panel::label_colour;
use project::{LabelColor, LabelId, TimelineLabel};
use ui_kit::Theme;

/// The label being edited, with the changes so far.
pub struct LabelEditor {
    id: LabelId,
    name: String,
    color: LabelColor,
}

/// What the window asked for.
pub enum Asked {
    Nothing,
    Close,
    Save {
        id: LabelId,
        name: String,
        color: LabelColor,
    },
    Remove(LabelId),
}

impl LabelEditor {
    pub fn new(label: &TimelineLabel) -> LabelEditor {
        LabelEditor {
            id: label.id,
            name: label.name.clone(),
            color: label.color,
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, theme: &Theme) -> Asked {
        let mut asked = Asked::Nothing;
        let mut open = true;
        egui::Window::new("Timeline label")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("Name"));
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    asked = self.save();
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for colour in LabelColor::ALL {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                        let r = r.on_hover_text(format!("{colour:?}"));
                        ui.painter()
                            .circle_filled(rect.center(), 8.0, label_colour(colour));
                        if colour == self.color {
                            ui.painter().circle_stroke(
                                rect.center(),
                                9.5,
                                egui::Stroke::new(2.0, theme.text),
                            );
                        }
                        if r.clicked() {
                            self.color = colour;
                        }
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui_kit::accent_button(ui, theme, "Done").clicked() {
                        asked = self.save();
                    }
                    if ui.button("Remove label").clicked() {
                        asked = Asked::Remove(self.id);
                    }
                });
            });
        if !open {
            asked = Asked::Close;
        }
        asked
    }

    fn save(&self) -> Asked {
        Asked::Save {
            id: self.id,
            name: self.name.clone(),
            color: self.color,
        }
    }
}
