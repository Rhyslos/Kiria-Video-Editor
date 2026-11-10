//! The transport: a low, rounded bar floating at the bottom of the viewer,
//! with play/pause at the exact horizontal centre. The same on every page
//! that has a viewer.

use crate::Theme;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// A button pressed on the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportClick {
    FrameBack,
    PlayPause,
    FrameForward,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    FrameBack,
    Play,
    Pause,
    FrameForward,
    Stop,
}

const BUTTON: f32 = 34.0;
const GAP: f32 = 6.0;
const HEIGHT: f32 = 44.0;

/// Draws the transport over the bottom of `viewer`. Returns the button clicked.
pub fn transport(ui: &mut Ui, theme: &Theme, viewer: Rect, playing: bool) -> Option<TransportClick> {
    // Two buttons each side of play keep play centred.
    let width = BUTTON * 5.0 + GAP * 4.0 + 20.0;
    let bar = Rect::from_center_size(
        pos2(viewer.center().x, viewer.bottom() - HEIGHT / 2.0 - 14.0),
        vec2(width, HEIGHT),
    );
    let painter = ui.painter();
    painter.rect_filled(bar, HEIGHT / 2.0 - 4.0, theme.panel.gamma_multiply(0.92));
    painter.rect_stroke(
        bar,
        HEIGHT / 2.0 - 4.0,
        Stroke::new(1.0, theme.line),
        egui::StrokeKind::Inside,
    );

    // Slots from -2 to 2; play is slot 0, at the centre. Slot -2 stays free
    // for the controls still to come (in, out, play backwards), so the bar
    // is symmetric.
    let buttons = [
        (
            -1.0,
            Icon::FrameBack,
            TransportClick::FrameBack,
            "Back one frame (←)",
        ),
        (
            0.0,
            if playing { Icon::Pause } else { Icon::Play },
            TransportClick::PlayPause,
            if playing {
                "Pause: back to where playback started (Space)"
            } else {
                "Play (Space)"
            },
        ),
        (
            1.0,
            Icon::FrameForward,
            TransportClick::FrameForward,
            "Forward one frame (→)",
        ),
        (
            2.0,
            Icon::Stop,
            TransportClick::Stop,
            "Stop: leave the playhead here",
        ),
    ];
    let mut clicked = None;
    for (i, (slot, icon, click, tip)) in buttons.iter().enumerate() {
        let x = bar.center().x + slot * (BUTTON + GAP);
        let rect = Rect::from_center_size(pos2(x, bar.center().y), vec2(BUTTON, BUTTON));
        let id = ui.scope_id().with(("transport", i));
        let response = ui.interact(rect, id, Sense::click()).on_hover_text(*tip);
        let main = *click == TransportClick::PlayPause;
        let fill = if main {
            theme.accent
        } else if response.hovered() {
            theme.line
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, BUTTON / 2.0, fill);
        let colour = if main { theme.accent_ink } else { theme.text };
        draw(ui, *icon, rect.center(), colour);
        if response.clicked() {
            clicked = Some(*click);
        }
    }
    clicked
}

fn draw(ui: &Ui, icon: Icon, c: Pos2, colour: Color32) {
    let p = ui.painter();
    let tri = |tip_right: bool, x: f32, size: f32| {
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
    match icon {
        Icon::Play => {
            p.add(tri(true, c.x + 1.5, 12.0));
        }
        Icon::Pause => {
            for dx in [-3.5, 3.5] {
                p.rect_filled(
                    Rect::from_center_size(pos2(c.x + dx, c.y), vec2(3.5, 13.0)),
                    1.0,
                    colour,
                );
            }
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(11.0, 11.0)), 2.0, colour);
        }
        Icon::FrameBack => {
            p.rect_filled(
                Rect::from_center_size(pos2(c.x - 5.0, c.y), vec2(2.0, 12.0)),
                0.0,
                colour,
            );
            p.add(tri(false, c.x + 1.5, 9.0));
        }
        Icon::FrameForward => {
            p.add(tri(true, c.x - 1.5, 9.0));
            p.rect_filled(
                Rect::from_center_size(pos2(c.x + 5.0, c.y), vec2(2.0, 12.0)),
                0.0,
                colour,
            );
        }
    }
}
