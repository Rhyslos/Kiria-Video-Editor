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
/// The Audio page's tools beside the viewer.
pub const TOOLS_WIDTH: f32 = 610.0;
/// The gap between a page's panels and what is between them.
pub const PANEL_GAP: f32 = 12.0;

/// Where a window floating over a page may be: below the top bar, a
/// [`PANEL_GAP`] in from every edge, so it never covers the page tabs and
/// always fits on the screen (taller content scrolls inside it).
pub fn window_area(screen: egui::Rect) -> egui::Rect {
    let top = screen.top() + TOP_BAR_HEIGHT + PANEL_GAP;
    let area = egui::Rect::from_min_max(
        egui::pos2(screen.left() + PANEL_GAP, top),
        egui::pos2(screen.right() - PANEL_GAP, screen.bottom() - PANEL_GAP),
    );
    // A screen too small for the gaps gets an area of no size below the bar.
    if area.is_positive() {
        area
    } else {
        egui::Rect::from_min_size(
            egui::pos2(screen.left(), top.min(screen.bottom())),
            egui::Vec2::ZERO,
        )
    }
}

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

/// A folder's path written to fit in about `chars` characters: its own
/// name first (what tells folders apart), then where it is, cut in the
/// middle when long: "Clients — /home/ana/…/Videos/projects".
pub fn path_text(path: &std::path::Path, chars: usize) -> String {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return path.display().to_string();
    };
    let Some(parent) = path
        .parent()
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
    else {
        return name;
    };
    let head = format!("{name} — ");
    let room = chars.saturating_sub(head.chars().count()).max(12);
    format!("{head}{}", cut_middle(&parent, room))
}

/// `text` in at most `chars` characters, its middle taken out for "…".
pub fn cut_middle(text: &str, chars: usize) -> String {
    let all: Vec<char> = text.chars().collect();
    if all.len() <= chars {
        return text.to_owned();
    }
    let keep = chars.saturating_sub(1);
    let front = keep / 2;
    let back = keep - front;
    let mut out: String = all[..front].iter().collect();
    out.push('…');
    out.extend(&all[all.len() - back..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_windows_fit_below_the_top_bar() {
        for (w, h) in [(1440.0, 900.0), (1280.0, 800.0)] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h));
            let area = window_area(screen);
            assert_eq!(area.top(), TOP_BAR_HEIGHT + PANEL_GAP);
            assert_eq!(area.bottom(), h - PANEL_GAP);
            assert!(screen.contains_rect(area));
            assert!(
                area.width() >= crate::settings::MAX_SIZE.x,
                "Settings gets its full width"
            );
        }
        let tiny = window_area(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(10.0, 30.0),
        ));
        assert_eq!(tiny.size(), egui::Vec2::ZERO);
    }

    #[test]
    fn tabs_sit_over_the_viewport_centre() {
        // The mockups: a 1440 px window puts the tabs at x = 574.
        assert_eq!(viewport_centre_x(1440.0), 574.0);
    }

    #[test]
    fn a_path_shows_its_folder_first_and_is_cut_in_the_middle() {
        let p = std::path::Path::new(
            "/tmp/claude-0/-home-claude/3db0b6c6-1d35/scratchpad/usetest/t4/projects/Clients",
        );
        let text = path_text(p, 46);
        assert!(text.starts_with("Clients — /tmp/"), "{text}");
        assert!(text.ends_with("/t4/projects"), "{text}");
        assert!(text.chars().count() <= 46, "{text}");
        assert_eq!(
            path_text(std::path::Path::new("/v/Personal"), 46),
            "Personal — /v"
        );
        assert_eq!(cut_middle("abcdef", 10), "abcdef");
        assert_eq!(cut_middle("abcdefghij", 5), "ab…ij");
    }
}
