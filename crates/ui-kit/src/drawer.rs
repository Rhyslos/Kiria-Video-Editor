//! The drawer on the left edge of a page: a narrow tab that slides out a
//! panel with media (and, later, effects). The same on every page that has
//! one; the page fills in the list.

use crate::Theme;
use crate::layout::EDGE_TAB_WIDTH;
use egui::epaint::TextShape;
use egui::{FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

/// What the drawer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerTab {
    #[default]
    Media,
    Effects,
}

/// The tab on the left edge: a tall rounded strip with its label written
/// upward. Fills the height it is given; click it to open or close.
pub fn edge_tab(ui: &mut Ui, theme: &Theme, label: &str, open: bool) -> Response {
    let height = ui.available_height().min(140.0);
    let (rect, response) = ui.allocate_exact_size(vec2(EDGE_TAB_WIDTH, height), Sense::click());
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
