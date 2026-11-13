//! Sizes and positions shared by every page, so nothing jumps when switching.

use serde::{Deserialize, Serialize};

/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f32 = 48.0;
/// Width of the right-hand panel (inspector, details, settings of the current tool).
pub const RIGHT_PANEL_WIDTH: f32 = 292.0;
/// Width of the left-hand panel: the launcher's folder tree, the Library page's labels and filters.
pub const LEFT_PANEL_WIDTH: f32 = 240.0;
/// Width of the drawer that slides out from the left edge (media, effects).
pub const DRAWER_WIDTH: f32 = 264.0;
/// Width of the tab on the left edge that opens the drawer.
pub const EDGE_TAB_WIDTH: f32 = 22.0;
/// Width of the master column next to the timeline.
pub const MASTER_WIDTH: f32 = 80.0;
/// The gap between a page's panels and what is between them.
pub const PANEL_GAP: f32 = 12.0;

/// A page's side panel (the Library's sidebar and detail, the Export page's
/// presets and summary): the panel colour, rounded, padded, filling `rect`.
pub fn panel(ui: &mut egui::Ui, theme: &crate::Theme, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::Frame::new()
        .fill(theme.panel)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(&mut child, |ui| {
            ui.set_min_size(rect.size() - egui::vec2(24.0, 20.0));
            ui.set_max_size(rect.size() - egui::vec2(24.0, 20.0));
            add(ui);
        });
}

/// The x position the page tabs are centred on: the middle of the viewport,
/// which is the window minus the right-hand panel. The play button sits on
/// the same line.
pub fn viewport_centre_x(window_width: f32) -> f32 {
    (window_width - RIGHT_PANEL_WIDTH) / 2.0
}

/// The five pages, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Page {
    Library,
    #[default]
    Edit,
    Color,
    Audio,
    Export,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Library, Page::Edit, Page::Color, Page::Audio, Page::Export];

    pub fn label(self) -> &'static str {
        match self {
            Page::Library => "Library",
            Page::Edit => "Edit",
            Page::Color => "Color",
            Page::Audio => "Audio",
            Page::Export => "Export",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_sit_over_the_viewport_centre() {
        // The mockups: a 1440 px window puts the tabs at x = 574.
        assert_eq!(viewport_centre_x(1440.0), 574.0);
    }
}
