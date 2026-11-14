//! The transport: a low, rounded bar floating at the bottom of the viewer,
//! with play/pause at the exact horizontal centre. The same on every page
//! that has a viewer.

use crate::Theme;
use crate::shortcuts::{Action, ShortcutTable};
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// A button pressed on the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportClick {
    MarkIn,
    MarkOut,
    GoToStart,
    FrameBack,
    PlayPause,
    FrameForward,
    GoToEnd,
    Stop,
    Loop,
}

impl TransportClick {
    /// The command the button gives: the same as its keys.
    pub fn action(self) -> Action {
        match self {
            TransportClick::MarkIn => Action::MarkIn,
            TransportClick::MarkOut => Action::MarkOut,
            TransportClick::GoToStart => Action::GoToStart,
            TransportClick::FrameBack => Action::PreviousFrame,
            TransportClick::PlayPause => Action::PlayPause,
            TransportClick::FrameForward => Action::NextFrame,
            TransportClick::GoToEnd => Action::GoToEnd,
            TransportClick::Stop => Action::Stop,
            TransportClick::Loop => Action::ToggleLoop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    MarkIn,
    MarkOut,
    GoToStart,
    FrameBack,
    Play,
    Pause,
    FrameForward,
    GoToEnd,
    Stop,
    Loop,
}

const BUTTON: f32 = 34.0;
const GAP: f32 = 6.0;
const HEIGHT: f32 = 44.0;
/// Buttons each side of play, so play stays centred.
const EACH_SIDE: usize = 4;
/// Room kept between the bar and the viewer's sides.
const MARGIN: f32 = 12.0;

/// What a button says when pointed at: its words, and the keys for it as
/// they are now, if it has any.
fn tip(words: &str, shortcuts: &ShortcutTable, action: Action) -> String {
    match shortcuts.shortcut_for(action) {
        Some(keys) => format!("{words} ({})", shortcuts.written(keys)),
        None => words.to_owned(),
    }
}

/// How much the bar is drawn at: all of it fits inside a viewer `width`
/// wide, buttons never smaller than two thirds.
fn scale_for(width: f32) -> f32 {
    let full = BUTTON * (2 * EACH_SIDE + 1) as f32 + GAP * (2 * EACH_SIDE) as f32 + 20.0;
    ((width - 2.0 * MARGIN) / full).clamp(2.0 / 3.0, 1.0)
}

/// Draws the transport over the bottom of `viewer`. Returns the button
/// clicked. The tooltips name the keys from `shortcuts`. `looping` lights
/// the loop button when on; `None` where the page has no looping (the
/// button is shown greyed, so the bar is the same everywhere).
///
/// Left of play: In, Out, go to the start, back one frame. Right: forward
/// one frame, go to the end, stop, loop.
pub fn transport(
    ui: &mut Ui,
    theme: &Theme,
    viewer: Rect,
    playing: bool,
    looping: Option<bool>,
    shortcuts: &ShortcutTable,
) -> Option<TransportClick> {
    let scale = scale_for(viewer.width());
    let (button, gap, height) = (BUTTON * scale, GAP * scale, HEIGHT * scale);
    let width = button * (2 * EACH_SIDE + 1) as f32 + gap * (2 * EACH_SIDE) as f32 + 20.0 * scale;
    let bar = Rect::from_center_size(
        pos2(viewer.center().x, viewer.bottom() - height / 2.0 - 14.0 * scale),
        vec2(width, height),
    );
    // The bar takes what happens on it: a press between its buttons, or a
    // drag begun on one, never reaches the picture under it.
    ui.interact(bar, ui.scope_id().with("transport-bar"), Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(bar, height / 2.0 - 4.0, theme.panel.gamma_multiply(0.92));
    painter.rect_stroke(
        bar,
        height / 2.0 - 4.0,
        Stroke::new(1.0, theme.line),
        egui::StrokeKind::Inside,
    );

    let row = Row {
        centre: bar.center(),
        button,
        gap,
        scale,
    };
    buttons_ui(ui, theme, row, playing, looping, shortcuts, "transport")
}

/// Height of the compact transport.
pub const COMPACT_HEIGHT: f32 = 24.0;

/// The same buttons, small and without a bar, in a row centred on `centre`
/// (the row under the viewer, when nothing is drawn over the picture).
pub fn compact(
    ui: &mut Ui,
    theme: &Theme,
    centre: Pos2,
    playing: bool,
    looping: Option<bool>,
    shortcuts: &ShortcutTable,
) -> Option<TransportClick> {
    let scale = COMPACT_HEIGHT / BUTTON;
    let row = Row {
        centre,
        button: COMPACT_HEIGHT,
        gap: GAP * scale,
        scale,
    };
    buttons_ui(ui, theme, row, playing, looping, shortcuts, "compact transport")
}

/// Where a row of the transport's buttons goes and how big.
#[derive(Debug, Clone, Copy)]
struct Row {
    centre: Pos2,
    button: f32,
    gap: f32,
    scale: f32,
}

/// The width of the compact transport's row.
pub fn compact_width() -> f32 {
    let scale = COMPACT_HEIGHT / BUTTON;
    COMPACT_HEIGHT * (2 * EACH_SIDE + 1) as f32 + GAP * scale * (2 * EACH_SIDE) as f32
}

/// The buttons, play in the middle of `row`. Returns the one clicked.
fn buttons_ui(
    ui: &mut Ui,
    theme: &Theme,
    row: Row,
    playing: bool,
    looping: Option<bool>,
    shortcuts: &ShortcutTable,
    name: &str,
) -> Option<TransportClick> {
    let Row {
        centre,
        button,
        gap,
        scale,
    } = row;
    // Slots from -4 to 4; play is slot 0, at the centre.
    let loop_words = match looping {
        Some(true) => "Loop inside the region: on",
        Some(false) => "Loop inside the region",
        None => "Loop: not on this page",
    };
    let buttons = [
        (
            -4.0,
            Icon::MarkIn,
            TransportClick::MarkIn,
            tip("Mark in", shortcuts, Action::MarkIn),
        ),
        (
            -3.0,
            Icon::MarkOut,
            TransportClick::MarkOut,
            tip("Mark out", shortcuts, Action::MarkOut),
        ),
        (
            -2.0,
            Icon::GoToStart,
            TransportClick::GoToStart,
            tip("Go to the start", shortcuts, Action::GoToStart),
        ),
        (
            -1.0,
            Icon::FrameBack,
            TransportClick::FrameBack,
            tip("Back one frame", shortcuts, Action::PreviousFrame),
        ),
        (
            0.0,
            if playing { Icon::Pause } else { Icon::Play },
            TransportClick::PlayPause,
            tip(
                if playing {
                    "Pause: back to where playback started"
                } else {
                    "Play"
                },
                shortcuts,
                Action::PlayPause,
            ),
        ),
        (
            1.0,
            Icon::FrameForward,
            TransportClick::FrameForward,
            tip("Forward one frame", shortcuts, Action::NextFrame),
        ),
        (
            2.0,
            Icon::GoToEnd,
            TransportClick::GoToEnd,
            tip("Go to the end", shortcuts, Action::GoToEnd),
        ),
        (
            3.0,
            Icon::Stop,
            TransportClick::Stop,
            tip("Stop: leave the playhead here", shortcuts, Action::Stop),
        ),
        (
            4.0,
            Icon::Loop,
            TransportClick::Loop,
            tip(loop_words, shortcuts, Action::ToggleLoop),
        ),
    ];
    let mut clicked = None;
    for (i, (slot, icon, click, tip)) in buttons.iter().enumerate() {
        let x = centre.x + slot * (button + gap);
        let rect = Rect::from_center_size(pos2(x, centre.y), vec2(button, button));
        let id = ui.scope_id().with((name, i));
        let enabled = *click != TransportClick::Loop || looping.is_some();
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let response = ui.interact(rect, id, sense).on_hover_text(tip.as_str());
        let main = *click == TransportClick::PlayPause;
        let lit = *click == TransportClick::Loop && looping == Some(true);
        let fill = if main {
            theme.accent
        } else if lit {
            theme.accent.gamma_multiply(0.35)
        } else if response.hovered() && enabled {
            theme.line
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, button / 2.0, fill);
        let colour = if main {
            theme.accent_ink
        } else if !enabled {
            theme.muted.gamma_multiply(0.6)
        } else {
            theme.text
        };
        draw(ui, *icon, rect.center(), colour, scale.max(0.8));
        if response.clicked() {
            clicked = Some(*click);
        }
    }
    clicked
}

fn draw(ui: &Ui, icon: Icon, c: Pos2, colour: Color32, k: f32) {
    let p = ui.painter();
    let tri = |tip_right: bool, x: f32, size: f32| {
        let size = size * k;
        let (l, r) = if tip_right {
            (x - size * 0.5, x + size * 0.5)
        } else {
            (x + size * 0.5, x - size * 0.5)
        };
        Shape::convex_polygon(
            vec![pos2(l, c.y - size * 0.6), pos2(r, c.y), pos2(l, c.y + size * 0.6)],
            colour,
            Stroke::NONE,
        )
    };
    let bar = |x: f32, h: f32| {
        p.rect_filled(
            Rect::from_center_size(pos2(x, c.y), vec2(2.0 * k, h * k)),
            0.0,
            colour,
        );
    };
    let line = Stroke::new(2.0 * k, colour);
    match icon {
        Icon::Play => {
            p.add(tri(true, c.x + 1.5 * k, 12.0));
        }
        Icon::Pause => {
            for dx in [-3.5, 3.5] {
                p.rect_filled(
                    Rect::from_center_size(pos2(c.x + dx * k, c.y), vec2(3.5 * k, 13.0 * k)),
                    1.0,
                    colour,
                );
            }
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(11.0 * k, 11.0 * k)), 2.0, colour);
        }
        Icon::FrameBack => {
            bar(c.x - 5.0 * k, 12.0);
            p.add(tri(false, c.x + 1.5 * k, 9.0));
        }
        Icon::FrameForward => {
            p.add(tri(true, c.x - 1.5 * k, 9.0));
            bar(c.x + 5.0 * k, 12.0);
        }
        Icon::GoToStart => {
            bar(c.x - 7.0 * k, 12.0);
            p.add(tri(false, c.x - 1.5 * k, 8.0));
            p.add(tri(false, c.x + 5.0 * k, 8.0));
        }
        Icon::GoToEnd => {
            p.add(tri(true, c.x - 5.0 * k, 8.0));
            p.add(tri(true, c.x + 1.5 * k, 8.0));
            bar(c.x + 7.0 * k, 12.0);
        }
        // A bracket opening to the right: where a part begins.
        Icon::MarkIn | Icon::MarkOut => {
            let dir = if icon == Icon::MarkIn { 1.0 } else { -1.0 };
            let x = c.x - dir * 3.0 * k;
            let (top, bottom) = (c.y - 7.0 * k, c.y + 7.0 * k);
            p.line_segment([pos2(x, top), pos2(x, bottom)], line);
            p.line_segment([pos2(x, top), pos2(x + dir * 6.0 * k, top)], line);
            p.line_segment([pos2(x, bottom), pos2(x + dir * 6.0 * k, bottom)], line);
        }
        // Two arcs chasing each other round.
        Icon::Loop => {
            let r = 6.5 * k;
            for half in [0.0f32, std::f32::consts::PI] {
                let points: Vec<Pos2> = (0..=10)
                    .map(|i| {
                        let a = half + 0.35 + (std::f32::consts::PI - 0.9) * i as f32 / 10.0;
                        pos2(c.x + r * a.cos(), c.y + r * a.sin())
                    })
                    .collect();
                let end = points[points.len() - 1];
                let a = half + 0.35 + std::f32::consts::PI - 0.9;
                // The head points along the arc, onwards.
                let (along, out) = (vec2(-a.sin(), a.cos()), vec2(a.cos(), a.sin()));
                p.add(Shape::line(points, line));
                p.add(Shape::convex_polygon(
                    vec![
                        end + along * 3.5 * k,
                        end + out * 3.0 * k - along * 1.0 * k,
                        end - out * 3.0 * k - along * 1.0 * k,
                    ],
                    colour,
                    Stroke::NONE,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shortcut;

    /// One frame of the compact row centred at (300, 20), with `events`.
    fn compact_frame(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<TransportClick> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 40.0))),
            events,
            ..Default::default()
        };
        let mut clicked = None;
        let output = ctx.run_ui(input, |ui| {
            let table = ShortcutTable::default();
            clicked = compact(ui, &Theme::ASH, pos2(300.0, 20.0), false, Some(false), &table);
        });
        // Nothing draws these frames.
        output.drop_without_applying_deltas();
        clicked
    }

    #[test]
    fn the_compact_row_has_play_in_its_middle_and_the_rest_each_side() {
        let ctx = egui::Context::default();
        let press = |at: Pos2, pressed: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let click = |at: Pos2| {
            compact_frame(&ctx, vec![egui::Event::PointerMoved(at)]);
            compact_frame(&ctx, vec![press(at, true)]);
            compact_frame(&ctx, vec![press(at, false)])
        };
        compact_frame(&ctx, Vec::new());
        assert_eq!(click(pos2(300.0, 20.0)), Some(TransportClick::PlayPause));
        let step = COMPACT_HEIGHT + GAP * COMPACT_HEIGHT / BUTTON;
        assert_eq!(
            click(pos2(300.0 - 4.0 * step, 20.0)),
            Some(TransportClick::MarkIn)
        );
        assert_eq!(click(pos2(300.0 + 4.0 * step, 20.0)), Some(TransportClick::Loop));
        assert!((compact_width() - (9.0 * COMPACT_HEIGHT + 8.0 * (step - COMPACT_HEIGHT))).abs() < 0.01);
    }

    #[test]
    fn tooltips_name_the_keys_in_use() {
        let mut table = ShortcutTable::default();
        assert_eq!(tip("Play", &table, Action::PlayPause), "Play (Space)");
        assert_eq!(
            tip("Stop: leave the playhead here", &table, Action::Stop),
            "Stop: leave the playhead here (Ctrl+Space)"
        );
        assert_eq!(
            tip("Mark in", &table, Action::NextKeyframe),
            "Mark in (Alt+Right)",
            "the keys are those of the action asked about"
        );
        let play = table
            .lines()
            .iter()
            .position(|b| b.action == Action::PlayPause)
            .unwrap();
        table.swap(play, Shortcut::key(egui::Key::K));
        assert_eq!(tip("Play", &table, Action::PlayPause), "Play (K)");
    }

    #[test]
    fn every_button_gives_the_command_its_keys_give() {
        assert_eq!(TransportClick::Stop.action(), Action::Stop);
        assert_eq!(TransportClick::Loop.action(), Action::ToggleLoop);
        assert_eq!(TransportClick::MarkIn.action(), Action::MarkIn);
        assert_eq!(TransportClick::GoToEnd.action(), Action::GoToEnd);
    }

    #[test]
    fn the_bar_shrinks_to_fit_a_narrow_viewer() {
        assert_eq!(scale_for(1_200.0), 1.0);
        let narrow = scale_for(330.0);
        assert!((2.0 / 3.0..1.0).contains(&narrow), "{narrow}");
        assert_eq!(scale_for(100.0), 2.0 / 3.0, "never smaller than two thirds");
    }
}
