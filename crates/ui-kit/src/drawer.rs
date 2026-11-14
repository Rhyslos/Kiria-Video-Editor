//! The drawer on the left edge of a page: a narrow tab beside the viewer
//! that opens a panel with media (and, later, effects) floating over the
//! page, so nothing underneath moves. The same on every page that has one;
//! the page fills in the list.

use crate::Theme;
use crate::layout::{DRAWER_WIDTH, EDGE_TAB_HEIGHT, EDGE_TAB_WIDTH};
use egui::epaint::TextShape;
use egui::{
    Color32, Context, FontId, Id, InnerResponse, Order, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};

/// What the drawer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerTab {
    #[default]
    Media,
    Effects,
}

/// The tab on the left edge: a tall rounded strip with its label written
/// upward, centred down the height it is given; click it to open or close.
pub fn edge_tab(ui: &mut Ui, theme: &Theme, label: &str, open: bool) -> Response {
    // Centred down its spot (the height beside the toolbar and viewer).
    let spot = ui.available_rect_before_wrap();
    let height = spot.height().min(EDGE_TAB_HEIGHT);
    let rect = Rect::from_center_size(
        pos2(spot.left() + EDGE_TAB_WIDTH / 2.0, spot.center().y),
        vec2(EDGE_TAB_WIDTH, height),
    );
    let response = ui.allocate_rect(rect, Sense::click());
    let tab = Rect::from_min_size(rect.min + vec2(2.0, 0.0), vec2(EDGE_TAB_WIDTH - 4.0, height));
    let fill = if open {
        theme.accent.gamma_multiply(0.25)
    } else if response.hovered() {
        theme.line
    } else {
        theme.panel
    };
    let painter = ui.painter();
    painter.rect_filled(tab, 6.0, fill);
    painter.rect_stroke(tab, 6.0, Stroke::new(1.0, theme.line), egui::StrokeKind::Inside);
    let colour = if open { theme.accent } else { theme.text_secondary };
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), colour);
    // Written bottom to top, centred in the tab.
    let size = galley.size();
    let at = pos2(tab.center().x - size.y / 2.0, tab.center().y + size.x / 2.0);
    painter.add(TextShape::new(at, galley, colour).with_angle(-std::f32::consts::FRAC_PI_2));
    response.on_hover_text(if open {
        "Close the drawer"
    } else {
        "Open the media drawer"
    })
}

/// The Media / Effects switch at the top of the drawer.
pub fn tab_switch(ui: &mut Ui, tab: &mut DrawerTab) {
    ui.horizontal(|ui| {
        ui.selectable_value(tab, DrawerTab::Media, "Media");
        ui.selectable_value(tab, DrawerTab::Effects, "Effects");
    });
}

/// Where the edge tab goes: down the page's left edge, as tall as the part
/// of the page beside it (`beside_height`, the toolbar and the viewer).
pub fn tab_rect(page: Rect, beside_height: f32) -> Rect {
    Rect::from_min_size(page.min, vec2(EDGE_TAB_WIDTH, beside_height.max(0.0)))
}

/// The part of `rect` right of the edge tab: what sits beside the tab.
pub fn beside_tab(rect: Rect) -> Rect {
    Rect::from_min_max(pos2(rect.left() + EDGE_TAB_WIDTH, rect.top()), rect.max)
}

/// Where the open drawer floats: right of the edge tab, the page's whole
/// height, over whatever is there.
pub fn drawer_rect(page: Rect) -> Rect {
    Rect::from_min_size(
        pos2(page.left() + EDGE_TAB_WIDTH, page.top()),
        vec2(DRAWER_WIDTH, page.height()),
    )
}

/// The part of the open drawer that a file dragged out of it may stay over
/// without it stepping aside: the drawer's stretch beside the tab (`tab`),
/// above the timeline.
pub fn keep_while_dragging(drawer: Rect, tab: Rect) -> Rect {
    Rect::from_min_max(
        drawer.min,
        pos2(drawer.right(), tab.bottom().min(drawer.bottom())),
    )
}

/// Whether the open drawer is drawn this frame. It steps aside while a
/// file dragged out of it is outside `keep` (see `keep_while_dragging`),
/// so the timeline under it, all of it, can take the drop; it comes back
/// when the pointer returns there or lets go.
pub fn shown(open: bool, dragging_out: bool, pointer: Option<Pos2>, keep: Rect) -> bool {
    open && !(dragging_out && pointer.is_some_and(|p| !keep.contains(p)))
}

/// Whether a click closes the open drawer: one on the page itself (not on
/// a window or a menu over it), away from the tab (which toggles it).
pub fn click_closes(click_at: Pos2, on_page: bool, tab: Rect) -> bool {
    on_page && !tab.contains(click_at)
}

/// Whether a click this frame landed on the page (not on the drawer, a
/// window or a menu) away from `tab`: the open drawer closes then.
pub fn clicked_away(ctx: &Context, tab: Rect) -> bool {
    let Some(at) = ctx.input(|i| i.pointer.any_click().then(|| i.pointer.interact_pos()).flatten()) else {
        return false;
    };
    let on_page = ctx
        .layer_id_at(at)
        .is_none_or(|layer| layer.order == Order::Background);
    click_closes(at, on_page, tab)
}

/// The open drawer, floating over the page at `rect` (see `drawer_rect`)
/// with a shadow and an edge on its right; `add_contents` fills it.
pub fn overlay<R>(
    ctx: &Context,
    theme: &Theme,
    id: Id,
    rect: Rect,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    egui::Area::new(id)
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(theme.bg)
                .shadow(egui::Shadow {
                    offset: [6, 0],
                    blur: 18,
                    spread: 0,
                    color: Color32::from_black_alpha(110),
                })
                .inner_margin(egui::Margin::symmetric(8, 4));
            let inner = frame
                .show(ui, |ui| {
                    let size = rect.size() - vec2(16.0, 8.0);
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    add_contents(ui)
                })
                .inner;
            ui.painter()
                .vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, theme.line));
            inner
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Rect {
        Rect::from_min_size(pos2(16.0, 64.0), vec2(1400.0, 820.0))
    }

    #[test]
    fn the_edge_tab_is_centred_down_its_spot() {
        let ctx = Context::default();
        let spot = tab_rect(page(), 450.0);
        let mut drawn = None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let mut edge = ui.new_child(egui::UiBuilder::new().max_rect(spot));
            drawn = Some(edge_tab(&mut edge, &Theme::ASH, "Media", false).rect);
        });
        // Nothing draws this frame.
        output.drop_without_applying_deltas();
        let tab = drawn.unwrap();
        assert_eq!(tab.height(), EDGE_TAB_HEIGHT);
        assert!(
            (tab.center().y - spot.center().y).abs() < 0.5,
            "{tab:?} in {spot:?}"
        );
        assert!((tab.center().x - spot.center().x).abs() < 0.5);
    }

    #[test]
    fn tab_runs_beside_the_viewer_only() {
        let tab = tab_rect(page(), 450.0);
        assert_eq!(tab.min, page().min);
        assert_eq!(tab.width(), EDGE_TAB_WIDTH);
        assert_eq!(tab.height(), 450.0);
        assert_eq!(tab_rect(page(), -3.0).height(), 0.0);
    }

    #[test]
    fn content_beside_the_tab_starts_after_it() {
        let beside = beside_tab(page());
        assert_eq!(beside.left(), page().left() + EDGE_TAB_WIDTH);
        assert_eq!(beside.right(), page().right());
        assert_eq!(beside.top(), page().top());
    }

    #[test]
    fn drawer_floats_right_of_the_tab_down_the_whole_page() {
        let drawer = drawer_rect(page());
        assert_eq!(drawer.left(), page().left() + EDGE_TAB_WIDTH);
        assert_eq!(drawer.width(), DRAWER_WIDTH);
        assert_eq!(drawer.top(), page().top());
        assert_eq!(drawer.bottom(), page().bottom());
    }

    #[test]
    fn drawer_steps_aside_for_a_drag_over_the_timeline_or_away() {
        let drawer = drawer_rect(page());
        let tab = tab_rect(page(), 450.0);
        let keep = keep_while_dragging(drawer, tab);
        assert_eq!(keep.bottom(), tab.bottom());
        assert_eq!((keep.left(), keep.right()), (drawer.left(), drawer.right()));
        let beside_viewer = pos2(drawer.center().x, tab.center().y);
        let over_timeline = pos2(drawer.center().x, tab.bottom() + 100.0);
        let away = pos2(drawer.right() + 40.0, tab.center().y);
        assert!(shown(true, false, Some(over_timeline), keep));
        assert!(shown(true, true, Some(beside_viewer), keep));
        assert!(!shown(true, true, Some(over_timeline), keep));
        assert!(!shown(true, true, Some(away), keep));
        assert!(shown(true, true, None, keep));
        assert!(!shown(false, false, Some(beside_viewer), keep));
    }

    #[test]
    fn only_a_click_on_the_page_away_from_the_tab_closes() {
        let tab = tab_rect(page(), 450.0);
        let away = pos2(800.0, 700.0);
        assert!(click_closes(away, true, tab));
        assert!(!click_closes(away, false, tab));
        assert!(!click_closes(tab.center(), true, tab));
    }
}
