//! The controls of a Settings row, one kind per kind of choice: a toggle
//! switch for on and off, a segmented control for two or three choices, a
//! dropdown for more, a slider with its value in a box for numbers (with
//! an Auto switch in front when the program can choose), a button for an
//! action, and a quiet value for what can only be read.
//!
//! They are added to the row's control column, which is laid out from the
//! right: the first thing added is the rightmost. Every one of them can be
//! reached with Tab and worked with Space or Enter, and shows a ring when
//! it has the keyboard.

use crate::Theme;
use egui::{
    Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

/// Height of a toggle switch.
const TOGGLE: egui::Vec2 = vec2(34.0, 18.0);
/// Height of the segmented control and the dropdown.
pub const CONTROL_HEIGHT: f32 = 26.0;
/// Width of a dropdown.
pub const DROPDOWN_WIDTH: f32 = 230.0;
/// Width of a slider (its value box comes beside it).
pub const SLIDER_WIDTH: f32 = 150.0;

/// A ring around a control that has the keyboard.
fn focus_ring(ui: &Ui, theme: &Theme, response: &Response, rect: Rect, radius: f32) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            radius + 2.0,
            Stroke::new(1.5, theme.accent),
            StrokeKind::Outside,
        );
    }
}

/// An on/off switch. Returns its response, changed when it was flipped.
pub fn toggle(ui: &mut Ui, theme: &Theme, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(TOGGLE, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = rect.height() / 2.0;
        let enabled = ui.is_enabled();
        let (track, edge, knob) = if *on {
            (theme.accent, theme.accent, theme.accent_ink)
        } else {
            (theme.bg, theme.line_strong, theme.text_secondary)
        };
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
        let painter = ui.painter();
        painter.rect(
            rect,
            radius,
            fade(track),
            Stroke::new(1.0, fade(edge)),
            StrokeKind::Inside,
        );
        let hovered = response.hovered() && enabled;
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob_radius = radius - if hovered { 2.5 } else { 3.0 };
        painter.circle_filled(pos2(x, rect.center().y), knob_radius, fade(knob));
        focus_ring(ui, theme, &response, rect, radius);
    }
    response
}

/// Two or three choices side by side, the chosen one lit. `options` are
/// the values with their (short) words. Returns the union of the
/// segments' responses, changed when another was chosen.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    theme: &Theme,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let font = FontId::proportional(12.5);
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, words)| {
            ui.painter()
                .layout_no_wrap((*words).to_owned(), font.clone(), theme.text)
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 24.0).collect();
    let total = vec2(widths.iter().sum::<f32>() + 4.0, CONTROL_HEIGHT);
    let (outer, _) = ui.allocate_exact_size(total, Sense::hover());
    let enabled = ui.is_enabled();
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    ui.painter().rect(
        outer,
        7.0,
        theme.bg,
        Stroke::new(1.0, fade(theme.line_strong)),
        StrokeKind::Inside,
    );
    let mut x = outer.left() + 2.0;
    let mut union: Option<Response> = None;
    let mut changed = false;
    for (i, ((option, _), galley)) in options.iter().zip(galleys).enumerate() {
        let rect = Rect::from_min_size(pos2(x, outer.top() + 2.0), vec2(widths[i], CONTROL_HEIGHT - 4.0));
        x += widths[i];
        let id = ui.scope_id().with(("segment", i));
        let response = ui.interact(rect, id, Sense::click());
        let chosen = *value == *option;
        if response.clicked() && !chosen {
            *value = *option;
            changed = true;
        }
        let chosen = *value == *option;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, chosen, galley.text())
        });
        let painter = ui.painter();
        if chosen {
            painter.rect(
                rect,
                5.0,
                fade(theme.active),
                Stroke::new(1.0, fade(theme.line_strong)),
                StrokeKind::Inside,
            );
        } else if response.hovered() && enabled {
            painter.rect_filled(rect, 5.0, theme.raised);
        }
        let colour = match (chosen, enabled) {
            (true, true) => theme.text,
            (false, true) => theme.text_secondary,
            _ => theme.faint,
        };
        painter.galley(rect.center() - galley.size() / 2.0, galley, colour);
        focus_ring(ui, theme, &response, rect, 5.0);
        union = Some(match union {
            Some(u) => u.union(response),
            None => response,
        });
    }
    let mut response =
        union.unwrap_or_else(|| ui.interact(outer, ui.scope_id().with("segments"), Sense::hover()));
    if changed {
        response.mark_changed();
    }
    response
}

/// A dropdown for more than three choices. Returns whether another was chosen.
pub fn dropdown<T: PartialEq + Copy>(
    ui: &mut Ui,
    id_salt: &str,
    value: &mut T,
    options: &[T],
    words: impl Fn(T) -> &'static str,
) -> bool {
    let before = *value;
    egui::ComboBox::from_id_salt(("settings-dropdown", id_salt))
        .width(DROPDOWN_WIDTH)
        .height(320.0)
        .selected_text(words(*value))
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(value, *option, words(*option));
            }
        });
    *value != before
}

/// A number: a slider and its value in a box (type to set it exactly),
/// `unit` after the value. Returns whether it changed.
pub fn number<T: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    unit: &str,
) -> bool {
    // Laid out from the right: the box first, then the slider before it.
    let boxed = ui.add_sized(
        vec2(74.0, CONTROL_HEIGHT - 2.0),
        egui::DragValue::new(value)
            .range(range.clone())
            .suffix(unit)
            .speed(0.25),
    );
    ui.spacing_mut().slider_width = SLIDER_WIDTH;
    let slid = ui.add(egui::Slider::new(value, range).show_value(false));
    boxed.changed() || slid.changed()
}

/// A number the program can choose by itself: an Auto switch, and the
/// slider with its value box beside it while Auto is off. `auto` turned off
/// starts the value at `start`. Returns whether anything changed.
pub fn auto_number<T: egui::emath::Numeric>(
    ui: &mut Ui,
    theme: &Theme,
    value: &mut Option<T>,
    start: T,
    range: std::ops::RangeInclusive<T>,
    unit: &str,
) -> bool {
    let mut auto = value.is_none();
    let flipped = toggle(ui, theme, &mut auto)
        .on_hover_text("On: chosen for this machine. Off: set it yourself.")
        .changed();
    ui.label(egui::RichText::new("Auto").size(12.5).color(theme.text_secondary));
    if flipped {
        *value = if auto { None } else { Some(start) };
    }
    let mut changed = flipped;
    if let Some(number_now) = value {
        ui.add_space(10.0);
        changed |= number(ui, number_now, range, unit);
    }
    changed
}

/// A button for an action (such as "Clear all caches…"). Returns its response.
pub fn action(ui: &mut Ui, words: &str) -> Response {
    ui.add(egui::Button::new(egui::RichText::new(words).size(12.5)).min_size(vec2(0.0, CONTROL_HEIGHT)))
}

/// What can only be read, such as the space in use, in quiet text.
pub fn value(ui: &mut Ui, theme: &Theme, words: &str) -> Response {
    ui.label(egui::RichText::new(words).size(12.5).color(theme.text_secondary))
}

/// The small "Restart" badge after a label: a change applies the next time
/// the program starts. Painted at `left_centre`; returns its rectangle.
pub(super) fn restart_badge(ui: &Ui, theme: &Theme, left_centre: egui::Pos2) -> Rect {
    let galley = ui
        .painter()
        .layout_no_wrap("Restart".to_owned(), FontId::proportional(10.5), theme.muted);
    let rect = Rect::from_min_size(
        pos2(left_centre.x, left_centre.y - 8.5),
        vec2(galley.size().x + 12.0, 17.0),
    );
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(8),
        theme.raised,
        Stroke::new(1.0, theme.card_edge),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, theme.muted);
    rect
}

/// The words the badge's tooltip says.
pub const RESTART_TIP: &str = "A change applies the next time the app starts.";

/// The × that closes Settings.
pub(super) fn close_button(ui: &mut Ui, theme: &Theme) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme.active);
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "×",
        FontId::proportional(18.0),
        if response.hovered() {
            theme.text
        } else {
            theme.muted
        },
    );
    focus_ring(ui, theme, &response, rect, 6.0);
    response.on_hover_text("Close (Escape)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws `add` in a fresh window frame after `events`, twice over so
    /// clicks land on what the first frame laid out.
    fn frames(ctx: &egui::Context, events: Vec<egui::Event>, mut add: impl FnMut(&mut Ui)) {
        for input in [Vec::new(), events] {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 200.0))),
                events: input,
                ..Default::default()
            };
            // Nothing draws these frames.
            ctx.run_ui(raw, |ui| add(ui)).drop_without_applying_deltas();
        }
    }

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), press(true), press(false)]
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Mode {
        Off,
        On,
        Auto,
    }

    #[test]
    fn a_click_on_a_segment_chooses_it_and_says_so() {
        let ctx = egui::Context::default();
        let theme = Theme::ASH;
        let options = [(Mode::Off, "Off"), (Mode::On, "On"), (Mode::Auto, "Auto")];
        let mut mode = Mode::Off;
        let mut rect = Rect::NOTHING;
        frames(&ctx, Vec::new(), |ui| {
            rect = segmented(ui, &theme, &mut mode, &options).rect;
        });
        assert!(rect.is_positive());
        // The last third is Auto.
        let auto = pos2(rect.right() - 12.0, rect.center().y);
        let mut changed = false;
        frames(&ctx, click_at(auto), |ui| {
            changed |= segmented(ui, &theme, &mut mode, &options).changed();
        });
        assert_eq!(mode, Mode::Auto);
        assert!(changed);
        // Clicking the chosen one again changes nothing.
        let mut again = false;
        frames(&ctx, click_at(auto), |ui| {
            again |= segmented(ui, &theme, &mut mode, &options).changed();
        });
        assert_eq!(mode, Mode::Auto);
        assert!(!again);
        // Disabled, a click does nothing.
        let first = pos2(rect.left() + 8.0, rect.center().y);
        frames(&ctx, click_at(first), |ui| {
            ui.disable();
            segmented(ui, &theme, &mut mode, &options);
        });
        assert_eq!(mode, Mode::Auto);
    }

    #[test]
    fn a_toggle_flips_on_a_click() {
        let ctx = egui::Context::default();
        let theme = Theme::CARBON;
        let mut on = false;
        let mut rect = Rect::NOTHING;
        frames(&ctx, Vec::new(), |ui| rect = toggle(ui, &theme, &mut on).rect);
        let mut changed = false;
        frames(&ctx, click_at(rect.center()), |ui| {
            changed |= toggle(ui, &theme, &mut on).changed();
        });
        assert!(on && changed);
    }

    #[test]
    fn auto_hands_the_number_back_to_the_program() {
        let ctx = egui::Context::default();
        let theme = Theme::ASH;
        let mut value: Option<u64> = None;
        let mut rect = Rect::NOTHING;
        frames(&ctx, Vec::new(), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                auto_number(ui, &theme, &mut value, 8, 1..=16, "");
                rect = ui.min_rect();
            });
        });
        // The switch is the rightmost thing.
        let switch = pos2(rect.right() - TOGGLE.x / 2.0, rect.center().y);
        frames(&ctx, click_at(switch), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                auto_number(ui, &theme, &mut value, 8, 1..=16, "");
            });
        });
        assert_eq!(value, Some(8), "off: set by hand, from the start value");
    }
}
