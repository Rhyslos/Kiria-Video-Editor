//! The Mask tool: a layer limited to a shape on the picture (an ellipse
//! or a rectangle) with a soft edge, or to everything outside it. Its
//! place and size are sliders here, and the shape is drawn over the
//! picture, where dragging it moves it.

use project::grade::ONE;
use project::{Mask, MaskShape};
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

/// Draws the tool for one layer's mask; says whether a slider is held.
pub fn ui(ui: &mut egui::Ui, theme: &Theme, mask: &mut Option<Mask>) -> bool {
    let mut busy = false;
    ui.horizontal(|ui| {
        let mut on = mask.is_some();
        if ui
            .checkbox(&mut on, "Only in a shape")
            .on_hover_text("This layer changes only the picture inside the shape (or outside it)")
            .changed()
        {
            *mask = on.then_some(Mask::MIDDLE);
        }
        if let Some(m) = mask.as_mut() {
            ui.separator();
            ui.selectable_value(&mut m.shape, MaskShape::Ellipse, "Ellipse");
            ui.selectable_value(&mut m.shape, MaskShape::Rectangle, "Rectangle");
            ui.separator();
            ui.checkbox(&mut m.invert, "Outside it")
                .on_hover_text("Change everything but the shape, for a vignette or the background");
            ui.label(
                egui::RichText::new("Drag the shape on the picture to move it")
                    .color(theme.muted)
                    .size(12.0),
            );
        }
    });
    let Some(m) = mask.as_mut() else {
        ui.label(egui::RichText::new("The layer reaches the whole picture.").color(theme.muted));
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
            egui::RichText::new("Place, in % of the picture")
                .small()
                .strong()
                .color(theme.muted),
        );
        row(ui, "Across", &mut m.x, 100.0, number((0.0, 100.0), "", 50.0));
        row(ui, "Down", &mut m.y, 100.0, number((0.0, 100.0), "", 50.0));
        row(ui, "Turn", &mut m.turn, 1.0, number((-180.0, 180.0), "°", 0.0));
        let ui = &mut columns[1];
        ui.label(
            egui::RichText::new("Size, in % of the picture")
                .small()
                .strong()
                .color(theme.muted),
        );
        row(ui, "Width", &mut m.width, 100.0, number((0.0, 100.0), "", 50.0));
        row(ui, "Height", &mut m.height, 100.0, number((0.0, 100.0), "", 60.0));
        row(
            ui,
            "Feather",
            &mut m.feather,
            100.0,
            number((0.0, 100.0), "", 30.0),
        );
    });
    busy
}

/// The mask's outline over the picture `frame` (where the clip fills it),
/// as points on screen; the feather's outer edge as a second line.
pub fn outline(mask: &Mask, frame: egui::Rect) -> [Vec<egui::Pos2>; 2] {
    let one = f64::from(ONE);
    let share = |v: i32| (f64::from(v) / one) as f32;
    let middle = frame.lerp_inside(egui::vec2(share(mask.x), share(mask.y)));
    let turn = (f64::from(mask.turn) / 1_000_000.0).to_radians() as f32;
    let (sin, cos) = turn.sin_cos();
    let along = |half: egui::Vec2, grow: f32| -> Vec<egui::Pos2> {
        let half = half * grow;
        let at = |x: f32, y: f32| middle + egui::vec2(cos * x - sin * y, sin * x + cos * y);
        match mask.shape {
            MaskShape::Ellipse => (0..=64)
                .map(|i| {
                    let a = i as f32 / 64.0 * std::f32::consts::TAU;
                    at(a.cos() * half.x, a.sin() * half.y)
                })
                .collect(),
            MaskShape::Rectangle => [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)]
                .iter()
                .map(|(x, y)| at(x * half.x, y * half.y))
                .collect(),
        }
    };
    let half = egui::vec2(
        share(mask.width) * frame.width() / 2.0,
        share(mask.height) * frame.height() / 2.0,
    );
    // The grade pass fades from half the feather inside to half outside.
    let feather = share(mask.feather);
    [along(half, 1.0), along(half, 1.0 + feather / 2.0)]
}

/// `mask` moved by `by` on screen over the picture `frame`.
pub fn dragged(mask: &Mask, frame: egui::Rect, by: egui::Vec2) -> Mask {
    let one = f64::from(ONE);
    let moved = |v: i32, by: f32, size: f32| {
        v.saturating_add((f64::from(by / size.max(1.0)) * one).round() as i32)
            .clamp(0, ONE)
    };
    Mask {
        x: moved(mask.x, by.x, frame.width()),
        y: moved(mask.y, by.y, frame.height()),
        ..*mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_sits_where_the_mask_is_and_moves_with_a_drag() {
        let frame = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        let rectangle = Mask {
            shape: MaskShape::Rectangle,
            ..Mask::MIDDLE
        };
        let [edge, _] = outline(&rectangle, frame);
        // Half the width and 60 % of the height, in the middle.
        assert_eq!(edge[0], egui::pos2(200.0, 90.0));
        assert_eq!(edge[2], egui::pos2(400.0, 210.0));
        let moved = dragged(&rectangle, frame, egui::vec2(100.0, -20.0));
        assert_eq!((moved.x, moved.y), (750_000, 400_000));
        let far = dragged(&rectangle, frame, egui::vec2(1000.0, 0.0));
        assert_eq!(far.x, ONE, "kept on the picture");
    }

    #[test]
    fn dragging_a_mask_with_values_at_the_ends_of_the_numbers_neither_wraps_nor_panics() {
        let frame = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 200.0));
        // Values no slider makes, as a file or a paste could hold them.
        let odd = Mask {
            x: i32::MAX,
            y: i32::MIN,
            ..Mask::MIDDLE
        };
        let moved = dragged(&odd, frame, egui::vec2(100.0, -100.0));
        assert_eq!((moved.x, moved.y), (ONE, 0), "held on the picture");
        let back = dragged(&odd, frame, egui::vec2(-1.0e9, 1.0e9));
        assert!((0..=ONE).contains(&back.x) && (0..=ONE).contains(&back.y));
    }
}
