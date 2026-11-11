//! How the keyboard shortcuts page draws its small parts: one key, a
//! context chip, a context dot. The page decides what each shows.

use super::layout::{Cap, Layout};
use crate::Theme;
use crate::shortcuts::Context;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

/// A colour between `a` and `b`: `share` of the way to `b`.
pub fn mix(a: Color32, b: Color32, share: f32) -> Color32 {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * share).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// How one key looks this frame.
#[derive(Debug, Clone, Default)]
pub struct KeyLook {
    /// The contexts the key does something in, with the modifiers chosen
    /// on the page. Empty: the key is dimmed.
    pub contexts: Vec<Context>,
    /// A modifier that is held on the page.
    pub held: bool,
    /// Pinned, or found by the search: outlined.
    pub marked: bool,
    pub hovered: bool,
}

/// One key: tinted with a dot per context when it does something, dimmed
/// when it does nothing. `origin` is the keyboard's top left corner and
/// `unit` the size of a letter key.
pub fn key(ui: &Ui, theme: &Theme, layout: &Layout, cap: &Cap, origin: Pos2, unit: f32, look: &KeyLook) {
    let fill = match (look.contexts.first(), look.held) {
        (Some(context), _) => mix(
            theme.raised,
            context.colour(),
            if look.hovered { 0.5 } else { 0.32 },
        ),
        (None, true) => theme.line_strong,
        (None, false) if look.hovered => theme.active,
        (None, false) => theme.panel,
    };
    let stroke = if look.marked {
        Stroke::new(2.0, theme.accent)
    } else if look.held {
        Stroke::new(1.0, theme.text_secondary)
    } else {
        Stroke::new(1.0, theme.line)
    };
    // The gap between keys, in key units.
    let inset = 1.5 / unit;
    let place = |p: Pos2| origin + p.to_vec2() * unit;
    let painter = ui.painter();
    let inner = cap.rect.shrink(inset);
    let upper = Rect::from_min_max(place(inner.min), place(inner.max));
    match cap.lower {
        None => {
            painter.rect_filled(upper, 4.0, fill);
            painter.rect_stroke(upper, 4.0, stroke, egui::StrokeKind::Inside);
        }
        // A tall Enter: two filled parts under one outline.
        Some(lower) => {
            let lower = lower.shrink(inset);
            painter.rect_filled(upper, 0.0, fill);
            painter.rect_filled(
                Rect::from_min_max(place(pos2(lower.left(), inner.bottom())), place(lower.max)),
                0.0,
                fill,
            );
            let outline = cap.outline(inset).into_iter().map(place).collect();
            painter.add(egui::Shape::closed_line(outline, stroke));
        }
    }

    let active = !look.contexts.is_empty() || look.held;
    let colour = if active { theme.text } else { theme.faint };
    let size = (unit * 0.3).clamp(9.0, 13.0);
    let legend = layout.printed_on(cap);
    match layout.us_mark(cap.spot) {
        None => {
            painter.text(
                upper.center(),
                Align2::CENTER_CENTER,
                legend,
                FontId::proportional(size),
                colour,
            );
        }
        // Printed differently here: this keyboard's character, and in the
        // corner the US one that shortcuts are called by.
        Some(us) => {
            painter.text(
                upper.center() + vec2(unit * 0.08, -unit * 0.06),
                Align2::CENTER_CENTER,
                legend,
                FontId::proportional(size),
                colour,
            );
            painter.text(
                upper.left_bottom() + vec2(4.0, -3.0),
                Align2::LEFT_BOTTOM,
                us,
                FontId::proportional((size * 0.75).max(8.0)),
                if active { theme.text_secondary } else { theme.faint },
            );
        }
    }
    for (i, context) in look.contexts.iter().enumerate() {
        painter.circle_filled(
            pos2(upper.right() - 6.0 - i as f32 * 8.0, upper.top() + 6.0),
            3.0,
            context.colour(),
        );
    }
}

/// A small filled circle in line with text.
pub fn dot(ui: &mut Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, colour);
}

/// One context chip: its colour and name. Picked chips stand out; when
/// some are picked the others fade.
pub fn chip(ui: &mut Ui, theme: &Theme, context: Context, picked: bool, filtering: bool) -> egui::Response {
    let faded = filtering && !picked;
    let text = if faded { theme.faint } else { theme.text };
    let galley = ui
        .painter()
        .layout_no_wrap(context.label().to_owned(), FontId::proportional(12.5), text);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 24.0), Sense::click());
    let fill = if picked || response.hovered() {
        theme.active
    } else {
        theme.panel
    };
    let colour = if faded {
        mix(theme.panel, context.colour(), 0.4)
    } else {
        context.colour()
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 12.0, fill);
    painter.rect_stroke(
        rect,
        12.0,
        Stroke::new(1.0, if picked { colour } else { theme.line }),
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 12.0, rect.center().y), 4.0, colour);
    painter.galley(
        pos2(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    response.on_hover_text(if picked {
        "Click to stop showing only this."
    } else {
        "Click to show only this. Pick more than one if you like."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_goes_from_one_colour_to_the_other() {
        let (a, b) = (Color32::from_rgb(0, 100, 200), Color32::from_rgb(100, 0, 200));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(50, 50, 200));
    }
}
