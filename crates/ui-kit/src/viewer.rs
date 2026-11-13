//! The viewer: the picture of the timeline, the same on every page that shows one.
//!
//! It can be zoomed ([`Zoom`], `zoom.rs`): Fit (the usual) or a scale,
//! with Ctrl+wheel over the picture zooming at the pointer and a drag with
//! the middle button panning; [`zoom_menu`] in the bottom row says the
//! scale and picks one.

mod zoom;

pub use zoom::{MENU as ZOOM_MENU, Zoom, seen_part};

use crate::Theme;
use crate::shortcuts::{Action, ShortcutTable};
use crate::video_picture::VideoPicture;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, Vec2, vec2};
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

/// What the viewer shows.
#[derive(Clone, Copy)]
pub enum Picture<'a> {
    /// An RGBA picture, already upright.
    Texture(&'a egui::TextureHandle),
    /// The decoder's YUV planes: the GPU converts and turns them while drawing.
    Video(&'a VideoPicture),
}

impl std::fmt::Debug for Picture<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, (w, h)) = match self {
            Picture::Texture(_) => ("Texture", self.size()),
            Picture::Video(_) => ("Video", self.size()),
        };
        write!(f, "Picture::{kind}({w}×{h})")
    }
}

impl Picture<'_> {
    /// The size on screen, upright.
    fn size(&self) -> (u32, u32) {
        match self {
            Picture::Texture(texture) => {
                let [w, h] = texture.size();
                (w as u32, h as u32)
            }
            Picture::Video(video) => video.size(),
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

/// The room around the picture inside the viewer, in points.
const MARGIN: f32 = 8.0;

/// Draws the viewer filling `size`: black frame of the timeline's shape and
/// the picture fitted inside it, plus a quiet note in the corner. Returns the
/// frame's rectangle (for sizing the next request) and the response.
pub fn viewer(
    ui: &mut Ui,
    theme: &Theme,
    size: Vec2,
    timeline: (u32, u32),
    picture: Option<Picture<'_>>,
    note: &str,
) -> (Rect, egui::Response) {
    let mut fit = Zoom::FIT;
    draw(ui, theme, size, timeline, picture, note, &mut fit, false)
}

/// [`viewer`] with the picture zoomed as `zoom` says, and the hand's
/// zooming and panning read into it: Ctrl+wheel (or a pinch) over the
/// picture zooms at the pointer, a drag with the middle button pans. The
/// frame's rectangle handed back is the whole frame on screen, larger than
/// the viewer when zoomed in (what lies on the picture lines up with it);
/// [`seen`] is the part of it inside the viewer.
pub fn zoomed_viewer(
    ui: &mut Ui,
    theme: &Theme,
    size: Vec2,
    timeline: (u32, u32),
    picture: Option<Picture<'_>>,
    note: &str,
    zoom: &mut Zoom,
) -> (Rect, egui::Response) {
    draw(ui, theme, size, timeline, picture, note, zoom, true)
}

/// Draws the viewer zoomed as `zoom` says, reading the hand into it when
/// `by_hand`.
#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut Ui,
    theme: &Theme,
    size: Vec2,
    timeline: (u32, u32),
    picture: Option<Picture<'_>>,
    note: &str,
    zoom: &mut Zoom,
    by_hand: bool,
) -> (Rect, egui::Response) {
    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let room = room(area);
    let ppp = ui.ctx().pixels_per_point();
    if by_hand {
        zoom_by_hand(ui, &response, room, timeline, ppp, zoom);
    }
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 6.0, theme.viewport);
    let frame = zoom.frame_rect(room, timeline, ppp);
    let clipped = ui.painter_at(room);
    clipped.rect_filled(frame, 0.0, Color32::BLACK);
    // The picture already has the frame's shape unless the clip's shape
    // differs: fit it inside, centred, never stretched. Only what is seen
    // is drawn.
    if let Some(picture) = picture
        && let Some((on_screen, part)) = seen_part(picture_rect(frame, picture.size()), room)
    {
        match picture {
            Picture::Texture(texture) => {
                clipped.image(texture.id(), on_screen, part, Color32::WHITE);
            }
            Picture::Video(video) => {
                let part = [part.min.x, part.min.y, part.max.x, part.max.y];
                clipped.add(video.shape(on_screen, part));
            }
        }
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

/// The part of a frame drawn by [`zoomed_viewer`] that is seen: inside the
/// viewer's room. For placing what floats over the picture (the transport).
pub fn seen(frame: Rect, viewer: Rect) -> Rect {
    frame.intersect(room(viewer))
}

/// Where the picture may be inside a viewer drawn at `viewer`: what
/// [`Zoom`] fits the frame in and clips a zoomed one to.
pub fn room(viewer: Rect) -> Rect {
    viewer.shrink(MARGIN)
}

/// Ctrl+wheel (egui hands it over as a zoom factor, as for a pinch) over
/// the picture zooms at the pointer; a drag with the middle button pans,
/// with the grabbing hand.
fn zoom_by_hand(
    ui: &Ui,
    response: &egui::Response,
    room: Rect,
    timeline: (u32, u32),
    ppp: f32,
    zoom: &mut Zoom,
) {
    if response.hovered()
        && let Some(at) = response.hover_pos()
    {
        let factor = ui.input(|i| i.zoom_delta());
        if factor != 1.0 {
            zoom.zoom_by_at(factor, at, room, timeline, ppp);
        }
    }
    if response.dragged_by(egui::PointerButton::Middle) && !zoom.is_fit() {
        zoom.pan(response.drag_delta(), room, timeline, ppp);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
}

/// What the zoom menu picked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomPick {
    /// A command: zoom in, out, to fit or to 100 %.
    Command(Action),
    /// A scale from [`ZOOM_MENU`] (1.0 is 100 %).
    Scale(f32),
}

/// The zoom menu's lines: what each picks and its words. 100 % is the
/// command, so it shows the command's keys.
pub fn zoom_lines() -> Vec<(ZoomPick, String)> {
    let mut lines = vec![(ZoomPick::Command(Action::ViewerZoomFit), "Fit".to_owned())];
    for scale in ZOOM_MENU {
        let pick = if scale == 1.0 {
            ZoomPick::Command(Action::ViewerZoom100)
        } else {
            ZoomPick::Scale(scale)
        };
        lines.push((pick, format!("{} %", (scale * 100.0).round() as i64)));
    }
    lines.push((ZoomPick::Command(Action::ViewerZoomIn), "Zoom in".to_owned()));
    lines.push((ZoomPick::Command(Action::ViewerZoomOut), "Zoom out".to_owned()));
    lines
}

/// The zoom menu in the viewer's bottom row: says the scale (`readout`,
/// from [`Zoom::readout`]) and opens Fit, 50 %, 100 %, 200 %, Zoom in and
/// Zoom out, each with the keys it has in `shortcuts`. `zoom` marks the
/// line in use. Returns what was picked; the page runs a command like the
/// same command from the keyboard or search.
pub fn zoom_menu(ui: &mut Ui, zoom: &Zoom, readout: &str, shortcuts: &ShortcutTable) -> Option<ZoomPick> {
    let mut picked = None;
    let response = ui.menu_button(readout, |ui| {
        ui.set_min_width(200.0);
        for (index, (pick, words)) in zoom_lines().into_iter().enumerate() {
            let on = match pick {
                ZoomPick::Command(Action::ViewerZoomFit) => zoom.is_fit(),
                ZoomPick::Command(Action::ViewerZoom100) => zoom.is_at(1.0),
                ZoomPick::Scale(scale) => zoom.is_at(scale),
                ZoomPick::Command(_) => false,
            };
            if index == ZOOM_MENU.len() + 1 {
                ui.separator();
            }
            let mut line = egui::Button::selectable(on, words);
            if let ZoomPick::Command(action) = pick {
                line = line.shortcut_text(shortcuts.keys_for(action));
            }
            if ui.add(line).clicked() {
                picked = Some(pick);
                ui.close();
            }
        }
    });
    response.response.on_hover_text(format!(
        "How large the picture is shown. Ctrl+wheel over the picture zooms at the pointer; drag with the middle button to move around. {}",
        shortcuts.tip(Action::ViewerZoomFit)
    ));
    picked
}

/// Writes `text` small in the top left corner of a picture in `frame`, on
/// a dark pill so it reads over any picture: which picture a split
/// viewer's half shows ("Source · match.mkv", "Program").
pub fn caption(ui: &Ui, theme: &Theme, frame: Rect, text: &str) {
    if text.is_empty() {
        return;
    }
    let painter = ui.painter_at(frame);
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(11.0), theme.text_secondary);
    let pill = Rect::from_min_size(frame.min + vec2(6.0, 6.0), galley.size() + vec2(12.0, 6.0));
    painter.rect_filled(pill, pill.height() / 2.0, theme.viewport.gamma_multiply(0.85));
    painter.galley(pill.min + vec2(6.0, 3.0), galley, theme.text_secondary);
}

/// The viewer's place with no picture in it, filling `size`: the viewer's
/// background and a quiet line in the middle (for example "Select a clip",
/// or where the picture went). Returns the area and its response.
pub fn empty(ui: &mut Ui, theme: &Theme, size: Vec2, message: &str) -> (Rect, egui::Response) {
    let (area, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 6.0, theme.viewport);
    painter.text(
        area.center(),
        Align2::CENTER_CENTER,
        message,
        FontId::proportional(13.0),
        theme.muted,
    );
    (area, response)
}

/// What the Display menu switches, as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Display {
    /// Source beside Program.
    pub split: bool,
    /// The picture is in a window of its own.
    pub own_window: bool,
    /// The picture fills the screen.
    pub full_screen: bool,
    /// The timeline's tracks are thin strips.
    pub compact_tracks: bool,
    /// The timeline has its divider between video and audio tracks.
    pub track_divider: bool,
    /// A click in the timeline's ruler moves the edit needle.
    pub click_moves_needle: bool,
}

/// The Display menu's lines: the command each runs, its words, and whether
/// it is on.
pub fn display_lines(display: Display) -> [(Action, &'static str, bool); 3] {
    [
        (Action::SplitView, "Split view", display.split),
        (
            Action::ViewerWindow,
            "Viewer in its own window",
            display.own_window,
        ),
        (Action::FullScreenViewer, "Full screen", display.full_screen),
    ]
}

/// The Display menu's lines under its Timeline heading: how the timeline's
/// tracks are looked at.
pub fn timeline_lines(display: Display) -> [(Action, &'static str, bool); 3] {
    [
        (Action::CompactTracks, "Compact tracks", display.compact_tracks),
        (
            Action::TrackDivider,
            "Divider between video and audio",
            display.track_divider,
        ),
        (
            Action::ClickMovesNeedle,
            "Clicking the timeline moves the edit needle",
            display.click_moves_needle,
        ),
    ]
}

/// The Display menu: a small button in the viewer's bottom row that opens
/// the ways to show the picture and, under a Timeline heading, the ways to
/// look at the timeline's tracks, each with the keys it has in
/// `shortcuts`. Returns the command picked; the page runs it like the same
/// command from the keyboard or search.
pub fn display_menu(ui: &mut Ui, display: Display, shortcuts: &ShortcutTable) -> Option<Action> {
    let mut picked = None;
    let mut line = |ui: &mut Ui, (action, words, on): (Action, &str, bool)| {
        let line = egui::Button::selectable(on, words).shortcut_text(shortcuts.keys_for(action));
        if ui.add(line).on_hover_text(action.label()).clicked() {
            picked = Some(action);
            ui.close();
        }
    };
    ui.menu_button("Display", |ui| {
        ui.set_min_width(260.0);
        for each in display_lines(display) {
            line(ui, each);
        }
        ui.separator();
        ui.label(egui::RichText::new("Timeline").small().weak());
        for each in timeline_lines(display) {
            line(ui, each);
        }
    })
    .response
    .on_hover_text("How the picture is shown (split, in its own window, full screen), how the timeline's tracks are looked at and what a click on it moves");
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

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
    fn the_display_menu_runs_the_commands_and_shows_what_is_on() {
        let lines = display_lines(Display {
            split: true,
            ..Display::default()
        });
        let actions: Vec<Action> = lines.iter().map(|(a, _, _)| *a).collect();
        assert_eq!(
            actions,
            [Action::SplitView, Action::ViewerWindow, Action::FullScreenViewer]
        );
        assert_eq!(lines.map(|(_, _, on)| on), [true, false, false]);
        let timeline = timeline_lines(Display {
            compact_tracks: true,
            track_divider: false,
            click_moves_needle: true,
            ..Display::default()
        });
        assert_eq!(
            timeline.map(|(a, _, on)| (a, on)),
            [
                (Action::CompactTracks, true),
                (Action::TrackDivider, false),
                (Action::ClickMovesNeedle, true)
            ]
        );
        // Each line's words are in its command's name, so the menu, search
        // and the shortcuts page say the same thing.
        for (action, words, _) in lines.into_iter().chain(timeline) {
            assert!(
                action.label().to_lowercase().contains(&words.to_lowercase()),
                "{words} / {}",
                action.label()
            );
        }
    }

    #[test]
    fn the_zoom_menu_offers_fit_the_usual_scales_and_the_commands() {
        let lines = zoom_lines();
        let words: Vec<&str> = lines.iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(words, ["Fit", "50 %", "100 %", "200 %", "Zoom in", "Zoom out"]);
        assert_eq!(lines[0].0, ZoomPick::Command(Action::ViewerZoomFit));
        assert_eq!(lines[1].0, ZoomPick::Scale(0.5));
        assert_eq!(lines[2].0, ZoomPick::Command(Action::ViewerZoom100));
        assert_eq!(lines[4].0, ZoomPick::Command(Action::ViewerZoomIn));
        assert_eq!(lines[5].0, ZoomPick::Command(Action::ViewerZoomOut));
    }

    #[test]
    fn what_floats_over_a_zoomed_picture_stays_inside_the_viewer() {
        let viewer = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 450.0));
        let zoomed = Zoom::at(2.0).frame_rect(viewer.shrink(MARGIN), (1920, 1080), 1.0);
        assert!(zoomed.width() > viewer.width());
        assert_eq!(seen(zoomed, viewer), viewer.shrink(MARGIN));
        let fitted = Zoom::FIT.frame_rect(viewer.shrink(MARGIN), (1920, 1080), 1.0);
        assert_eq!(seen(fitted, viewer), fitted);
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
