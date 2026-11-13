//! Colour themes. Every colour in the interface comes from here.
//!
//! The interface itself stays neutral; colour is kept for content and
//! status. The accent marks only the playhead, selection, the active tool and
//! the main button.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

/// The themes people can pick in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeName {
    /// Soft graphite greys with a bone accent. The default.
    #[default]
    Ash,
    /// Warm near-black with a copper accent.
    Carbon,
}

impl ThemeName {
    pub const ALL: [ThemeName; 2] = [ThemeName::Ash, ThemeName::Carbon];

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Ash => "Ash",
            ThemeName::Carbon => "Carbon",
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            ThemeName::Ash => Theme::ASH,
            ThemeName::Carbon => Theme::CARBON,
        }
    }
}

/// The colour tokens of one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Window background behind everything.
    pub bg: Color32,
    /// Panels: top bar, side panels, timeline.
    pub panel: Color32,
    /// Raised surfaces: buttons, tab groups, popups.
    pub raised: Color32,
    /// Behind the video picture.
    pub viewport: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    /// Hovered or selected rows and tabs.
    pub active: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    /// Text on an accent-filled button.
    pub accent_ink: Color32,
    /// A card: a group of rows lifted a step off the background (the
    /// sections of Settings).
    pub card: Color32,
    /// The rounded edge around a card.
    pub card_edge: Color32,
    /// The thin line between rows inside a card.
    pub divider: Color32,
}

/// Status colours mean the same thing in every theme.
pub mod status {
    use egui::Color32;
    /// Cached or ready.
    pub const GOOD: Color32 = Color32::from_rgb(0x5D, 0xBB, 0x7A);
    /// Needs attention.
    pub const WARNING: Color32 = Color32::from_rgb(0xE6, 0xC3, 0x5C);
    /// A blocking error.
    pub const ERROR: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
    /// The edit needle (the one red that is not an error).
    pub const EDIT_NEEDLE: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
}

/// Context colours for the keyboard shortcuts page: one per place a
/// command acts, the same in every theme and on the printed cheat sheet.
pub mod context {
    use egui::Color32;
    pub const ANYWHERE: Color32 = Color32::from_rgb(0x6C, 0xA6, 0xE8);
    pub const VIEWER: Color32 = Color32::from_rgb(0x4F, 0xC4, 0xB0);
    pub const TIMELINE: Color32 = Color32::from_rgb(0xB5, 0x8B, 0xE8);
    pub const DRAGGING: Color32 = Color32::from_rgb(0xE0, 0x7B, 0xA8);
}

/// Track role colours: the mark by an audio track's name, the top of its
/// mixer strip and the Audio page's Show filter. The same in every theme;
/// told apart by lightness as well as hue.
pub mod role {
    use egui::Color32;
    pub const DIALOGUE: Color32 = Color32::from_rgb(0xFB, 0x92, 0x3C);
    pub const MUSIC: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
    pub const EFFECTS: Color32 = Color32::from_rgb(0xA3, 0xE6, 0x35);
    pub const AMBIENCE: Color32 = Color32::from_rgb(0x9C, 0xA3, 0xAF);
    pub const OTHER: Color32 = Color32::from_rgb(0x6B, 0x70, 0x78);
}

/// The clarity check's colours: its findings' marks on the timeline and in
/// its band. The same in every theme, apart from the roles' colours.
pub mod clarity {
    use egui::Color32;
    /// Another track covers the voice.
    pub const COVERED: Color32 = Color32::from_rgb(0xE0, 0x7B, 0xF0);
    /// Several tracks pile up between 200 and 500 Hz.
    pub const MUDDY: Color32 = Color32::from_rgb(0xC8, 0xA9, 0x6E);
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Theme {
    pub const ASH: Theme = Theme {
        bg: hex(0x1A1B1C),
        panel: hex(0x222324),
        raised: hex(0x2A2B2D),
        viewport: hex(0x121314),
        line: hex(0x303133),
        line_strong: hex(0x45464A),
        active: hex(0x3A3B3E),
        text: hex(0xF0EFEC),
        text_secondary: hex(0xC6C5C1),
        muted: hex(0xA5A4A0),
        faint: hex(0x6C6B69),
        accent: hex(0xEDE6D6),
        accent_ink: hex(0x161616),
        card: hex(0x242527),
        card_edge: hex(0x37383B),
        divider: hex(0x2F3033),
    };

    pub const CARBON: Theme = Theme {
        bg: hex(0x0D0C0B),
        panel: hex(0x151413),
        raised: hex(0x1D1B19),
        viewport: hex(0x080707),
        line: hex(0x272421),
        line_strong: hex(0x3B3733),
        active: hex(0x2F2C29),
        text: hex(0xEDE9E4),
        text_secondary: hex(0xBBB4AC),
        muted: hex(0x9C958D),
        faint: hex(0x665F58),
        accent: hex(0xD98A4E),
        accent_ink: hex(0x1E1006),
        card: hex(0x181715),
        card_edge: hex(0x2E2B28),
        divider: hex(0x252220),
    };

    /// Applies this theme to egui's widgets.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = Visuals::dark();
        v.panel_fill = self.panel;
        v.window_fill = self.raised;
        v.extreme_bg_color = self.viewport;
        v.faint_bg_color = self.bg;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.window_stroke = Stroke::new(1.0, self.line_strong);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.error_fg_color = status::ERROR;
        v.warn_fg_color = status::WARNING;

        let widgets = &mut v.widgets;
        widgets.noninteractive.bg_fill = self.panel;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.line);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_secondary);
        for (w, fill) in [
            (&mut widgets.inactive, self.raised),
            (&mut widgets.hovered, self.active),
            (&mut widgets.active, self.active),
            (&mut widgets.open, self.active),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = CornerRadius::same(6);
            w.bg_stroke = Stroke::new(1.0, self.line);
            w.fg_stroke = Stroke::new(1.0, self.text);
        }
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.line_strong);
        widgets.inactive.fg_stroke = Stroke::new(1.0, self.text_secondary);

        ctx.set_visuals(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_unpacks_channels() {
        assert_eq!(hex(0x123456), Color32::from_rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn every_theme_keeps_text_readable() {
        // Text against panels must stay well apart in brightness.
        for name in ThemeName::ALL {
            let t = name.theme();
            let lum = |c: Color32| f32::from(c.r()) * 0.3 + f32::from(c.g()) * 0.59 + f32::from(c.b()) * 0.11;
            assert!(lum(t.text) - lum(t.panel) > 150.0, "{name:?}");
            assert!(lum(t.muted) - lum(t.panel) > 90.0, "{name:?}");
            // Settings: text on cards, and cards, edges and dividers that
            // can be told apart from what they sit on.
            assert!(lum(t.text) - lum(t.card) > 150.0, "{name:?}");
            assert!(lum(t.muted) - lum(t.card) > 85.0, "{name:?}");
            assert!(
                lum(t.card) - lum(t.bg) >= 8.0,
                "{name:?}: a card stands off the background"
            );
            assert!(
                lum(t.card_edge) - lum(t.card) >= 15.0,
                "{name:?}: a card has an edge"
            );
            assert!(lum(t.divider) - lum(t.card) >= 8.0, "{name:?}: rows are divided");
        }
    }
}
