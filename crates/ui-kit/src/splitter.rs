//! The draggable boundary between a page's viewer and its timeline.
//!
//! Every page with a viewer over a timeline uses this one, so the boundary
//! looks, drags and resets the same everywhere. The page only keeps the
//! [`Split`] (in its settings) and gives the upper part the height
//! [`Split::top_height`] returns.

use crate::Theme;
use egui::{CursorIcon, Rect, Sense, Ui, vec2};
use serde::{Deserialize, Serialize};

/// Height of the strip the handle is drawn in.
pub const HANDLE_HEIGHT: f32 = 8.0;
/// The least height the viewer keeps (its picture, timecodes and quality menu).
pub const MIN_VIEWER: f32 = 200.0;
/// The least height everything under the handle keeps: the tool row and a
/// timeline with room for a track of each kind.
pub const MIN_TIMELINE: f32 = 230.0;

/// The share of the height the upper part (the viewer) gets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Split(pub f32);

impl Default for Split {
    fn default() -> Self {
        Split(Split::DEFAULT)
    }
}

impl Split {
    /// A little more than half for the viewer.
    pub const DEFAULT: f32 = 0.55;

    /// Height of the upper part when `total` is shared: the split's share,
    /// but never under `min_top`, and leaving `min_bottom` below when both fit.
    pub fn top_height(self, total: f32, min_top: f32, min_bottom: f32) -> f32 {
        let share = if self.0.is_finite() {
            self.0
        } else {
            Split::DEFAULT
        };
        let most = (total - min_bottom).max(min_top);
        (total * share).clamp(min_top, most)
    }

    /// The split that gives the upper part `height` of `total`, within the same limits.
    pub fn for_height(height: f32, total: f32, min_top: f32, min_bottom: f32) -> Split {
        if total <= 0.0 {
            return Split::default();
        }
        let most = (total - min_bottom).max(min_top);
        Split(height.clamp(min_top, most) / total)
    }
}

/// What happened to the splitter this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SplitterChange {
    /// The split moved.
    pub changed: bool,
    /// A drag ended or the split was reset: the moment to save it.
    pub settled: bool,
}

/// Draws the handle across the available width and lets it be dragged:
/// down for a taller upper part, up for a taller lower part. Double-click
/// goes back to the default. `total` is the height the two parts share.
pub fn splitter(
    ui: &mut Ui,
    theme: &Theme,
    split: &mut Split,
    total: f32,
    min_top: f32,
    min_bottom: f32,
) -> SplitterChange {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), HANDLE_HEIGHT), Sense::click_and_drag());
    let response = response
        .on_hover_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(
            "Drag to share the height between the viewer and the timeline. Double-click: the usual sizes.",
        );
    let before = *split;
    let mut change = SplitterChange::default();
    // Where in the handle it was grabbed, so it doesn't jump under the pointer.
    let grab_id = response.id.with("grab");
    if response.drag_started()
        && let Some(pressed) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(grab_id, pressed.y - rect.center().y));
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        let grab: f32 = ui.data(|d| d.get_temp(grab_id)).unwrap_or(0.0);
        let top = split.top_height(total, min_top, min_bottom);
        let wanted = top + (p.y - grab - rect.center().y);
        *split = Split::for_height(wanted, total, min_top, min_bottom);
    }
    if response.double_clicked() {
        *split = Split::default();
        change.settled = true;
    }
    change.settled |= response.drag_stopped();
    change.changed = *split != before;

    let active = response.dragged() || response.hovered();
    let painter = ui.painter_at(rect);
    if active {
        painter.rect_filled(
            Rect::from_center_size(rect.center(), vec2(rect.width(), 1.0)),
            0.0,
            theme.line,
        );
    }
    painter.rect_filled(
        Rect::from_center_size(rect.center(), vec2(44.0, 3.0)),
        1.5,
        if response.dragged() {
            theme.text_secondary
        } else if response.hovered() {
            theme.muted
        } else {
            theme.line_strong
        },
    );
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewer_gets_its_share_within_the_limits() {
        let s = Split::default();
        assert_eq!(s.top_height(1000.0, 160.0, 230.0), 550.0);
        assert_eq!(
            Split(0.05).top_height(1000.0, 160.0, 230.0),
            160.0,
            "never under the least"
        );
        assert_eq!(
            Split(0.99).top_height(1000.0, 160.0, 230.0),
            770.0,
            "the timeline keeps its least"
        );
        // A window too small for both: the viewer keeps its least.
        assert_eq!(Split(0.5).top_height(300.0, 160.0, 230.0), 160.0);
        assert_eq!(
            Split(f32::NAN).top_height(1000.0, 160.0, 230.0),
            550.0,
            "a damaged value means the default"
        );
    }

    #[test]
    fn a_height_becomes_a_split_and_back() {
        let s = Split::for_height(400.0, 1000.0, 160.0, 230.0);
        assert_eq!(s, Split(0.4));
        assert_eq!(s.top_height(1000.0, 160.0, 230.0), 400.0);
        assert_eq!(Split::for_height(10.0, 1000.0, 160.0, 230.0), Split(0.16));
        assert_eq!(Split::for_height(990.0, 1000.0, 160.0, 230.0), Split(0.77));
        assert_eq!(Split::for_height(100.0, 0.0, 160.0, 230.0), Split::default());
    }

    /// One frame of a 600-point-tall page with the handle under a 300-point
    /// upper part, with `events` as input.
    fn frame(ctx: &egui::Context, split: &mut Split, events: Vec<egui::Event>) -> SplitterChange {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut change = SplitterChange::default();
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    let top = split.top_height(600.0, 100.0, 100.0);
                    ui.allocate_exact_size(vec2(800.0, top), Sense::hover());
                    change = splitter(ui, &Theme::ASH, split, 600.0, 100.0, 100.0);
                });
        });
        // Nothing draws these frames.
        output.drop_without_applying_deltas();
        change
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn dragging_the_handle_moves_the_split_and_stops_at_the_limits() {
        let ctx = egui::Context::default();
        let mut split = Split(0.5);
        let at = |y: f32| egui::pos2(400.0, y);
        frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(304.0))]);
        frame(&ctx, &mut split, vec![button(at(304.0), true)]);
        // Down by 90: a taller upper part.
        let change = frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(394.0))]);
        assert!(change.changed && !change.settled);
        assert!((split.0 - 0.65).abs() < 1e-4, "{split:?}");
        // Far past the bottom: the lower part keeps its least.
        frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(590.0))]);
        frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(590.0))]);
        assert!((split.0 - 500.0 / 600.0).abs() < 1e-4, "{split:?}");
        // And far up: the upper part keeps its least.
        frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(20.0))]);
        frame(&ctx, &mut split, vec![egui::Event::PointerMoved(at(20.0))]);
        assert!((split.0 - 100.0 / 600.0).abs() < 1e-4, "{split:?}");
        let change = frame(&ctx, &mut split, vec![button(at(20.0), false)]);
        assert!(change.settled, "letting go is the moment to save");
    }
}
