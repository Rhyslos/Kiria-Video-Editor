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

/// Draws the tool; says whether a slider is still held.
pub fn ui(ui: &mut egui::Ui, theme: &Theme, p: &mut Primary) -> bool {
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
        ui.label(
            egui::RichText::new("White balance")
                .small()
                .strong()
                .color(theme.muted),
        );
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
        ui.label(egui::RichText::new("Light").small().strong().color(theme.muted));
        row(
            ui,
            "Exposure (stops)",
            &mut p.exposure,
            1.0,
            number((-4.0, 4.0), 2, "", 0.0),
        );
        row(
            ui,
            "Highlights (stops)",
            &mut p.highlights,
            1.0,
            number((-2.0, 2.0), 2, "", 0.0),
        );
        row(
            ui,
            "Shadows (stops)",
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
    busy
}
