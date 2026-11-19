//! A small pan knob: where a track sits between the speakers, from all
//! left to all right, in an audio track's header.
//!
//! Centred straight up; the arc grows from the top to the side it leans.
//! Drag up or right to turn it right, down or left to turn it left (Shift
//! for finer steps); it snaps to the centre on the way through. A
//! double-click centres it. While dragged the value is shown
//! ([`PanOut::shown`]); let go, it is one change ([`PanOut::done`]).

use crate::Theme;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// The angle at all left and all right, degrees from 12 o'clock.
const REACH: f32 = 135.0;
/// Pixels of drag for the whole way from the centre to one side.
const DRAG_TO_SIDE: f32 = 60.0;
/// Within this of the centre a drag rests on it.
const SNAP: f32 = 0.04;

/// A pan knob to draw this frame.
#[derive(Debug, Clone, Copy)]
pub struct PanKnob<'a> {
    pub centre: Pos2,
    pub radius: f32,
    /// Keeps a drag between frames.
    pub id: egui::Id,
    /// -1 all left, 0 centred, 1 all right.
    pub pan: f32,
    /// The arc's colour.
    pub colour: Color32,
    pub tip: &'a str,
}

/// What a frame of a pan knob gives back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanOut {
    /// The pan to show now: being dragged, or as it is.
    pub shown: f32,
    /// Let go (or double-clicked) at a new pan: one change.
    pub done: Option<f32>,
}

/// Where a drag that has moved `by` (right and up positive) from `from`
/// lands: clamped, and resting on the centre near it.
pub fn dragged(from: f32, by: f32, fine: bool) -> f32 {
    let scale = if fine { 0.2 } else { 1.0 };
    let to = (from + by * scale / DRAG_TO_SIDE).clamp(-1.0, 1.0);
    if to.abs() < SNAP { 0.0 } else { to }
}

/// How a pan reads: "Centre", "30 % left", "All right".
pub fn words(pan: f32) -> String {
    let share = (pan.abs() * 100.0).round();
    match (share, pan < 0.0) {
        (0.0, _) => "Centre".to_owned(),
        (s, true) if s >= 100.0 => "All left".to_owned(),
        (s, false) if s >= 100.0 => "All right".to_owned(),
        (s, true) => format!("{s:.0} % left"),
        (s, false) => format!("{s:.0} % right"),
    }
}

/// Draws a pan knob and turns it as the pointer says.
pub fn pan_knob(ui: &mut Ui, theme: &Theme, knob: PanKnob) -> PanOut {
    let PanKnob {
        centre,
        radius,
        id,
        pan,
        colour,
        tip,
    } = knob;
    let grab = Rect::from_center_size(centre, Vec2::splat(radius * 2.0 + 6.0));
    let r = ui
        .interact(grab, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text(format!("{tip}: {}", words(pan)));
    let mut drag: Option<f32> = ui.data(|d| d.get_temp(id));
    let fine = ui.input(|i| i.modifiers.shift);
    let mut done = None;
    if r.drag_started() {
        drag = Some(pan);
    }
    if let Some(from) = drag
        && r.dragged()
    {
        let d = r.drag_delta();
        // Unrounded, so slow moves add up; the snap only shows.
        drag = Some((from + (d.x - d.y) * if fine { 0.2 } else { 1.0 } / DRAG_TO_SIDE).clamp(-1.0, 1.0));
    }
    let settle = |p: f32| if p.abs() < SNAP { 0.0 } else { p };
    if r.drag_stopped()
        && let Some(value) = drag.take()
    {
        let value = settle(value);
        if value != pan {
            done = Some(value);
        }
    }
    if r.double_clicked() {
        drag = None;
        done = (pan != 0.0).then_some(0.0);
    }
    match drag {
        Some(v) => ui.data_mut(|d| {
            d.insert_temp(id, v);
        }),
        None => ui.data_mut(|d| d.remove::<f32>(id)),
    }
    let shown = drag.map(settle).or(done).unwrap_or(pan);
    draw(ui.painter(), theme, centre, radius, shown, colour, r.hovered());
    PanOut { shown, done }
}

/// The point `r` from `c` at `degrees` clockwise from 12 o'clock.
fn polar(c: Pos2, r: f32, degrees: f32) -> Pos2 {
    let a = (degrees - 90.0).to_radians();
    c + egui::vec2(r * a.cos(), r * a.sin())
}

fn draw(p: &egui::Painter, theme: &Theme, c: Pos2, r: f32, pan: f32, colour: Color32, lit: bool) {
    p.circle_filled(c, r, if lit { theme.line_strong } else { theme.line });
    let ring = |from: f32, to: f32| -> Vec<Pos2> {
        let steps = ((to - from).abs() / 8.0).ceil().max(1.0) as usize;
        (0..=steps)
            .map(|i| polar(c, r + 2.0, from + (to - from) * i as f32 / steps as f32))
            .collect()
    };
    p.add(egui::Shape::line(
        ring(-REACH, REACH),
        Stroke::new(1.0, theme.faint),
    ));
    let angle = pan * REACH;
    if pan != 0.0 {
        p.add(egui::Shape::line(ring(0.0, angle), Stroke::new(2.0, colour)));
    }
    // The centre mark, and the pointer.
    p.line_segment(
        [polar(c, r + 1.0, 0.0), polar(c, r + 4.0, 0.0)],
        Stroke::new(1.0, theme.muted),
    );
    p.line_segment(
        [polar(c, r * 0.25, angle), polar(c, r - 1.0, angle)],
        Stroke::new(1.6, theme.text),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_rests_on_the_centre_and_stops_at_the_sides() {
        assert_eq!(dragged(0.0, 30.0, false), 0.5);
        assert_eq!(dragged(0.5, -29.0, false), 0.0, "close to the centre rests on it");
        assert_eq!(dragged(0.0, -500.0, false), -1.0);
        assert_eq!(dragged(0.0, 30.0, true), 0.1, "Shift: a fifth as far");
    }

    #[test]
    fn a_pan_reads_in_words() {
        assert_eq!(words(0.0), "Centre");
        assert_eq!(words(-0.3), "30 % left");
        assert_eq!(words(1.0), "All right");
        assert_eq!(words(-1.0), "All left");
    }

    #[test]
    fn a_double_click_centres_it() {
        let ctx = egui::Context::default();
        let centre = egui::pos2(50.0, 50.0);
        let mut out = None;
        let click = |pressed| egui::Event::PointerButton {
            pos: centre,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let frames = [
            vec![egui::Event::PointerMoved(centre)],
            vec![click(true)],
            vec![click(false)],
            vec![click(true)],
            vec![click(false)],
        ];
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let knob = PanKnob {
                    centre,
                    radius: 7.0,
                    id: egui::Id::unique("pan"),
                    pan: -0.4,
                    colour: Color32::LIGHT_BLUE,
                    tip: "Pan",
                };
                let o = pan_knob(ui, &Theme::CARBON, knob);
                if o.done.is_some() {
                    out = o.done;
                }
            });
            output.drop_without_applying_deltas();
        }
        assert_eq!(out, Some(0.0));
    }
}
