//! A scripted pointer for automated runs (`--pointer`, `--drag-to`): it
//! puts the pointer somewhere, or drags from there to somewhere else and
//! keeps holding, so a screenshot can show what pointing at or dragging
//! something looks like on a machine with no mouse to move.

use egui::{Event, PointerButton, Pos2};

/// The frame the pointer appears on, the frame the button goes down, and
/// how many frames a drag takes to reach its end.
const APPEARS: u64 = 3;
const PRESSES: u64 = 6;
const DRAG_FRAMES: u64 = 6;

/// Where the pointer is put, and where it then drags to with the left button held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FakePointer {
    pub at: Pos2,
    pub drag_to: Option<Pos2>,
}

impl FakePointer {
    /// Reads "640,420".
    pub fn parse_pos(text: &str) -> Option<Pos2> {
        let (x, y) = text.split_once(',')?;
        Some(Pos2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
    }

    /// The events to add to frame number `frame` (counted from 0).
    pub fn events(&self, frame: u64) -> Vec<Event> {
        // Each place is told once: a pointer told where it is again and
        // again never counts as resting, and tooltips wait for that.
        match (frame, self.drag_to) {
            (APPEARS, _) => vec![Event::PointerMoved(self.at)],
            (PRESSES, Some(_)) => vec![Event::PointerButton {
                pos: self.at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            (f, Some(to)) if f > PRESSES && f <= PRESSES + DRAG_FRAMES => {
                let done = (f - PRESSES) as f32 / DRAG_FRAMES as f32;
                vec![Event::PointerMoved(self.at + (to - self.at) * done)]
            }
            _ => Vec::new(),
        }
    }

    /// Makes frame `frame`'s input follow the script: whatever the real
    /// pointer did is taken out (on a machine without a mouse the window
    /// system still reports one leaving), and the script's events go in.
    pub fn apply(&self, frame: u64, events: &mut Vec<Event>) {
        events.retain(|event| {
            !matches!(
                event,
                Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone
            )
        });
        events.extend(self.events(frame));
    }

    /// The first frame on which everything the script does has happened.
    pub fn settled_at(&self) -> u64 {
        match self.drag_to {
            Some(_) => PRESSES + DRAG_FRAMES + 2,
            None => APPEARS + 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn positions_are_read_from_two_numbers() {
        assert_eq!(FakePointer::parse_pos("640,420"), Some(pos2(640.0, 420.0)));
        assert_eq!(FakePointer::parse_pos(" 12.5 , 7 "), Some(pos2(12.5, 7.0)));
        assert_eq!(FakePointer::parse_pos("640"), None);
        assert_eq!(FakePointer::parse_pos("here,there"), None);
    }

    #[test]
    fn a_pointer_without_a_drag_rests_where_it_was_put() {
        let pointer = FakePointer {
            at: pos2(10.0, 20.0),
            drag_to: None,
        };
        assert!(pointer.events(0).is_empty());
        assert_eq!(pointer.events(APPEARS), [Event::PointerMoved(pos2(10.0, 20.0))]);
        assert!(
            (APPEARS + 1..50).all(|frame| pointer.events(frame).is_empty()),
            "told once, then it rests"
        );
    }

    #[test]
    fn the_real_pointer_is_left_out_while_the_script_runs() {
        let pointer = FakePointer {
            at: pos2(10.0, 20.0),
            drag_to: None,
        };
        let key = Event::Copy;
        let mut events = vec![
            Event::PointerGone,
            key.clone(),
            Event::PointerMoved(pos2(900.0, 900.0)),
        ];
        pointer.apply(APPEARS, &mut events);
        assert_eq!(events, [key, Event::PointerMoved(pos2(10.0, 20.0))]);
    }

    #[test]
    fn a_drag_presses_moves_and_keeps_holding() {
        let pointer = FakePointer {
            at: pos2(100.0, 100.0),
            drag_to: Some(pos2(100.0, 160.0)),
        };
        assert_eq!(pointer.events(APPEARS), [Event::PointerMoved(pos2(100.0, 100.0))]);
        assert!(matches!(
            pointer.events(PRESSES)[..],
            [Event::PointerButton { pressed: true, .. }]
        ));
        assert_eq!(
            pointer.events(PRESSES + DRAG_FRAMES / 2),
            [Event::PointerMoved(pos2(100.0, 130.0))],
            "half-way"
        );
        assert_eq!(
            pointer.events(PRESSES + DRAG_FRAMES),
            [Event::PointerMoved(pos2(100.0, 160.0))],
            "there"
        );
        assert!(
            (pointer.settled_at()..500).all(|frame| pointer.events(frame).is_empty()),
            "and it never lets go"
        );
    }
}
