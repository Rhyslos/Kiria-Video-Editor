//! The Basic tool: white balance (temperature and tint), then the tone:
//! exposure, contrast and its pivot, saturation, highlights and shadows.
//! Each a slider with its number; a double-click puts one back.

use project::Primary;
use project::grade::ONE;
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// A slider's settings for a number shown `scale` times its share of one.
fn number(range: (f64, f64), decimals: usize, suffix: &'static str, usual: f64) -> Number {
    Number {
        range,
        per_point: (range.1 - range.0) / 300.0,
        decimals,
        suffix,
        usual,
    }
}

/// What the tool asked for besides its sliders.
#[derive(Debug, Default)]
pub struct Basic {
    /// A slider is still held.
    pub busy: bool,
    /// Auto white balance was clicked.
    pub auto: bool,
}

/// Draws the tool. `strength` is Auto's (0 to 1, kept by the page);
/// `working` says Auto is looking at the clip now; `picking` is the
/// "Pick white" button's state (the next click on the picture picks).
pub fn ui(
    ui: &mut egui::Ui,
    theme: &Theme,
    p: &mut Primary,
    strength: &mut f64,
    working: bool,
    picking: &mut bool,
) -> Basic {
    let mut auto = false;
    let mut busy = false;
    let one = f64::from(ONE);
    let mut row = |ui: &mut egui::Ui, label: &str, value: &mut i32, scale: f64, how: Number| {
        let mut shown = f64::from(*value) / one * scale;
        let touched = inspector::slider(ui, theme, label, &mut shown, how);
        busy |= touched.busy;
        if touched.changed {
            *value = (shown / scale * one).round() as i32;
        }
    };
    ui.columns(2, |columns| {
        let ui = &mut columns[0];
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("White balance").small().strong().color(theme.muted));
            let label = if working { "Looking…" } else { "Auto" };
            let button = ui.add_enabled(!working, egui::Button::new(label).small());
            auto = button
                .on_hover_text("Finds the light's colour from frames across the clip and sets Temperature and Tint to take it back to neutral. Never runs by itself.")
                .clicked();
            let mut percent = *strength * 100.0;
            if ui
                .add(egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(1.0).suffix(" %"))
                .on_hover_text("How much of Auto's correction is used")
                .changed()
            {
                *strength = percent / 100.0;
            }
            ui.toggle_value(picking, "Pick white").on_hover_text(
                "Then click something on the picture that should be white or grey: Temperature and Tint change so it is",
            );
        });
        row(
            ui,
            "Temperature",
            &mut p.temperature,
            100.0,
            number((-100.0, 100.0), 0, "", 0.0),
        );
        row(
            ui,
            "Tint",
            &mut p.tint,
            100.0,
            number((-100.0, 100.0), 0, "", 0.0),
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Light, in stops").small().strong().color(theme.muted));
        row(
            ui,
            "Exposure",
            &mut p.exposure,
            1.0,
            number((-4.0, 4.0), 2, "", 0.0),
        );
        row(
            ui,
            "Highlights",
            &mut p.highlights,
            1.0,
            number((-2.0, 2.0), 2, "", 0.0),
        );
        row(
            ui,
            "Shadows",
            &mut p.shadows,
            1.0,
            number((-2.0, 2.0), 2, "", 0.0),
        );
        let ui = &mut columns[1];
        ui.label(egui::RichText::new("Tone").small().strong().color(theme.muted));
        row(
            ui,
            "Contrast",
            &mut p.contrast,
            100.0,
            number((-100.0, 100.0), 0, "", 0.0),
        );
        row(
            ui,
            "Pivot",
            &mut p.pivot,
            100.0,
            number((1.0, 100.0), 0, " %", 18.0),
        );
        row(
            ui,
            "Saturation",
            &mut p.saturation,
            100.0,
            number((0.0, 200.0), 0, " %", 100.0),
        );
    });
    Basic { busy, auto }
}
