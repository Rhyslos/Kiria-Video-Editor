//! The viewer: the picture of the timeline, the same on every page that shows one.

use crate::Theme;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

/// How sharp the preview is. Lower is lighter on the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PreviewQuality {
    /// As sharp as the viewer is big on screen.
    #[default]
    Auto,
    /// The timeline's full size.
    Full,
    Half,
    Quarter,
}

impl PreviewQuality {
    pub const ALL: [PreviewQuality; 4] = [
        PreviewQuality::Auto,
        PreviewQuality::Full,
        PreviewQuality::Half,
        PreviewQuality::Quarter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "Auto",
            PreviewQuality::Full => "Full",
            PreviewQuality::Half => "1/2",
            PreviewQuality::Quarter => "1/4",
        }
    }

    /// The largest picture to decode, in pixels, for a timeline of
    /// `timeline` size shown in a viewer `on_screen` pixels big.
    pub fn picture_size(self, timeline: (u32, u32), on_screen: (u32, u32)) -> (u32, u32) {
        let (w, h) = (timeline.0.max(2), timeline.1.max(2));
        match self {
            PreviewQuality::Full => (w, h),
            PreviewQuality::Half => (w / 2, h / 2),
            PreviewQuality::Quarter => (w / 4, h / 4),
            PreviewQuality::Auto => (on_screen.0.clamp(2, w), on_screen.1.clamp(2, h)),
        }
    }
}

/// Where the picture goes: the largest rectangle of the timeline's shape
/// inside `area`, centred.
pub fn picture_rect(area: Rect, timeline: (u32, u32)) -> Rect {
    let aspect = timeline.0.max(1) as f32 / timeline.1.max(1) as f32;
    let size = if area.width() / area.height().max(1.0) > aspect {
        vec2(area.height() * aspect, area.height())
    } else {
        vec2(area.width(), area.width() / aspect)
    };
    Rect::from_center_size(area.center(), size)
}

/// Draws the viewer filling `size`: black frame of the timeline's shape and
/// the picture fitted inside it, plus a quiet note in the corner. Returns the
/// frame's rectangle (for sizing the next request) and the response.
pub fn viewer(
    ui: &mut Ui,
    theme: &Theme,
    size: Vec2,
    timeline: (u32, u32),
    picture: Option<&egui::TextureHandle>,
    note: &str,
) -> (Rect, egui::Response) {
    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 6.0, theme.viewport);
    let frame = picture_rect(area.shrink(8.0), timeline);
    painter.rect_filled(frame, 0.0, Color32::BLACK);
    if let Some(texture) = picture {
        // The picture already has the frame's shape unless the clip's shape
        // differs: fit it inside, centred, never stretched.
        let [w, h] = texture.size();
        let inner = picture_rect(frame, (w as u32, h as u32));
        painter.image(
            texture.id(),
            inner,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    if !note.is_empty() {
        painter.text(
            area.right_bottom() - vec2(12.0, 10.0),
            Align2::RIGHT_BOTTOM,
            note,
            FontId::proportional(11.0),
            theme.faint,
        );
    }
    (frame, response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_sizes_per_quality() {
        let tl = (2560, 1440);
        assert_eq!(PreviewQuality::Full.picture_size(tl, (800, 450)), (2560, 1440));
        assert_eq!(PreviewQuality::Half.picture_size(tl, (800, 450)), (1280, 720));
        assert_eq!(PreviewQuality::Quarter.picture_size(tl, (800, 450)), (640, 360));
        assert_eq!(PreviewQuality::Auto.picture_size(tl, (800, 450)), (800, 450));
        assert_eq!(
            PreviewQuality::Auto.picture_size(tl, (5000, 3000)),
            (2560, 1440),
            "never above full"
        );
    }

    #[test]
    fn the_picture_keeps_its_shape() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let r = picture_rect(area, (1920, 1080));
        assert_eq!(r.size(), vec2(1000.0, 562.5));
        let tall = picture_rect(area, (1080, 1920));
        assert_eq!(tall.size(), vec2(562.5, 1000.0));
        assert_eq!(tall.center(), area.center());
    }
}
