//! A row of tabs that may hold more than fits: it scrolls sideways (the
//! wheel, a drag, or a touchpad) and fades out at an edge where more tabs
//! wait, so nothing is cut off without a sign. The Library's type tabs and
//! custom lists use it. `tab_rows` lays the same tabs out in as many rows
//! as they need instead, every one in sight (Settings). Also `chip`: a small removable pill, for the
//! filters that are on.

use crate::Theme;
use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

/// How wide the fade at an edge with more tabs behind it is.
const FADE: f32 = 36.0;

/// One tab of the strip.
#[derive(Debug, Clone, Copy)]
pub struct StripTab<'a> {
    pub text: &'a str,
    /// A number after the name (how many things it holds), if any.
    pub count: Option<usize>,
    pub selected: bool,
}

/// What the strip hands back: each tab's response in order (for clicks
/// and its right-click menu), and the response of what `trailing` added.
#[derive(Debug)]
pub struct StripResponse<R> {
    pub tabs: Vec<Response>,
    pub trailing: R,
}

/// The tabs, then `trailing` (such as "+ New list"), in a strip `id_salt`
/// as wide as the room it has, scrolling and fading when they don't fit.
pub fn tab_strip<R>(
    ui: &mut Ui,
    theme: &Theme,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    tabs: &[StripTab<'_>],
    trailing: impl FnOnce(&mut Ui) -> R,
) -> StripResponse<R> {
    let output = egui::ScrollArea::horizontal()
        .id_salt(("tab-strip", id_salt))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let responses: Vec<Response> = tabs.iter().map(|tab| strip_tab(ui, theme, tab)).collect();
                ui.add_space(8.0);
                let trailing = trailing(ui);
                (responses, trailing)
            })
            .inner
        });
    let (inner, content, offset) = (
        output.inner_rect,
        output.content_size,
        output.state.clamped_offset().x,
    );
    let (left, right) = fades(offset, inner.width(), content.x);
    let painter = ui.painter_at(inner);
    // The strip sits on the page's background, so it fades into that.
    let fill = theme.bg;
    if left {
        fade(
            &painter,
            Rect::from_min_size(inner.min, vec2(FADE, inner.height())),
            fill,
            true,
        );
    }
    if right {
        fade(
            &painter,
            Rect::from_min_size(
                pos2(inner.right() - FADE, inner.top()),
                vec2(FADE, inner.height()),
            ),
            fill,
            false,
        );
    }
    let (tabs, trailing) = output.inner;
    StripResponse { tabs, trailing }
}

/// The tabs in rows as wide as `ui`'s room, as few rows as they need and
/// as even as they can be (not one tab alone on the last), so every tab is
/// in sight (Settings' categories). Each tab's response, in order.
pub fn tab_rows(ui: &mut Ui, theme: &Theme, tabs: &[StripTab<'_>]) -> Vec<Response> {
    const GAP: f32 = 2.0;
    // Measured as if open (its name in bold), so opening another tab
    // never moves a tab to another row.
    let widths: Vec<f32> = tabs
        .iter()
        .map(|t| tab_galley(ui, theme, &StripTab { selected: true, ..*t }).size().x + 20.0)
        .collect();
    let rows = even_rows(&widths, ui.available_width(), GAP);
    let mut responses = Vec::with_capacity(tabs.len());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        for row in rows {
            ui.horizontal(|ui| {
                responses.extend(tabs[row].iter().map(|tab| strip_tab(ui, theme, tab)));
            });
        }
    });
    responses
}

/// The tabs of each row, for tabs `widths` wide in `room` with `gap`
/// between them: the fewest rows that hold them, made as even as they can
/// be by narrowing the width each row may fill until one more row would
/// be needed.
fn even_rows(widths: &[f32], room: f32, gap: f32) -> Vec<std::ops::Range<usize>> {
    let rows_at = |limit: f32| {
        let mut rows = Vec::new();
        let (mut start, mut used) = (0, 0.0);
        for (i, w) in widths.iter().enumerate() {
            let needs = if i == start { *w } else { used + gap + w };
            if i > start && needs > limit {
                rows.push(start..i);
                start = i;
                used = *w;
            } else {
                used = needs;
            }
        }
        if start < widths.len() {
            rows.push(start..widths.len());
        }
        rows
    };
    let fewest = rows_at(room);
    let mut best = fewest.clone();
    let mut limit = room;
    // Narrower and narrower while the rows stay as few.
    while limit > 0.0 {
        limit -= 8.0;
        let tried = rows_at(limit);
        if tried.len() > fewest.len() {
            break;
        }
        best = tried;
    }
    best
}

/// Which edges fade: the left once scrolled away from the start, the right
/// while more lies beyond the room `width` of content `content` wide.
pub fn fades(offset: f32, width: f32, content: f32) -> (bool, bool) {
    (offset > 0.5, offset + width < content - 0.5)
}

/// A soft edge from `fill` to nothing, solid at the strip's edge.
fn fade(painter: &egui::Painter, rect: Rect, fill: Color32, solid_left: bool) {
    let mut mesh = egui::Mesh::default();
    let (a, b) = if solid_left {
        (fill, Color32::TRANSPARENT)
    } else {
        (Color32::TRANSPARENT, fill)
    };
    let i = mesh.vertices.len() as u32;
    for (pos, colour) in [
        (rect.left_top(), a),
        (rect.right_top(), b),
        (rect.right_bottom(), b),
        (rect.left_bottom(), a),
    ] {
        mesh.colored_vertex(pos, colour);
    }
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// One tab: its name, a quiet count, and a line under it when it is the one open.
fn strip_tab(ui: &mut Ui, theme: &Theme, tab: &StripTab<'_>) -> Response {
    let colour = if tab.selected {
        theme.text
    } else {
        theme.text_secondary
    };
    let galley = tab_galley(ui, theme, tab);
    let size = galley.size() + vec2(20.0, 14.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    if response.hovered() && !tab.selected {
        painter.rect_filled(rect, 6.0, theme.active.gamma_multiply(0.6));
    }
    painter.galley(rect.min + vec2(10.0, 7.0), galley, colour);
    if tab.selected {
        let under = Rect::from_min_max(
            pos2(rect.left() + 8.0, rect.bottom() - 2.0),
            pos2(rect.right() - 8.0, rect.bottom()),
        );
        painter.rect_filled(under, 1.0, theme.accent);
    }
    response
}

/// A tab's words laid out: its name, then its count, quieter.
fn tab_galley(ui: &Ui, theme: &Theme, tab: &StripTab<'_>) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let colour = if tab.selected {
        theme.text
    } else {
        theme.text_secondary
    };
    let font = if tab.selected {
        FontId::new(13.0, crate::fonts::semibold())
    } else {
        FontId::proportional(13.0)
    };
    job.append(tab.text, 0.0, egui::TextFormat::simple(font, colour));
    if let Some(count) = tab.count {
        job.append(
            &count.to_string(),
            6.0,
            egui::TextFormat::simple(FontId::proportional(11.0), theme.muted),
        );
    }
    ui.painter().layout_job(job)
}

/// A small pill naming a filter that is on, with a dot in `colour` when
/// it has one and a × that takes it off. Returns whether it was taken off.
pub fn chip(ui: &mut Ui, theme: &Theme, text: &str, colour: Option<Color32>) -> bool {
    let font = FontId::proportional(12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), theme.text);
    let dot = if colour.is_some() { 12.0 } else { 0.0 };
    let size = vec2(galley.size().x + dot + 34.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 11.0, theme.raised);
    painter.rect_stroke(rect, 11.0, Stroke::new(1.0, theme.line), egui::StrokeKind::Inside);
    let mut x = rect.left() + 10.0;
    if let Some(colour) = colour {
        painter.circle_filled(pos2(x + 3.0, rect.center().y), 4.0, colour);
        x += dot;
    }
    painter.galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        theme.text,
    );
    let cross = pos2(rect.right() - 12.0, rect.center().y);
    let hovered = response.hovered();
    painter.text(
        cross,
        egui::Align2::CENTER_CENTER,
        "×",
        font,
        if hovered { theme.text } else { theme.muted },
    );
    response.on_hover_text("Take this filter off").clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_fade_where_more_tabs_wait() {
        assert_eq!(fades(0.0, 400.0, 300.0), (false, false), "everything fits");
        assert_eq!(fades(0.0, 400.0, 900.0), (false, true), "more to the right");
        assert_eq!(fades(200.0, 400.0, 900.0), (true, true), "more both ways");
        assert_eq!(fades(500.0, 400.0, 900.0), (true, false), "at the end");
    }

    #[test]
    fn tabs_in_rows_all_stay_inside_their_room() {
        let ctx = egui::Context::default();
        let names = [
            "Look",
            "Editing",
            "Playback & proxies",
            "Audio",
            "Library",
            "Resources",
            "Cache",
            "Graphics",
            "Keyboard",
            "Diagnostics",
            "Help",
        ];
        let tabs: Vec<StripTab<'_>> = names
            .iter()
            .map(|text| StripTab {
                text,
                count: None,
                selected: false,
            })
            .collect();
        let room = Rect::from_min_size(pos2(10.0, 10.0), vec2(420.0, 400.0));
        let mut rects = Vec::new();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(room));
            rects = tab_rows(&mut child, &Theme::ASH, &tabs)
                .iter()
                .map(|r| r.rect)
                .collect();
        })
        .drop_without_applying_deltas();
        assert_eq!(rects.len(), names.len());
        for r in &rects {
            assert!(
                r.left() >= room.left() && r.right() <= room.right() + 0.5,
                "{r:?} inside"
            );
        }
        let rows: std::collections::BTreeSet<i32> = rects.iter().map(|r| r.top() as i32).collect();
        assert!(rows.len() > 1, "more than one row when they don't fit in one");
    }

    #[test]
    fn rows_are_as_few_as_fit_and_as_even_as_they_can_be() {
        let widths = [60.0; 11];
        // Ten fit in a row of 620: two rows, six and five, not ten and one.
        assert_eq!(even_rows(&widths, 620.0, 2.0), vec![0..6, 6..11]);
        // All fit: one row.
        assert_eq!(even_rows(&widths, 2000.0, 2.0), vec![0..11]);
        // A tab wider than the room still gets a row of its own.
        assert_eq!(even_rows(&[500.0, 50.0], 300.0, 2.0), vec![0..1, 1..2]);
    }
}
