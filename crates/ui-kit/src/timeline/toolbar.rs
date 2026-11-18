//! The row of tools above the timeline, the same on every page that shows
//! one: each button is a [`ToolbarClick`], which knows its picture and
//! whether it is lit; the page gives each its tooltip (with its shortcut).

use super::{TimelineView, Tool};
use crate::Theme;
use egui::{Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// What the timeline's toolbar asked for: one of its buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarClick {
    Tool(Tool),
    Snapping,
    Fit,
    RoughCut,
}

impl ToolbarClick {
    /// The buttons in the order they are shown: the tools, then (after a
    /// gap) Snapping, Fit and the rough cut view.
    pub const ALL: [ToolbarClick; 9] = [
        ToolbarClick::Tool(Tool::Select),
        ToolbarClick::Tool(Tool::Blade),
        ToolbarClick::Tool(Tool::Ripple),
        ToolbarClick::Tool(Tool::Roll),
        ToolbarClick::Tool(Tool::Slip),
        ToolbarClick::Tool(Tool::Slide),
        ToolbarClick::Snapping,
        ToolbarClick::Fit,
        ToolbarClick::RoughCut,
    ];

    /// Whether the button is drawn lit: the tool in use (none while the
    /// lines are worked: the pointer works them, not a tool), snapping
    /// on, the rough cut view on. Fit is never lit.
    fn is_on(self, view: &TimelineView) -> bool {
        match self {
            ToolbarClick::Tool(t) => view.tool == t && view.lines.is_none(),
            ToolbarClick::Snapping => view.snapping,
            ToolbarClick::Fit => false,
            ToolbarClick::RoughCut => view.rough_cut,
        }
    }

    /// The first button of the second group, after the gap.
    fn starts_group(self) -> bool {
        self == ToolbarClick::Snapping
    }
}

/// How wide [`toolbar`] is (at most): its buttons, the gap, and the
/// spacing between them.
pub const TOOLBAR_WIDTH: f32 = BUTTONS * TOOL_BUTTON + TOOLBAR_GAP + BUTTONS * TOOLBAR_SPACING;

const BUTTONS: f32 = ToolbarClick::ALL.len() as f32;

const TOOL_BUTTON: f32 = 30.0;

/// The room between the tools and the buttons after them.
const TOOLBAR_GAP: f32 = 10.0;

/// The toolbar's own spacing between buttons (it sets it, so the width holds).
const TOOLBAR_SPACING: f32 = 4.0;

/// The row of tools above the timeline: [`ToolbarClick::ALL`] in order.
/// `tip` gives each button's tooltip (with its shortcut).
pub fn toolbar(
    ui: &mut Ui,
    theme: &Theme,
    view: &TimelineView,
    tip: impl Fn(ToolbarClick) -> String,
) -> Option<ToolbarClick> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = TOOLBAR_SPACING;
        let size = vec2(TOOL_BUTTON, 26.0);
        for click in ToolbarClick::ALL {
            if click.starts_group() {
                ui.add_space(TOOLBAR_GAP);
            }
            let on = click.is_on(view);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            let r = r.on_hover_text(tip(click));
            let fill = if on {
                theme.accent.gamma_multiply(0.25)
            } else if r.hovered() {
                theme.line
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let colour = if on { theme.accent } else { theme.text_secondary };
            draw_icon(click, ui.painter(), rect.center(), colour);
            if r.clicked() {
                clicked = Some(click);
            }
        }
    });
    clicked
}

/// While the volume and pan lines are worked (`TimelineView::lines`): a
/// lit pill after the [`toolbar`] saying so, with a cross to leave them
/// (`tip` names the key, which leaves them too, as Escape does). Returns
/// whether it was clicked.
pub fn lines_chip(ui: &mut Ui, theme: &Theme, tip: &str) -> bool {
    let text = "Lines on   \u{00D7}";
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.5), theme.accent);
    let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 26.0), Sense::click());
    let fill = if r.hovered() {
        theme.accent.gamma_multiply(0.35)
    } else {
        theme.accent.gamma_multiply(0.22)
    };
    ui.painter().rect_filled(rect, 13.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme.accent);
    r.on_hover_text(format!(
        "The pointer works the volume and pan lines of the audio tracks, not clips. Click to go back to the clips ({tip}, or Escape)."
    ))
    .clicked()
}

/// Draws the button's picture centred on `c`.
fn draw_icon(click: ToolbarClick, p: &egui::Painter, c: Pos2, colour: Color32) {
    let stroke = Stroke::new(1.6, colour);
    match click {
        // Pointer arrow.
        ToolbarClick::Tool(Tool::Select) => {
            p.add(Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, -7.0),
                    c + vec2(5.0, 1.0),
                    c + vec2(0.5, 1.5),
                    c + vec2(-1.5, 6.5),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        // Blade: a cut line through a clip.
        ToolbarClick::Tool(Tool::Blade) => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(16.0, 9.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [c + vec2(0.0, -8.0), c + vec2(0.0, 8.0)],
                Stroke::new(2.0, colour),
            );
        }
        // Ripple: an edge with the rest of the timeline following.
        ToolbarClick::Tool(Tool::Ripple) => {
            p.line_segment(
                [c + vec2(-4.0, -7.0), c + vec2(-4.0, 7.0)],
                Stroke::new(2.0, colour),
            );
            p.rect_stroke(
                Rect::from_min_max(c + vec2(-1.0, -4.0), c + vec2(8.0, 4.0)),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment([c + vec2(-8.0, 0.0), c + vec2(-5.0, 0.0)], stroke);
        }
        // Roll: two clips meeting at a cut that moves.
        ToolbarClick::Tool(Tool::Roll) => {
            for s in [-1.0, 1.0] {
                p.rect_stroke(
                    Rect::from_min_max(
                        c + vec2(if s < 0.0 { -8.0 } else { 1.0 }, -4.0),
                        c + vec2(if s < 0.0 { -1.0 } else { 8.0 }, 4.0),
                    ),
                    1.5,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            p.line_segment(
                [c + vec2(0.0, -8.0), c + vec2(0.0, 8.0)],
                Stroke::new(2.0, colour),
            );
        }
        // Slip: arrows inside a clip.
        ToolbarClick::Tool(Tool::Slip) => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(18.0, 10.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(5.0 * s, 0.0), c + vec2(3.0 * s, -2.0)], stroke);
                p.line_segment([c + vec2(5.0 * s, 0.0), c + vec2(3.0 * s, 2.0)], stroke);
            }
        }
        // Slide: a clip with arrows outside it.
        ToolbarClick::Tool(Tool::Slide) => {
            p.rect_filled(Rect::from_center_size(c, vec2(8.0, 10.0)), 1.5, colour);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(6.0 * s, 0.0), c + vec2(10.0 * s, 0.0)], stroke);
                p.line_segment([c + vec2(10.0 * s, 0.0), c + vec2(8.0 * s, -2.0)], stroke);
                p.line_segment([c + vec2(10.0 * s, 0.0), c + vec2(8.0 * s, 2.0)], stroke);
            }
        }
        // Snapping: a magnet.
        ToolbarClick::Snapping => {
            p.add(Shape::line(
                vec![
                    c + vec2(-5.0, -6.0),
                    c + vec2(-5.0, 1.0),
                    c + vec2(-3.0, 5.0),
                    c + vec2(0.0, 6.0),
                    c + vec2(3.0, 5.0),
                    c + vec2(5.0, 1.0),
                    c + vec2(5.0, -6.0),
                ],
                Stroke::new(2.4, colour),
            ));
        }
        // Rough cut: a picture with its sound underneath, as one strip.
        ToolbarClick::RoughCut => {
            p.rect_filled(
                Rect::from_min_max(c + vec2(-8.0, -6.0), c + vec2(8.0, 0.0)),
                1.5,
                colour,
            );
            for (i, h) in [2.0f32, 4.0, 3.0, 5.0, 2.0, 4.0, 3.0].iter().enumerate() {
                let x = c.x - 7.0 + i as f32 * 2.3;
                p.line_segment(
                    [pos2(x, c.y + 4.0 - h / 2.0), pos2(x, c.y + 4.0 + h / 2.0)],
                    stroke,
                );
            }
        }
        // Fit: arrows out to both sides.
        ToolbarClick::Fit => {
            p.line_segment([c + vec2(-7.0, 0.0), c + vec2(7.0, 0.0)], stroke);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(7.0 * s, 0.0), c + vec2(4.0 * s, -3.0)], stroke);
                p.line_segment([c + vec2(7.0 * s, 0.0), c + vec2(4.0 * s, 3.0)], stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_button_comes_once_in_order_with_the_gap_before_snapping() {
        let all = ToolbarClick::ALL;
        for (i, a) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(a), "{a:?} twice");
        }
        let tools: Vec<_> = all
            .iter()
            .take_while(|c| matches!(c, ToolbarClick::Tool(_)))
            .collect();
        assert_eq!(tools.len(), 6, "the tools come first");
        let groups: Vec<_> = all.iter().filter(|c| c.starts_group()).collect();
        assert_eq!(groups, [&ToolbarClick::Snapping]);
        assert_eq!(TOOLBAR_WIDTH, 9.0 * 30.0 + 10.0 + 9.0 * 4.0, "as wide as before");
    }

    #[test]
    fn the_tool_in_use_snapping_and_the_rough_cut_view_are_lit() {
        let mut view = TimelineView {
            tool: Tool::Blade,
            snapping: true,
            rough_cut: false,
            ..TimelineView::default()
        };
        let lit = |view: &TimelineView| -> Vec<ToolbarClick> {
            ToolbarClick::ALL.into_iter().filter(|c| c.is_on(view)).collect()
        };
        assert_eq!(
            lit(&view),
            [ToolbarClick::Tool(Tool::Blade), ToolbarClick::Snapping]
        );
        view.rough_cut = true;
        view.snapping = false;
        view.lines = Some(crate::timeline::LinesShown::default());
        assert_eq!(
            lit(&view),
            [ToolbarClick::RoughCut],
            "no tool while the lines are worked"
        );
    }
}
