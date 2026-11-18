//! The draggable boundaries between parts of a page stacked top to bottom.
//!
//! Every page with a viewer over a timeline uses [`splitter`], so the
//! boundary looks, drags and resets the same everywhere. The page only
//! keeps the [`Split`] (in its settings) and gives the upper part the
//! height [`Split::top_height`] returns.
//!
//! A page of more than two parts uses [`stack`]: a handle in each gap
//! between two parts, the same handle as [`splitter`]'s. Each part has a
//! least height and a usual one, and keeps the share of the height it was
//! dragged to (a [`Part`]'s `kept`, kept by the page in its settings); the
//! last part takes what is left.

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
    if let Some(by) = dragged_by(ui, rect, &response) {
        let top = split.top_height(total, min_top, min_bottom);
        *split = Split::for_height(top + by, total, min_top, min_bottom);
    }
    if response.double_clicked() {
        *split = Split::default();
        change.settled = true;
    }
    change.settled |= response.drag_stopped();
    change.changed = *split != before;
    paint_handle(ui, theme, rect, &response);
    change
}

/// How far the pointer dragging the handle in `rect` is from it this
/// frame, measured from where in the handle it was grabbed, so the handle
/// doesn't jump under the pointer.
fn dragged_by(ui: &Ui, rect: Rect, response: &egui::Response) -> Option<f32> {
    let grab_id = response.id.with("grab");
    if response.drag_started()
        && let Some(pressed) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(grab_id, pressed.y - rect.center().y));
    }
    if !response.dragged() {
        return None;
    }
    let p = response.interact_pointer_pos()?;
    ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    let grab: f32 = ui.data(|d| d.get_temp(grab_id)).unwrap_or(0.0);
    Some(p.y - grab - rect.center().y)
}

/// The handle: a short bar in the middle of `rect`, and a line across
/// while it is pointed at or dragged.
fn paint_handle(ui: &Ui, theme: &Theme, rect: Rect, response: &egui::Response) {
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
}

/// One part of a [`stack`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    /// The least height it keeps (while the page has room for every least).
    pub least: f32,
    /// Its height when nothing was kept for it.
    pub usual: f32,
    /// The share of the stack's height it was dragged to, kept by the page
    /// (`None`: the usual height). The last part's is never used: it takes
    /// what is left.
    pub kept: Option<f32>,
}

impl Part {
    pub fn new(least: f32, usual: f32, kept: Option<f32>) -> Part {
        Part { least, usual, kept }
    }

    /// The height it asks for out of `room`, never under its least.
    fn wanted(&self, room: f32) -> f32 {
        let kept = self.kept.filter(|k| k.is_finite() && *k > 0.0 && *k < 1.0);
        kept.map_or(self.usual, |k| k * room).max(self.least)
    }
}

/// The heights of `parts` sharing `room` (the stack's height without its
/// gaps): each part what it asks for, the last what is left. When the last
/// would be under its least, the others give it room in proportion to what
/// they have over theirs; a page too small for every least leaves the last
/// what there is.
pub fn fit(room: f32, parts: &[Part]) -> Vec<f32> {
    let Some((last, firsts)) = parts.split_last() else {
        return Vec::new();
    };
    let mut heights: Vec<f32> = firsts.iter().map(|p| p.wanted(room)).collect();
    let used: f32 = heights.iter().sum();
    let short = last.least - (room - used);
    if short > 0.0 {
        let spare: f32 = heights.iter().zip(firsts).map(|(h, p)| h - p.least).sum();
        if spare > 0.0 {
            let give = short.min(spare) / spare;
            for (h, p) in heights.iter_mut().zip(firsts) {
                *h -= (*h - p.least) * give;
            }
        }
    }
    let used: f32 = heights.iter().sum();
    heights.push((room - used).max(0.0));
    heights
}

/// `heights` with the boundary under part `boundary` moved down by `by`
/// (up when negative). The parts on the side it moves into give way one
/// after another, the nearest first, each down to its least; the part on
/// the other side takes what they gave.
pub fn moved(heights: &[f32], parts: &[Part], boundary: usize, by: f32) -> Vec<f32> {
    let mut out = heights.to_vec();
    if boundary + 1 >= out.len() || parts.len() != out.len() || !by.is_finite() {
        return out;
    }
    let mut give = |order: &mut dyn Iterator<Item = usize>, wanted: f32| {
        let mut given = 0.0;
        for i in order {
            let take = (out[i] - parts[i].least).max(0.0).min(wanted - given);
            out[i] -= take;
            given += take;
        }
        given
    };
    if by > 0.0 {
        let given = give(&mut (boundary + 1..heights.len()), by);
        out[boundary] += given;
    } else {
        let given = give(&mut (0..=boundary).rev(), -by);
        out[boundary + 1] += given;
    }
    out
}

/// Keeps in `parts` the shares of the parts whose height changed from
/// `before` to `after` (not the last's: it takes what is left).
fn keep(parts: &mut [Part], before: &[f32], after: &[f32], room: f32) {
    if room <= 0.0 {
        return;
    }
    let firsts = parts.len().saturating_sub(1);
    for (i, part) in parts.iter_mut().enumerate().take(firsts) {
        if let (Some(b), Some(a)) = (before.get(i), after.get(i))
            && (a - b).abs() > 0.01
        {
            part.kept = Some(a / room);
        }
    }
}

/// Moves the boundary under part `boundary` by `by` and keeps the new shares.
pub fn drag(room: f32, parts: &mut [Part], boundary: usize, by: f32) {
    let before = fit(room, parts);
    let after = moved(&before, parts, boundary, by);
    keep(parts, &before, &after, room);
}

/// Puts the boundary under part `boundary` back where it is when nothing
/// is kept, as if dragged there; a part that ends at its usual height
/// keeps nothing again (so it follows the page's height as at first).
pub fn reset(room: f32, parts: &mut [Part], boundary: usize) {
    let usual: Vec<Part> = parts.iter().map(|p| Part { kept: None, ..*p }).collect();
    let usual_heights = fit(room, &usual);
    let now = fit(room, parts);
    let at = |h: &[f32]| h.iter().take(boundary + 1).sum::<f32>();
    let by = at(&usual_heights) - at(&now);
    let after = moved(&now, parts, boundary, by);
    keep(parts, &now, &after, room);
    let firsts = parts.len().saturating_sub(1);
    for (i, part) in parts.iter_mut().enumerate().take(firsts) {
        if (after[i] - usual_heights[i]).abs() < 0.5 {
            part.kept = None;
        }
    }
}

/// The parts' rectangles in `area`, top to bottom, `gap` apart.
pub fn stacked(area: Rect, gap: f32, parts: &[Part]) -> Vec<Rect> {
    let gaps = gap * parts.len().saturating_sub(1) as f32;
    let mut y = area.top();
    fit(area.height() - gaps, parts)
        .into_iter()
        .map(|h| {
            let r = Rect::from_min_size(egui::pos2(area.left(), y), vec2(area.width(), h));
            y += h + gap;
            r
        })
        .collect()
}

/// Lays `parts` out in `area`, top to bottom, `gap` apart, with a handle in
/// each gap (`tips[i]` says what the one under part `i` shares), and lets
/// the handles be dragged; double-click puts one back. Returns the parts'
/// rectangles as they are after this frame's drag, and what changed (the
/// page keeps the parts' `kept` in its settings).
pub fn stack(
    ui: &mut Ui,
    theme: &Theme,
    area: Rect,
    gap: f32,
    parts: &mut [Part],
    tips: &[&str],
) -> (Vec<Rect>, SplitterChange) {
    let gaps = gap * parts.len().saturating_sub(1) as f32;
    let room = area.height() - gaps;
    let before: Vec<Option<f32>> = parts.iter().map(|p| p.kept).collect();
    let rects = stacked(area, gap, parts);
    let mut change = SplitterChange::default();
    let mut handles = Vec::with_capacity(rects.len().saturating_sub(1));
    for (i, pair) in rects.windows(2).enumerate() {
        let rect = Rect::from_min_max(
            egui::pos2(area.left(), pair[0].bottom()),
            egui::pos2(area.right(), pair[1].top()),
        );
        let response = ui
            .interact(
                rect,
                ui.scope_id().with(("stack-handle", i)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical);
        let response = match tips.get(i) {
            Some(tip) => response.on_hover_text(format!("{tip} Double-click: the usual size.")),
            None => response,
        };
        if let Some(by) = dragged_by(ui, rect, &response) {
            drag(room, parts, i, by);
        }
        if response.double_clicked() {
            reset(room, parts, i);
            change.settled = true;
        }
        change.settled |= response.drag_stopped();
        handles.push(response);
    }
    let after: Vec<Option<f32>> = parts.iter().map(|p| p.kept).collect();
    change.changed = after != before;
    let rects = if change.changed {
        stacked(area, gap, parts)
    } else {
        rects
    };
    for (pair, response) in rects.windows(2).zip(&handles) {
        let rect = Rect::from_min_max(
            egui::pos2(area.left(), pair[0].bottom()),
            egui::pos2(area.right(), pair[1].top()),
        );
        paint_handle(ui, theme, rect, response);
    }
    (rects, change)
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

    /// A page of three parts: 300 usual (least 200), 100 usual (least 100)
    /// and the rest (least 200).
    fn three() -> [Part; 3] {
        [
            Part::new(200.0, 300.0, None),
            Part::new(100.0, 100.0, None),
            Part::new(200.0, 0.0, None),
        ]
    }

    #[test]
    fn a_stack_gives_each_part_its_usual_height_and_the_last_the_rest() {
        assert_eq!(fit(1000.0, &three()), vec![300.0, 100.0, 600.0]);
        let mut parts = three();
        parts[1].kept = Some(0.25);
        assert_eq!(fit(1000.0, &parts), vec![300.0, 250.0, 450.0]);
        // A kept share follows the page's height.
        assert_eq!(fit(800.0, &parts), vec![300.0, 200.0, 300.0]);
        // Never under a part's least, whatever was kept.
        parts[0].kept = Some(0.01);
        assert_eq!(fit(1000.0, &parts)[0], 200.0);
        // A damaged share means the usual height.
        parts[0].kept = Some(f32::NAN);
        assert_eq!(fit(1000.0, &parts)[0], 300.0);
        assert!(fit(1000.0, &[]).is_empty());
    }

    #[test]
    fn a_smaller_page_takes_room_from_the_parts_over_their_least() {
        let mut parts = three();
        parts[1].kept = Some(0.4); // 400 of 1000
        // 700: the last needs 200 of the 500 asked for; the first has 100
        // over its least, the second 180 at 0.4 × 700 = 280.
        let h = fit(700.0, &parts);
        assert!((h.iter().sum::<f32>() - 700.0).abs() < 1e-3, "{h:?}");
        assert_eq!(h[2], 200.0, "the last keeps its least");
        assert!(h[0] >= 200.0 && h[1] >= 100.0, "{h:?}");
        assert!(h[0] < 300.0 && h[1] < 280.0, "both gave room: {h:?}");
        // Too small for every least: the others keep theirs, the last has what is left.
        assert_eq!(fit(350.0, &parts), vec![200.0, 100.0, 50.0]);
        assert_eq!(fit(250.0, &parts), vec![200.0, 100.0, 0.0]);
    }

    #[test]
    fn a_boundary_moved_pushes_the_parts_beyond_down_to_their_least() {
        let parts = three();
        let h = [300.0, 100.0, 600.0];
        // The band's lower boundary down 200: the last gives it.
        assert_eq!(moved(&h, &parts, 1, 200.0), vec![300.0, 300.0, 400.0]);
        // The first boundary down 50: the band is at its least, so the last gives way.
        assert_eq!(moved(&h, &parts, 0, 50.0), vec![350.0, 100.0, 550.0]);
        // Far down: everything under it stops at its least.
        assert_eq!(moved(&h, &parts, 0, 900.0), vec![700.0, 100.0, 200.0]);
        // Far up: the first stops at its least.
        assert_eq!(moved(&h, &parts, 1, -900.0), vec![200.0, 100.0, 700.0]);
        // A boundary that is not there changes nothing.
        assert_eq!(moved(&h, &parts, 2, 50.0), h.to_vec());
    }

    #[test]
    fn dragging_keeps_shares_and_double_click_puts_the_boundary_back() {
        let mut parts = three();
        drag(1000.0, &mut parts, 1, 200.0);
        assert_eq!(parts[0].kept, None, "the part not moved keeps following the page");
        assert_eq!(parts[1].kept, Some(0.3));
        drag(1000.0, &mut parts, 0, -100.0);
        assert_eq!(parts[0].kept, Some(0.2));
        assert_eq!(parts[1].kept, Some(0.4), "the lower boundary stayed put");
        assert_eq!(fit(1000.0, &parts), vec![200.0, 400.0, 400.0]);
        // The first boundary back: the first part usual again, the band
        // giving it the room, the lower boundary where it was.
        reset(1000.0, &mut parts, 0);
        assert_eq!(parts[0].kept, None);
        assert_eq!(fit(1000.0, &parts), vec![300.0, 300.0, 400.0]);
        // The second back: everything usual.
        reset(1000.0, &mut parts, 1);
        assert_eq!(parts.map(|p| p.kept), [None, None, None]);
        assert_eq!(fit(1000.0, &parts), vec![300.0, 100.0, 600.0]);
    }

    /// One frame of a 1000-point-tall page stacked as [`three`] (10-point gaps).
    fn stack_frame(
        ctx: &egui::Context,
        parts: &mut [Part],
        events: Vec<egui::Event>,
    ) -> (Vec<Rect>, SplitterChange) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 1020.0))),
            events,
            ..Default::default()
        };
        let mut out = (Vec::new(), SplitterChange::default());
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| {
                    let area = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 1020.0));
                    out = stack(ui, &Theme::ASH, area, 10.0, parts, &["Top.", "Band."]);
                });
        });
        output.drop_without_applying_deltas();
        out
    }

    #[test]
    fn the_stacks_handles_drag_and_double_click_back() {
        let ctx = egui::Context::default();
        let mut parts = three();
        let at = |y: f32| egui::pos2(400.0, y);
        // The handle under the band is in the gap at 410..420.
        let (rects, _) = stack_frame(&ctx, &mut parts, vec![egui::Event::PointerMoved(at(415.0))]);
        assert_eq!(
            rects[1],
            Rect::from_min_size(egui::pos2(0.0, 310.0), vec2(800.0, 100.0))
        );
        stack_frame(&ctx, &mut parts, vec![button(at(415.0), true)]);
        let (rects, change) = stack_frame(&ctx, &mut parts, vec![egui::Event::PointerMoved(at(565.0))]);
        assert!(change.changed && !change.settled);
        assert_eq!(rects[1].height(), 250.0, "the band grew by the drag");
        assert_eq!(rects[2].top(), 570.0);
        let (_, change) = stack_frame(&ctx, &mut parts, vec![button(at(565.0), false)]);
        assert!(change.settled, "letting go is the moment to save");
        assert_eq!(parts[1].kept, Some(0.25));
        // Double-click: the band's usual height again.
        for _ in 0..2 {
            stack_frame(&ctx, &mut parts, vec![button(at(565.0), true)]);
            stack_frame(&ctx, &mut parts, vec![button(at(565.0), false)]);
        }
        assert_eq!(parts[1].kept, None);
        let (rects, _) = stack_frame(&ctx, &mut parts, vec![]);
        assert_eq!(rects[1].height(), 100.0);
    }
}
