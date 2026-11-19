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
        draw_note(&painter, theme, area, frame, note);
    }
    (frame, response)
}

/// The quiet note in the viewer's bottom right corner. Beside the picture
/// it is plain faint text; where it would lie on the picture (a narrow
/// viewer, such as each half of the split view) it is cut to the
/// viewer's width and put on a dark pill like a caption, so it never mixes
/// with the picture under it.
fn draw_note(painter: &egui::Painter, theme: &Theme, area: Rect, frame: Rect, note: &str) {
    let mut job =
        egui::text::LayoutJob::simple_singleline(note.to_owned(), FontId::proportional(11.0), theme.faint);
    job.wrap = egui::text::TextWrapping::truncate_at_width((area.width() - 36.0).max(40.0));
    let galley = painter.layout_job(job);
    let corner = area.right_bottom() - vec2(12.0, 10.0);
    let place = Rect::from_min_max(corner - galley.size(), corner);
    if !place.intersects(frame) {
        painter.galley(place.min, galley, theme.faint);
        return;
    }
    let pill = place.expand2(vec2(6.0, 3.0));
    painter.rect_filled(pill, pill.height() / 2.0, theme.viewport.gamma_multiply(0.85));
    painter.galley(place.min, galley, theme.text_secondary);
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
    /// Zooming the timeline keeps the playhead in place.
    pub zoom_to_playhead: bool,
    /// Audio tracks have volume knobs instead of sliders.
    pub track_knobs: bool,
    /// Audio tracks have the detailed volume knob.
    pub detailed_gain: bool,
    /// The selected audio track has a column beside the master.
    pub track_column: bool,
    /// Audio track headers have a volume control.
    pub header_volume: bool,
    /// The timeline is in the rough cut view.
    pub rough_cut: bool,
    /// Cuts, splits and pastes happen at the playhead (no edit needle).
    pub edit_at_playhead: bool,
    /// Nothing is drawn over the picture: the play buttons are in the row
    /// under it, and handles, notes and captions are away.
    pub clean: bool,
}

/// One line of the Display menu: the command it runs, its words, whether
/// it is on (its box ticked), and whether it does anything now (greyed when
/// another line makes it moot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayLine {
    pub action: Action,
    pub words: &'static str,
    pub on: bool,
    pub enabled: bool,
}

/// A titled group of the Display menu's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayGroup {
    pub title: &'static str,
    pub lines: Vec<DisplayLine>,
}

/// The Display menu, group by group: how the picture is shown, how the
/// timeline looks, how edits are placed, and how audio tracks set their
/// volume.
pub fn display_groups(display: Display) -> Vec<DisplayGroup> {
    let line = |action, words, on| DisplayLine {
        action,
        words,
        on,
        enabled: true,
    };
    vec![
        DisplayGroup {
            title: "Picture",
            lines: vec![
                line(Action::SplitView, "Split view", display.split),
                line(
                    Action::ViewerWindow,
                    "Viewer in its own window",
                    display.own_window,
                ),
                line(Action::FullScreenViewer, "Full screen", display.full_screen),
                line(Action::CleanPicture, "Nothing over the picture", display.clean),
            ],
        },
        DisplayGroup {
            title: "Timeline",
            lines: vec![
                line(Action::RoughCut, "Rough cut", display.rough_cut),
                line(Action::CompactTracks, "Compact tracks", display.compact_tracks),
                line(
                    Action::TrackDivider,
                    "Divider between video and audio",
                    display.track_divider,
                ),
                line(
                    Action::ZoomToPlayhead,
                    "Zoom to playhead",
                    display.zoom_to_playhead,
                ),
            ],
        },
        DisplayGroup {
            title: "Editing",
            lines: vec![
                line(
                    Action::ClickMovesNeedle,
                    "Clicking the timeline moves the edit needle",
                    display.click_moves_needle,
                ),
                line(
                    Action::EditAtPlayhead,
                    "Edit at the playhead",
                    display.edit_at_playhead,
                ),
            ],
        },
        DisplayGroup {
            title: "Audio tracks",
            lines: vec![
                line(
                    Action::HeaderVolume,
                    "Volume in the track headers",
                    display.header_volume,
                ),
                // Which control the headers have: moot without one.
                DisplayLine {
                    enabled: display.header_volume,
                    ..line(Action::TrackKnobs, "Volume knobs", display.track_knobs)
                },
                DisplayLine {
                    enabled: display.header_volume,
                    ..line(Action::DetailedGain, "Detailed audio gain", display.detailed_gain)
                },
                line(Action::TrackColumn, "Selected track column", display.track_column),
            ],
        },
    ]
}

/// How wide the Display menu's lines are.
const MENU_WIDTH: f32 = 330.0;

/// Lines that change where the picture is: the menu closes after them.
fn closes_the_menu(action: Action) -> bool {
    matches!(action, Action::ViewerWindow | Action::FullScreenViewer)
}

/// One line of the menu as a checkbox: its box, its words and its keys on
/// the right. True when clicked.
fn checkbox_line(ui: &mut Ui, line: DisplayLine, keys: &str) -> bool {
    let height = 22.0;
    let width = MENU_WIDTH;
    let (rect, r) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click());
    let r = r.on_hover_text(line.action.label());
    let visuals = ui.visuals().clone();
    let p = ui.painter();
    if r.hovered() && line.enabled {
        p.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let text = if line.enabled {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let check =
        egui::Rect::from_center_size(egui::pos2(rect.left() + 13.0, rect.center().y), vec2(13.0, 13.0));
    if line.on {
        let fill = if line.enabled {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.inactive.bg_fill
        };
        p.rect_filled(check, 3.0, fill);
        let c = check.center();
        p.add(egui::Shape::line(
            vec![c + vec2(-3.5, 0.2), c + vec2(-1.0, 2.8), c + vec2(3.8, -2.8)],
            egui::Stroke::new(1.8, visuals.selection.stroke.color),
        ));
    } else {
        p.rect_stroke(
            check,
            3.0,
            egui::Stroke::new(1.2, visuals.widgets.inactive.fg_stroke.color.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    p.text(
        egui::pos2(check.right() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        line.words,
        font.clone(),
        text,
    );
    if !keys.is_empty() {
        p.text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            keys,
            font,
            visuals.weak_text_color(),
        );
    }
    r.clicked() && line.enabled
}

/// The Display menu: a small button in the viewer's bottom row that opens
/// the ways to show the picture and look at the timeline, in titled groups,
/// each a checkbox with the keys it has in `shortcuts`. Returns the command
/// picked; the page runs it like the same command from the keyboard or
/// search. The menu stays open to tick several, except after a line that
/// moves the picture.
pub fn display_menu(ui: &mut Ui, display: Display, shortcuts: &ShortcutTable) -> Option<Action> {
    let mut picked = None;
    // Clicks inside keep it open, so several lines can be ticked in turn.
    let config = egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (button, _) = egui::containers::menu::MenuButton::new("Display")
        .config(config)
        .ui(ui, |ui| {
            ui.set_width(MENU_WIDTH);
            ui.spacing_mut().item_spacing.y = 1.0;
            for (i, group) in display_groups(display).into_iter().enumerate() {
                if i > 0 {
                    ui.add_space(4.0);
                    ui.separator();
                }
                ui.add_space(2.0);
                ui.label(egui::RichText::new(group.title).small().strong().weak());
                for line in group.lines {
                    if checkbox_line(ui, line, &shortcuts.keys_for(line.action)) {
                        picked = Some(line.action);
                        if closes_the_menu(line.action) {
                            ui.close();
                        }
                    }
                }
            }
        });
    button.on_hover_text("How the picture is shown, how the timeline looks, where edits happen and how audio tracks set their volume");
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
    fn the_display_menu_is_in_groups_and_shows_what_is_on() {
        let groups = display_groups(Display {
            split: true,
            compact_tracks: true,
            click_moves_needle: true,
            detailed_gain: true,
            ..Display::default()
        });
        let titles: Vec<&str> = groups.iter().map(|g| g.title).collect();
        assert_eq!(titles, ["Picture", "Timeline", "Editing", "Audio tracks"]);
        let on: Vec<(Action, bool)> = groups
            .iter()
            .flat_map(|g| &g.lines)
            .map(|l| (l.action, l.on))
            .collect();
        assert_eq!(
            on,
            [
                (Action::SplitView, true),
                (Action::ViewerWindow, false),
                (Action::FullScreenViewer, false),
                (Action::CleanPicture, false),
                (Action::RoughCut, false),
                (Action::CompactTracks, true),
                (Action::TrackDivider, false),
                (Action::ZoomToPlayhead, false),
                (Action::ClickMovesNeedle, true),
                (Action::EditAtPlayhead, false),
                (Action::HeaderVolume, false),
                (Action::TrackKnobs, false),
                (Action::DetailedGain, true),
                (Action::TrackColumn, false),
            ]
        );
        // Without a volume control in the headers, which one is moot.
        let moot: Vec<Action> = groups
            .iter()
            .flat_map(|g| &g.lines)
            .filter(|l| !l.enabled)
            .map(|l| l.action)
            .collect();
        assert_eq!(moot, [Action::TrackKnobs, Action::DetailedGain]);
        // Each line's words are in its command's name, so the menu, search
        // and the shortcuts page say the same thing.
        for line in groups.iter().flat_map(|g| &g.lines) {
            assert!(
                line.action
                    .label()
                    .to_lowercase()
                    .contains(&line.words.to_lowercase()),
                "{} / {}",
                line.words,
                line.action.label()
            );
        }
        assert!(closes_the_menu(Action::FullScreenViewer) && !closes_the_menu(Action::CompactTracks));
    }

    #[test]
    fn ticking_a_line_keeps_the_display_menu_open() {
        let ctx = egui::Context::default();
        let table = ShortcutTable::default();
        let mut picked = Vec::new();
        let frame = |events: Vec<egui::Event>, picked: &mut Vec<Action>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 800.0))),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                if let Some(action) = display_menu(ui, Display::default(), &table) {
                    picked.push(action);
                }
            });
            output.drop_without_applying_deltas();
        };
        let click = |at: egui::Pos2| {
            vec![
                vec![egui::Event::PointerMoved(at)],
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            ]
        };
        frame(Vec::new(), &mut picked);
        // The Display button sits at the top left.
        for events in click(pos2(20.0, 10.0)) {
            frame(events, &mut picked);
        }
        frame(Vec::new(), &mut picked);
        assert!(egui::Popup::is_any_open(&ctx), "the menu opened");
        // Lines down the menu until one is ticked.
        for y in (40..400).step_by(6) {
            if !picked.is_empty() {
                break;
            }
            for events in click(pos2(80.0, y as f32)) {
                frame(events, &mut picked);
            }
        }
        frame(Vec::new(), &mut picked);
        assert!(!picked.is_empty(), "a line was ticked");
        if !closes_the_menu(picked[0]) {
            assert!(egui::Popup::is_any_open(&ctx), "still open after {:?}", picked[0]);
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
