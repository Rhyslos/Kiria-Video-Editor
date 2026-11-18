//! The Color range tool: a layer limited to the colours in a range of
//! hue, saturation and lightness, with soft edges, so a correction can
//! reach skin, a sky or one coloured object and leave the rest. "Pick
//! from the picture" puts the range round what is clicked.

use project::ColorRange;
use project::grade::ONE;
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// A slider's settings for a number from `range.0` to `range.1`.
fn number(range: (f64, f64), suffix: &'static str, usual: f64) -> Number {
    Number {
        range,
        per_point: (range.1 - range.0) / 300.0,
        decimals: 0,
        suffix,
        usual,
    }
}

/// Hue (0 to 1 round the wheel), saturation and lightness of `light`, on
/// display-like values, as the grade pass works them out.
pub fn hsl(light: [f32; 3]) -> (f64, f64, f64) {
    let v = light.map(|c| f64::from(c.clamp(0.0, 1.0)).powf(1.0 / 2.4));
    let most = v[0].max(v[1]).max(v[2]);
    let least = v[0].min(v[1]).min(v[2]);
    let chroma = most - least;
    let lightness = (most + least) / 2.0;
    if chroma < 1e-5 {
        return (0.0, 0.0, lightness);
    }
    let hue = if most == v[0] {
        ((v[1] - v[2]) / chroma / 6.0).rem_euclid(1.0)
    } else if most == v[1] {
        ((v[2] - v[0]) / chroma + 2.0) / 6.0
    } else {
        ((v[0] - v[1]) / chroma + 4.0) / 6.0
    };
    let saturation = chroma / (1.0 - (2.0 * lightness - 1.0).abs()).max(1e-5);
    (hue, saturation.min(1.0), lightness)
}

/// A range round the colour of `light`: its hue give or take a twentieth
/// of the wheel, its saturation and lightness give or take a quarter.
pub fn around(light: [f32; 3]) -> ColorRange {
    let (hue, saturation, lightness) = hsl(light);
    let one = f64::from(ONE);
    let share = |v: f64| (v.clamp(0.0, 1.0) * one).round() as i32;
    ColorRange {
        hue: share(hue),
        hue_width: share(0.05),
        saturation_low: share(saturation - 0.25),
        saturation_high: share(saturation + 0.25),
        lightness_low: share(lightness - 0.25),
        lightness_high: share(lightness + 0.25),
        softness: share(0.3),
    }
}

/// Draws the tool for one layer's range; says whether a slider is held.
/// `picking` is "Pick from the picture" (the next click on it picks).
pub fn ui(ui: &mut egui::Ui, theme: &Theme, range: &mut Option<ColorRange>, picking: &mut bool) -> bool {
    let mut busy = false;
    ui.horizontal(|ui| {
        let mut on = range.is_some();
        if ui
            .checkbox(&mut on, "Only these colours")
            .on_hover_text(
                "This layer changes only the colours in the range below, the rest is left as it is",
            )
            .changed()
        {
            *range = on.then_some(ColorRange::SKIN);
        }
        ui.separator();
        if ui
            .toggle_value(picking, "Pick from the picture")
            .on_hover_text("Then click a colour on the picture: the range is put round it")
            .clicked()
            && *picking
            && range.is_none()
        {
            *range = Some(ColorRange::SKIN);
        }
        if ui
            .button("Skin")
            .on_hover_text("Orange hues with some colour, not too dark or bright")
            .clicked()
        {
            *range = Some(ColorRange::SKIN);
        }
        ui.label(
            egui::RichText::new("Switch the layer off and on to see what it reaches")
                .color(theme.muted)
                .size(12.0),
        );
    });
    let Some(r) = range.as_mut() else {
        ui.label(egui::RichText::new("The layer reaches every colour.").color(theme.muted));
        return false;
    };
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
            egui::RichText::new("Which colours")
                .small()
                .strong()
                .color(theme.muted),
        );
        row(ui, "Hue", &mut r.hue, 360.0, number((0.0, 360.0), "°", 25.0));
        row(
            ui,
            "Either side",
            &mut r.hue_width,
            360.0,
            number((0.0, 180.0), "°", 22.0),
        );
        row(
            ui,
            "Softness",
            &mut r.softness,
            100.0,
            number((0.0, 100.0), " %", 30.0),
        );
        let ui = &mut columns[1];
        ui.label(
            egui::RichText::new("Saturation and lightness, in %")
                .small()
                .strong()
                .color(theme.muted),
        );
        row(
            ui,
            "Saturation from",
            &mut r.saturation_low,
            100.0,
            number((0.0, 100.0), "", 15.0),
        );
        row(
            ui,
            "Saturation to",
            &mut r.saturation_high,
            100.0,
            number((0.0, 100.0), "", 75.0),
        );
        row(
            ui,
            "Lightness from",
            &mut r.lightness_low,
            100.0,
            number((0.0, 100.0), "", 20.0),
        );
        row(
            ui,
            "Lightness to",
            &mut r.lightness_high,
            100.0,
            number((0.0, 100.0), "", 90.0),
        );
    });
    // From is never past to.
    r.saturation_high = r.saturation_high.max(r.saturation_low);
    r.lightness_high = r.lightness_high.max(r.lightness_low);
    busy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_read_as_the_grade_pass_reads_them() {
        let (hue, saturation, lightness) = hsl([1.0, 0.0, 0.0]);
        assert!(hue.abs() < 1e-9 && (saturation - 1.0).abs() < 1e-9 && (lightness - 0.5).abs() < 1e-9);
        let (hue, _, _) = hsl([0.0, 0.0, 1.0]);
        assert!((hue - 2.0 / 3.0).abs() < 1e-9, "blue is two thirds round");
        let (_, saturation, lightness) = hsl([0.18, 0.18, 0.18]);
        assert_eq!(saturation, 0.0, "grey has no colour");
        assert!((lightness - 0.18f64.powf(1.0 / 2.4)).abs() < 1e-6);
    }

    #[test]
    fn a_picked_colour_is_in_the_middle_of_its_range() {
        let range = around([0.6, 0.3, 0.2]);
        let (hue, saturation, lightness) = hsl([0.6, 0.3, 0.2]);
        let share = |v: i32| f64::from(v) / f64::from(ONE);
        assert!((share(range.hue) - hue).abs() < 1e-5);
        assert!(share(range.saturation_low) < saturation && saturation < share(range.saturation_high));
        assert!(share(range.lightness_low) < lightness && lightness < share(range.lightness_high));
        assert!(range.check().is_ok());
    }
}
