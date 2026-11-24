//! The Looks tool: a look put on the clip in a click, as a layer of its
//! own on top of its grade at the strength chosen, from those the app
//! comes with and the person's own (`project::looks`); and the layer
//! shown kept as a look of their own, named in a small dialog.

use project::looks::{Look, Looks};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// What the tool was asked for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    /// Put this look on the clip.
    Put(Box<Look>),
    /// Keep the layer shown as a look.
    SaveLayer,
    /// Stop keeping the person's look of this name.
    Remove(String),
}

/// The strength slider: 0 to 100 %.
const STRENGTH: Number = Number {
    range: (0.0, 100.0),
    per_point: 0.5,
    decimals: 0,
    suffix: " %",
    usual: 100.0,
};

/// Draws the tool. `strength` is 0 to 1.
pub fn ui(
    ui: &mut egui::Ui,
    theme: &Theme,
    own: &Looks,
    strength: &mut f64,
    has_layer: bool,
) -> Option<Picked> {
    let mut picked = None;
    let mut percent = *strength * 100.0;
    inspector::slider(ui, theme, "Strength", &mut percent, STRENGTH);
    *strength = (percent / 100.0).clamp(0.0, 1.0);
    ui.label(
        egui::RichText::new("A look goes on as a layer of its own, on top of the grade: change or switch it off in the layer list.")
            .color(theme.muted)
            .size(12.0),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for look in project::looks::built_in() {
            if ui.button(&look.name).clicked() {
                picked = Some(Picked::Put(Box::new(look)));
            }
        }
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Your looks").color(theme.text_secondary));
    if own.looks.is_empty() {
        ui.label(
            egui::RichText::new(
                "None yet: grade a layer the way you like, then keep it here for every project.",
            )
            .color(theme.muted)
            .size(12.0),
        );
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for look in &own.looks {
            let button = ui
                .button(&look.name)
                .on_hover_text("Click to put it on; right-click to remove it");
            if button.clicked() {
                picked = Some(Picked::Put(Box::new(look.clone())));
            }
            button.context_menu(|ui| {
                if ui.button("Remove this look").clicked() {
                    picked = Some(Picked::Remove(look.name.clone()));
                    ui.close();
                }
            });
        }
    });
    ui.add_space(4.0);
    if ui
        .add_enabled(has_layer, egui::Button::new("Keep this layer as a look…"))
        .on_hover_text("Its colour (not its range or mask), for every project")
        .clicked()
    {
        picked = Some(Picked::SaveLayer);
    }
    picked
}
