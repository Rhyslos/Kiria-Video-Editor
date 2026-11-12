//! Keyframe diamonds: the one shape that says where a value changes over
//! time, in the inspector beside each value that can and on the timeline
//! along the selected clip.
//!
//! A hollow diamond: the value doesn't change over time. Filled: a
//! keyframe is right here, at the playhead. Hollow with a dot: the value
//! changes over time, and the playhead is between keyframes.

use crate::Theme;
use egui::{Color32, CursorIcon, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// Where a value's keyframes stand at the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyState {
    /// It has no keyframes: one value for the whole clip.
    #[default]
    Still,
    /// It changes over time; no keyframe at the playhead.
    Between,
    /// A keyframe is at the playhead.
    At,
}

/// What a value's keyframe control shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Keys<'a> {
    pub state: KeyState,
    /// There is a keyframe before / after the playhead to jump to.
    pub before: bool,
    pub after: bool,
    /// Keyframes can be added or removed here (the playhead is on the clip).
    pub can: bool,
    /// The ways the value can go on from a keyframe, offered by a
    /// right-click on a filled diamond, and which of them the one at the
    /// playhead has.
    pub eases: &'a [&'a str],
    pub ease: Option<usize>,
}

/// What was asked of a keyframe control this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyClicks {
    /// The diamond: add a keyframe at the playhead, or remove the one there.
    pub toggle: bool,
    /// The arrows: move the playhead to the previous or next keyframe.
    pub previous: bool,
    pub next: bool,
    /// One of `eases` was picked for the keyframe at the playhead.
    pub ease: Option<usize>,
}

impl KeyClicks {
    pub fn any(&self) -> bool {
        self.toggle || self.previous || self.next || self.ease.is_some()
    }
}

/// How wide the control is: an arrow, the diamond, an arrow.
pub const WIDTH: f32 = 40.0;

/// The diamond's corners around `centre`, `radius` from it.
fn corners(centre: Pos2, radius: f32) -> Vec<Pos2> {
    vec![
        pos2(centre.x, centre.y - radius),
        pos2(centre.x + radius, centre.y),
        pos2(centre.x, centre.y + radius),
        pos2(centre.x - radius, centre.y),
    ]
}

/// Draws a diamond in `state`, in `colour`.
pub fn paint(painter: &Painter, centre: Pos2, radius: f32, state: KeyState, colour: Color32) {
    let edge = Stroke::new(1.3, colour);
    match state {
        KeyState::At => {
            painter.add(Shape::convex_polygon(corners(centre, radius), colour, edge));
        }
        KeyState::Between => {
            painter.add(Shape::closed_line(corners(centre, radius), edge));
            painter.circle_filled(centre, radius * 0.32, colour);
        }
        KeyState::Still => {
            painter.add(Shape::closed_line(corners(centre, radius), edge));
        }
    }
}

/// A mark on a clip on the timeline: a small filled diamond with a dark
/// rim, so it reads over pictures and waveforms.
pub fn paint_mark(painter: &Painter, centre: Pos2, radius: f32, colour: Color32) {
    painter.add(Shape::convex_polygon(
        corners(centre, radius),
        colour,
        Stroke::new(1.0, Color32::from_black_alpha(200)),
    ));
}

/// An arrow pointing left or right, for jumping between keyframes.
fn paint_arrow(painter: &Painter, centre: Pos2, left: bool, colour: Color32) {
    let (w, h) = (3.5, 4.0);
    let (tip, back) = if left { (-w, w) } else { (w, -w) };
    painter.add(Shape::convex_polygon(
        vec![
            pos2(centre.x + tip, centre.y),
            pos2(centre.x + back, centre.y - h),
            pos2(centre.x + back, centre.y + h),
        ],
        colour,
        Stroke::NONE,
    ));
}

/// The control at the end of a row: ◀ ◆ ▶ for the value called `label`.
/// The arrows show only while the value changes over time.
pub fn control(ui: &mut Ui, theme: &Theme, label: &str, keys: &Keys) -> KeyClicks {
    let mut clicks = KeyClicks::default();
    let (cell, _) = ui.allocate_exact_size(vec2(WIDTH, 20.0), Sense::hover());
    let third = WIDTH / 3.0;
    let part = |i: f32| {
        Rect::from_min_size(
            pos2(cell.left() + third * i, cell.top()),
            vec2(third, cell.height()),
        )
    };
    let animated = keys.state != KeyState::Still;
    let lower = label.to_lowercase();

    // The diamond.
    let id = ui.make_persistent_id(("keyframe", label));
    let diamond = ui.interact(part(1.0), id.with("diamond"), Sense::click());
    let colour = match (keys.can, keys.state, diamond.hovered()) {
        (false, KeyState::Still, _) => theme.faint,
        (_, KeyState::Still, true) => theme.text,
        (_, KeyState::Still, false) => theme.muted,
        (_, _, true) => theme.text,
        (_, _, false) => theme.accent,
    };
    paint(ui.painter(), part(1.0).center(), 5.0, keys.state, colour);
    let tip = match (keys.can, keys.state) {
        (false, _) => "Move the playhead over the clip to add or remove keyframes".to_owned(),
        (true, KeyState::Still) => format!("Change {lower} over time: adds a keyframe at the playhead"),
        (true, KeyState::Between) => format!("Add a {lower} keyframe at the playhead"),
        (true, KeyState::At) => {
            format!("Remove the {lower} keyframe at the playhead (right-click: how it goes on to the next)")
        }
    };
    let diamond = diamond.on_hover_text(tip);
    if keys.can {
        diamond.clone().on_hover_cursor(CursorIcon::PointingHand);
        clicks.toggle = diamond.clicked();
    }
    if keys.state == KeyState::At && !keys.eases.is_empty() {
        diamond.context_menu(|ui| {
            ui.label(
                egui::RichText::new("From this keyframe on")
                    .color(theme.muted)
                    .size(12.0),
            );
            for (i, name) in keys.eases.iter().enumerate() {
                if ui.selectable_label(keys.ease == Some(i), *name).clicked() {
                    clicks.ease = Some(i);
                    ui.close();
                }
            }
        });
    }

    // The arrows either side.
    if animated {
        for (i, left, there) in [(0.0, true, keys.before), (2.0, false, keys.after)] {
            let r = part(i);
            let way = if left { "previous" } else { "next" };
            let arrow = ui.interact(r, id.with(way), Sense::click());
            let colour = match (there, arrow.hovered()) {
                (false, _) => theme.faint,
                (true, true) => theme.text,
                (true, false) => theme.text_secondary,
            };
            paint_arrow(ui.painter(), r.center(), left, colour);
            if there {
                let arrow = arrow
                    .on_hover_text(format!("Move the playhead to the {way} {lower} keyframe"))
                    .on_hover_cursor(CursorIcon::PointingHand);
                if arrow.clicked() {
                    if left {
                        clicks.previous = true;
                    } else {
                        clicks.next = true;
                    }
                }
            }
        }
    }
    clicks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_say_whether_anything_was_asked() {
        assert!(!KeyClicks::default().any());
        assert!(
            KeyClicks {
                ease: Some(1),
                ..KeyClicks::default()
            }
            .any()
        );
    }

    #[test]
    fn a_diamond_has_four_corners_around_its_centre() {
        let c = corners(pos2(10.0, 10.0), 4.0);
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|p| (p.x - 10.0).abs() + (p.y - 10.0).abs() == 4.0));
    }
}
