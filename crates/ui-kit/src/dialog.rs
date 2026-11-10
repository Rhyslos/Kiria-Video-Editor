//! Dialogs: one look for every window that asks something and waits.

use crate::Theme;
use egui::{Context, Frame, Id, Margin, Modal, ModalResponse, Stroke, Ui};

/// Shows a dialog in front of everything, with the shared frame, padding and
/// dimmed backdrop. `id` must be the same every frame. Click outside or
/// Escape asks it to close (see [`ModalResponse::should_close`]).
pub fn dialog<R>(ctx: &Context, theme: &Theme, id: &str, add: impl FnOnce(&mut Ui) -> R) -> ModalResponse<R> {
    let frame = Frame::new()
        .fill(theme.raised)
        .stroke(Stroke::new(1.0, theme.line_strong))
        .corner_radius(10.0)
        .inner_margin(Margin::same(20));
    Modal::new(Id::unique(id))
        .frame(frame)
        .backdrop_color(theme.viewport.gamma_multiply(0.6))
        .show(ctx, add)
}
