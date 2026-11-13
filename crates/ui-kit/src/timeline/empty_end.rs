//! A nested clip longer than its timeline: what lies past the end of what
//! it plays is hatched, so an end that shows nothing can be seen.

use crate::theme::Theme;
use egui::{Rect, Stroke, pos2};

/// Hatches the part of a clip drawn in `r` from `x` (on screen) to its end.
pub(super) fn draw(p: &egui::Painter, theme: &Theme, r: Rect, x: f32) {
    let empty = Rect::from_min_max(pos2(x.max(r.left()), r.top()), r.max);
    if empty.width() < 1.0 {
        return;
    }
    let p = p.with_clip_rect(p.clip_rect().intersect(empty));
    p.rect_filled(empty, 0.0, theme.panel.gamma_multiply(0.75));
    let stroke = Stroke::new(1.0, theme.muted.gamma_multiply(0.6));
    let step = 7.0;
    let mut at = empty.left() - empty.height();
    while at < empty.right() {
        p.line_segment(
            [pos2(at, empty.bottom()), pos2(at + empty.height(), empty.top())],
            stroke,
        );
        at += step;
    }
    p.line_segment(
        [pos2(empty.left(), r.top()), pos2(empty.left(), r.bottom())],
        Stroke::new(1.0, theme.muted),
    );
}
