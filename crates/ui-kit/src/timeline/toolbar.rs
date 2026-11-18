//! The row of tools above the timeline, the same on every page that shows
//! one: each button is a [`ToolbarClick`], which knows its picture, its
//! name and whether it is lit; the page gives each its tooltip (with its
//! shortcut). Where the row does not fit, [`folded_toolbar`] offers the
//! same buttons in one dropdown.

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

    /// The button's name, as the folded dropdown lists it.
    pub fn name(self) -> &'static str {
        match self {
            ToolbarClick::Tool(t) => t.name(),
            ToolbarClick::Snapping => "Snapping",
            ToolbarClick::Fit => "Show the whole timeline",
            ToolbarClick::RoughCut => "Rough cut",
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

/// The text of the [`folded_toolbar`]'s button: the tool in use and a
/// downward arrow. A page measures it to know how wide the folded bar is.
pub fn folded_toolbar_text(view: &TimelineView) -> String {
    format!("{} \u{25BE}", view.tool.name())
}

/// The [`toolbar`]'s buttons folded into one dropdown, for a bar too
/// narrow for them in a row: [`ToolbarClick::ALL`] in order, by name, a
/// line where the row has its gap, lit as the row lights them.
pub fn folded_toolbar(ui: &mut Ui, view: &TimelineView) -> Option<ToolbarClick> {
    let mut clicked = None;
    ui.menu_button(folded_toolbar_text(view), |ui| {
        for entry in folded_entries(view) {
            if entry.line_before {
                ui.separator();
            }
            let name = entry.click.name();
            let r = if entry.click == ToolbarClick::Fit {
                ui.button(name)
            } else {
                ui.selectable_label(entry.lit, name)
            };
            if r.clicked() {
                clicked = Some(entry.click);
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("The timeline's tools (the bar is too narrow for them in a row)");
    clicked
}

/// One line of the [`folded_toolbar`]'s dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FoldedEntry {
    click: ToolbarClick,
    /// Drawn lit, as the row would draw its button.
    lit: bool,
    /// A line above it, where the row has its gap.
    line_before: bool,
}

/// The dropdown's lines: every button of the row, in its order.
fn folded_entries(view: &TimelineView) -> Vec<FoldedEntry> {
    ToolbarClick::ALL
        .into_iter()
        .map(|click| FoldedEntry {
            click,
            lit: click.is_on(view),
            line_before: click.starts_group(),
        })
        .collect()
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
    fn every_button_has_its_own_name_and_each_tool_its_own() {
        let names: Vec<_> = ToolbarClick::ALL.iter().map(|c| c.name()).collect();
        for (i, n) in names.iter().enumerate() {
            assert!(!n.is_empty());
            assert!(!names[i + 1..].contains(n), "{n} twice");
        }
        assert_eq!(ToolbarClick::RoughCut.name(), "Rough cut");
        assert_eq!(ToolbarClick::Tool(Tool::Slide).name(), Tool::Slide.name());
        let view = TimelineView {
            tool: Tool::Ripple,
            ..TimelineView::default()
        };
        assert_eq!(folded_toolbar_text(&view), "Ripple \u{25BE}");
    }

    /// The folded dropdown offers every button of the row, Rough cut
    /// included, in the row's order, lit as the row lights them.
    #[test]
    fn the_folded_dropdown_lists_every_button_of_the_row() {
        let view = TimelineView {
            tool: Tool::Slip,
            rough_cut: true,
            ..TimelineView::default()
        };
        let entries = folded_entries(&view);
        let clicks: Vec<_> = entries.iter().map(|e| e.click).collect();
        assert_eq!(clicks, ToolbarClick::ALL);
        assert!(clicks.contains(&ToolbarClick::RoughCut));
        let lines: Vec<_> = entries
            .iter()
            .filter(|e| e.line_before)
            .map(|e| e.click)
            .collect();
        assert_eq!(lines, [ToolbarClick::Snapping]);
        let lit: Vec<_> = entries.iter().filter(|e| e.lit).map(|e| e.click).collect();
        assert_eq!(
            lit,
            [
                ToolbarClick::Tool(Tool::Slip),
                ToolbarClick::Snapping,
                ToolbarClick::RoughCut
            ]
        );
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
