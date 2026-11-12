//! A file's own scrub bar: where in the file the preview is, the part
//! marked to add (in and out), and a click or drag to go somewhere. Under
//! the Library's small player, and over its large waveform, so a file is
//! scrubbed the same way whatever it holds.

use crate::Theme;
use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};

/// The part of a file marked to add: where it begins and ends, in seconds
/// from the file's start. Either may be unset (the file's start or end).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Marks {
    pub mark_in: Option<f64>,
    pub mark_out: Option<f64>,
}

impl Marks {
    /// The marked part of a file `length` seconds long, as (start, end).
    pub fn part(&self, length: f64) -> (f64, f64) {
        let start = self.mark_in.unwrap_or(0.0).clamp(0.0, length);
        let end = self.mark_out.unwrap_or(length).clamp(start, length);
        (start, end)
    }

    pub fn is_set(&self) -> bool {
        self.mark_in.is_some() || self.mark_out.is_some()
    }

    /// Marks in at `at`; an out mark before it goes.
    pub fn set_in(&mut self, at: f64) {
        self.mark_in = Some(at);
        if self.mark_out.is_some_and(|out| out <= at) {
            self.mark_out = None;
        }
    }

    /// Marks out at `at`; an in mark after it goes.
    pub fn set_out(&mut self, at: f64) {
        self.mark_out = Some(at);
        if self.mark_in.is_some_and(|start| start >= at) {
            self.mark_in = None;
        }
    }
}

/// How a scrub bar looks: a bar of its own, or over something else (a
/// waveform) that already shows the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Bar,
    Over,
}

/// The height of a scrub bar drawn as a bar of its own.
pub const BAR_HEIGHT: f32 = 18.0;

/// The seconds in a file `length` long at `x` across `rect`.
pub fn seconds_at(rect: Rect, length: f64, x: f32) -> f64 {
    let share = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    f64::from(share) * length
}

/// Draws the scrub bar `id_salt` over `rect` for a file `length` seconds
/// long, the preview at `at`, with `marks`. Returns where a click or drag
/// on it asks the preview to go.
#[allow(clippy::too_many_arguments)]
pub fn scrub(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    rect: Rect,
    length: f64,
    at: f64,
    marks: Marks,
    look: Look,
) -> Option<f64> {
    let id = ui.scope_id().with(("scrub", id_salt));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    let x_of = |seconds: f64| rect.left() + (seconds / length.max(1e-9)) as f32 * rect.width();
    if look == Look::Bar {
        let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 6.0));
        painter.rect_filled(track, 3.0, theme.line);
    }
    if marks.is_set() && length > 0.0 {
        let (start, end) = marks.part(length);
        let part = Rect::from_min_max(pos2(x_of(start), rect.top()), pos2(x_of(end), rect.bottom()));
        let part = if look == Look::Bar {
            Rect::from_center_size(part.center(), vec2(part.width(), 6.0))
        } else {
            part
        };
        let strength = if look == Look::Bar { 0.7 } else { 0.3 };
        painter.rect_filled(part, 2.0, theme.accent.gamma_multiply(strength));
        for (mark, edge) in [(marks.mark_in, part.left()), (marks.mark_out, part.right())] {
            if mark.is_some() {
                painter.line_segment(
                    [pos2(edge, rect.top()), pos2(edge, rect.bottom())],
                    Stroke::new(2.0, theme.accent),
                );
            }
        }
    }
    if length > 0.0 {
        let x = x_of(at.clamp(0.0, length));
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, theme.text),
        );
        if look == Look::Bar {
            painter.circle_filled(pos2(x, rect.center().y), 5.0, theme.text);
        }
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let pressed = response.clicked() || response.dragged() || response.drag_started();
    let seek = pressed
        .then(|| response.interact_pointer_pos())
        .flatten()
        .map(|p| seconds_at(rect, length, p.x));
    if seek.is_some() {
        ui.ctx().request_repaint();
    }
    seek.filter(|_| length > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marked_part_stays_inside_the_file() {
        let mut marks = Marks::default();
        assert_eq!(marks.part(10.0), (0.0, 10.0));
        assert!(!marks.is_set());
        marks.set_in(2.0);
        marks.set_out(8.0);
        assert_eq!(marks.part(10.0), (2.0, 8.0));
        assert_eq!(marks.part(5.0), (2.0, 5.0), "a shorter file cuts the out mark");
        marks.set_in(9.0);
        assert_eq!(marks.mark_out, None, "an out mark before the new in mark goes");
        marks.set_out(1.0);
        assert_eq!(marks.mark_in, None, "an in mark after the new out mark goes");
    }

    #[test]
    fn a_place_on_the_bar_is_a_time_in_the_file() {
        let rect = Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 18.0));
        assert_eq!(seconds_at(rect, 10.0, 200.0), 5.0);
        assert_eq!(seconds_at(rect, 10.0, 50.0), 0.0);
        assert_eq!(seconds_at(rect, 10.0, 900.0), 10.0);
    }
}
