//! The Wheels tool: Shadows, Midtones, Highlights and Overall. Each wheel's
//! puck pushes the colour towards a hue (how far out, how strongly), a
//! slider under it sets its brightness, and a double-click on the wheel
//! puts it back. Advanced draws them larger with their red, green, blue
//! and brightness as numbers to type.

use project::Wheel;
use project::grade::ONE;
use ui_kit::Theme;

/// A push towards the hue at `angle` (radians, red at the top, going
/// clockwise through yellow and green), `strength` 0 to 1 strong: red,
/// green and blue adding up to nothing, so brightness stays.
pub fn push(angle: f64, strength: f64) -> [f64; 3] {
    let third = std::f64::consts::TAU / 3.0;
    [
        strength * angle.cos(),
        strength * (angle - third).cos(),
        strength * (angle + third).cos(),
    ]
}

/// Where a wheel's puck sits for a push of red, green and blue: its angle
/// and strength, as [`push`] takes them.
pub fn puck(red: f64, green: f64, blue: f64) -> (f64, f64) {
    let x = red - (green + blue) / 2.0;
    let y = (green - blue) * 3f64.sqrt() / 2.0;
    let strength = x.hypot(y) / 1.5;
    (y.atan2(x), strength)
}

/// Draws the four wheels; says whether one is still held.
pub fn ui(ui: &mut egui::Ui, theme: &Theme, p: &mut project::Primary, advanced: bool) -> bool {
    let mut busy = false;
    let size = if advanced { 150.0 } else { 104.0 };
    let wheels: [(&str, &mut Wheel); 4] = [
        ("Shadows", &mut p.shadows_wheel),
        ("Midtones", &mut p.midtones_wheel),
        ("Highlights", &mut p.highlights_wheel),
        ("Overall", &mut p.overall_wheel),
    ];
    ui.columns(4, |columns| {
        for (column, (name, wheel)) in columns.iter_mut().zip(wheels) {
            column.vertical_centered(|ui| {
                ui.label(egui::RichText::new(name).strong());
                busy |= one(ui, theme, wheel, size, name);
                if advanced {
                    busy |= numbers(ui, wheel);
                }
            });
        }
    });
    busy
}

/// One wheel and its brightness slider.
fn one(ui: &mut egui::Ui, theme: &Theme, wheel: &mut Wheel, size: f32, name: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let centre = rect.center();
    let radius = size / 2.0 - 4.0;
    let painter = ui.painter_at(rect.expand(2.0));
    // The hue ring and the disc.
    let steps = 72;
    for i in 0..steps {
        let a0 = i as f64 / steps as f64 * std::f64::consts::TAU;
        let a1 = (i + 1) as f64 / steps as f64 * std::f64::consts::TAU;
        let [r, g, b] = push(a0, 1.0).map(|c| ((0.5 + 0.5 * c) * 255.0) as u8);
        let at = |a: f64, k: f32| centre + egui::vec2(a.sin() as f32, -(a.cos() as f32)) * radius * k;
        painter.add(egui::Shape::convex_polygon(
            vec![at(a0, 0.86), at(a0, 1.0), at(a1, 1.0), at(a1, 0.86)],
            egui::Color32::from_rgb(r, g, b),
            egui::Stroke::NONE,
        ));
    }
    painter.circle_filled(centre, radius * 0.86, theme.bg);
    painter.line_segment(
        [
            centre - egui::vec2(radius * 0.8, 0.0),
            centre + egui::vec2(radius * 0.8, 0.0),
        ],
        egui::Stroke::new(1.0, theme.line),
    );
    painter.line_segment(
        [
            centre - egui::vec2(0.0, radius * 0.8),
            centre + egui::vec2(0.0, radius * 0.8),
        ],
        egui::Stroke::new(1.0, theme.line),
    );
    let one = f64::from(ONE);
    let (angle, strength) = puck(
        f64::from(wheel.red) / one,
        f64::from(wheel.green) / one,
        f64::from(wheel.blue) / one,
    );
    let reach = f64::from(radius * 0.86);
    let to_screen = |angle: f64, strength: f64| {
        centre
            + egui::vec2(
                (angle.sin() * strength * reach) as f32,
                -(angle.cos() * strength * reach) as f32,
            )
    };
    if let Some(pointer) = response
        .interact_pointer_pos()
        .filter(|_| response.dragged() || response.clicked())
    {
        let d = pointer - centre;
        let strength = (f64::from(d.length()) / reach).min(1.0);
        let angle = f64::from(d.x).atan2(-f64::from(d.y));
        let [r, g, b] = push(angle, strength);
        wheel.red = (r * one).round() as i32;
        wheel.green = (g * one).round() as i32;
        wheel.blue = (b * one).round() as i32;
    }
    if response.double_clicked() {
        *wheel = Wheel {
            brightness: wheel.brightness,
            ..Wheel::PLAIN
        };
    }
    let at = to_screen(angle, strength);
    painter.circle_filled(at, 5.0, theme.text);
    painter.circle_stroke(at, 5.0, egui::Stroke::new(1.5, theme.bg));
    let busy = response.dragged();
    response.on_hover_text(format!(
        "Drag towards a colour to push the {} that way; double-click to put it back",
        name.to_lowercase()
    ));
    // Its brightness.
    let mut brightness = f64::from(wheel.brightness) / one * 100.0;
    let slider = ui.add(
        egui::Slider::new(&mut brightness, -100.0..=100.0)
            .show_value(false)
            .trailing_fill(false),
    );
    if slider.double_clicked() {
        brightness = 0.0;
    }
    if slider.changed() || slider.double_clicked() {
        wheel.brightness = (brightness / 100.0 * one).round() as i32;
    }
    busy || slider.dragged()
}

/// The wheel's numbers, typed (Advanced).
fn numbers(ui: &mut egui::Ui, wheel: &mut Wheel) -> bool {
    let mut busy = false;
    let one = f64::from(ONE);
    for (label, value) in [
        ("R", &mut wheel.red),
        ("G", &mut wheel.green),
        ("B", &mut wheel.blue),
        ("Y", &mut wheel.brightness),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            let mut shown = f64::from(*value) / one;
            let drag = ui.add(
                egui::DragValue::new(&mut shown)
                    .range(-1.0..=1.0)
                    .speed(0.005)
                    .max_decimals(3),
            );
            busy |= drag.dragged() || drag.has_focus();
            if drag.changed() {
                *value = (shown * one).round() as i32;
            }
        });
    }
    busy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_keeps_brightness_and_its_puck_is_where_it_was_put() {
        for angle in [0.0, 1.0, 2.5, -2.0] {
            let [r, g, b] = push(angle, 0.6);
            assert!((r + g + b).abs() < 1e-9, "adds up to nothing");
            let (back, strength) = puck(r, g, b);
            assert!((strength - 0.6).abs() < 1e-9);
            let turned = (back - angle).rem_euclid(std::f64::consts::TAU);
            assert!(
                turned < 1e-9 || (std::f64::consts::TAU - turned) < 1e-9,
                "{angle} came back as {back}"
            );
        }
        assert_eq!(push(0.0, 1.0)[0], 1.0, "red is at the top");
    }
}
