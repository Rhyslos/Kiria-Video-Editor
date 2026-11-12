//! A scripted pointer for automated runs (`--pointer`, `--drag-to`): it
//! puts the pointer somewhere, or drags from there to somewhere else and
//! keeps holding, so a screenshot can show what pointing at or dragging
//! something looks like on a machine with no mouse to move. `--release`
//! lets go at the end of the drag, so what the drag did can be seen;
//! `--right-click` clicks the right button where the pointer was put, for
//! screenshots of menus, `--double-click` the left button twice and
//! `--click` once (a button, or a menu that opens on a click; with
//! `--scroll`, once the scrolling has settled, so a button scrolled into
//! view can be clicked).

use egui::{Event, PointerButton, Pos2};

/// The frame the pointer appears on, the frame the button goes down, and
/// how many frames a drag takes to reach its end.
const APPEARS: u64 = 3;
const PRESSES: u64 = 6;
const DRAG_FRAMES: u64 = 6;
/// How many frames scrolling glides for.
const GLIDES: u64 = 30;

/// Where the pointer is put, and where it then drags to with the left button held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FakePointer {
    pub at: Pos2,
    pub drag_to: Option<Pos2>,
    /// Let go of the button once the drag has arrived.
    pub release: bool,
    /// Click the right button at `at` (when nothing is dragged).
    pub right_click: bool,
    /// Click the left button twice at `at` (when nothing is dragged).
    pub double_click: bool,
    /// Points the wheel turns, down, with the pointer resting there.
    pub scroll: f32,
    /// Click the left button once at `at` (when nothing is dragged).
    pub click: bool,
}

impl FakePointer {
    /// Reads "640,420".
    pub fn parse_pos(text: &str) -> Option<Pos2> {
        let (x, y) = text.split_once(',')?;
        Some(Pos2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
    }

    /// A pointer that is put at `at` and does nothing else.
    pub fn resting(at: Pos2) -> FakePointer {
        FakePointer {
            at,
            drag_to: None,
            release: false,
            right_click: false,
            double_click: false,
            scroll: 0.0,
            click: false,
        }
    }

    /// The events to add to frame number `frame` (counted from 0).
    pub fn events(&self, frame: u64) -> Vec<Event> {
        let button = |pos, button, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Each place is told once: a pointer told where it is again and
        // again never counts as resting, and tooltips wait for that.
        match (frame, self.drag_to) {
            (APPEARS, _) => vec![Event::PointerMoved(self.at)],
            (PRESSES, Some(_)) => vec![button(self.at, PointerButton::Primary, true)],
            (f, Some(to)) if f > PRESSES && f <= PRESSES + DRAG_FRAMES => {
                let done = (f - PRESSES) as f32 / DRAG_FRAMES as f32;
                vec![Event::PointerMoved(self.at + (to - self.at) * done)]
            }
            (f, Some(to)) if self.release && f == PRESSES + DRAG_FRAMES + 2 => {
                vec![button(to, PointerButton::Primary, false)]
            }
            (PRESSES, None) if self.scroll != 0.0 => vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -self.scroll),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            }],
            (PRESSES, None) if self.right_click => vec![button(self.at, PointerButton::Secondary, true)],
            (f, None) if self.right_click && f == PRESSES + 1 => {
                vec![button(self.at, PointerButton::Secondary, false)]
            }
            // Down, up, down, up, a frame each.
            (f, None) if self.double_click && (PRESSES..PRESSES + 4).contains(&f) => {
                vec![button(
                    self.at,
                    PointerButton::Primary,
                    (f - PRESSES).is_multiple_of(2),
                )]
            }
            (f, None) if self.click && (self.clicks_at()..self.clicks_at() + 2).contains(&f) => {
                vec![button(self.at, PointerButton::Primary, f == self.clicks_at())]
            }
            _ => Vec::new(),
        }
    }

    /// The frame a click presses on: after the scrolling, when there is any.
    fn clicks_at(&self) -> u64 {
        if self.scroll != 0.0 {
            PRESSES + GLIDES
        } else {
            PRESSES
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
            Some(_) if self.release => PRESSES + DRAG_FRAMES + 4,
            Some(_) => PRESSES + DRAG_FRAMES + 2,
            None if self.right_click => PRESSES + 3,
            None if self.double_click => PRESSES + 5,
            // Scrolling glides for a while, then the click.
            None if self.scroll != 0.0 && self.click => PRESSES + GLIDES + 3,
            None if self.scroll != 0.0 => PRESSES + GLIDES,
            None if self.click => PRESSES + 3,
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
    fn the_wheel_turns_once_where_the_pointer_rests() {
        let pointer = FakePointer {
            scroll: 400.0,
            ..FakePointer::resting(pos2(10.0, 20.0))
        };
        let wheel: Vec<u64> = (0..60)
            .filter(|frame| {
                pointer
                    .events(*frame)
                    .iter()
                    .any(|e| matches!(e, Event::MouseWheel { delta, .. } if delta.y == -400.0))
            })
            .collect();
        assert_eq!(wheel, [PRESSES]);
        assert!(pointer.settled_at() > PRESSES);
    }

    #[test]
    fn a_pointer_without_a_drag_rests_where_it_was_put() {
        let pointer = FakePointer::resting(pos2(10.0, 20.0));
        assert!(pointer.events(0).is_empty());
        assert_eq!(pointer.events(APPEARS), [Event::PointerMoved(pos2(10.0, 20.0))]);
        assert!(
            (APPEARS + 1..50).all(|frame| pointer.events(frame).is_empty()),
            "told once, then it rests"
        );
    }

    #[test]
    fn the_real_pointer_is_left_out_while_the_script_runs() {
        let pointer = FakePointer::resting(pos2(10.0, 20.0));
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
            drag_to: Some(pos2(100.0, 160.0)),
            ..FakePointer::resting(pos2(100.0, 100.0))
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

    #[test]
    fn a_drag_that_is_released_lets_go_where_it_arrived() {
        let pointer = FakePointer {
            drag_to: Some(pos2(100.0, 160.0)),
            release: true,
            ..FakePointer::resting(pos2(100.0, 100.0))
        };
        let released: Vec<(u64, Event)> = (0..pointer.settled_at())
            .flat_map(|frame| pointer.events(frame).into_iter().map(move |e| (frame, e)))
            .filter(|(_, e)| matches!(e, Event::PointerButton { pressed: false, .. }))
            .collect();
        assert!(
            matches!(
                released[..],
                [(frame, Event::PointerButton { pos, button: PointerButton::Primary, .. })]
                    if pos == pos2(100.0, 160.0) && frame > PRESSES + DRAG_FRAMES
            ),
            "once, at the end, after it has arrived: {released:?}"
        );
        assert!((pointer.settled_at()..500).all(|frame| pointer.events(frame).is_empty()));
    }

    #[test]
    fn a_right_click_presses_and_lets_go_where_the_pointer_was_put() {
        let pointer = FakePointer {
            right_click: true,
            ..FakePointer::resting(pos2(40.0, 300.0))
        };
        let buttons: Vec<(PointerButton, bool)> = (0..pointer.settled_at())
            .flat_map(|frame| pointer.events(frame))
            .filter_map(|e| match e {
                Event::PointerButton {
                    pos, button, pressed, ..
                } if pos == pos2(40.0, 300.0) => Some((button, pressed)),
                _ => None,
            })
            .collect();
        assert_eq!(
            buttons,
            [
                (PointerButton::Secondary, true),
                (PointerButton::Secondary, false)
            ]
        );
        assert!((pointer.settled_at()..500).all(|frame| pointer.events(frame).is_empty()));
    }

    #[test]
    fn a_click_is_one_press_and_release_of_the_left_button() {
        let pointer = FakePointer {
            click: true,
            ..FakePointer::resting(pos2(40.0, 300.0))
        };
        let buttons: Vec<(PointerButton, bool)> = (0..pointer.settled_at())
            .flat_map(|frame| pointer.events(frame))
            .filter_map(|e| match e {
                Event::PointerButton { button, pressed, .. } => Some((button, pressed)),
                _ => None,
            })
            .collect();
        let primary = PointerButton::Primary;
        assert_eq!(buttons, [(primary, true), (primary, false)]);
        assert!((pointer.settled_at()..500).all(|frame| pointer.events(frame).is_empty()));
    }

    #[test]
    fn a_double_click_is_two_clicks_of_the_left_button() {
        let pointer = FakePointer {
            double_click: true,
            ..FakePointer::resting(pos2(40.0, 300.0))
        };
        let buttons: Vec<(PointerButton, bool)> = (0..pointer.settled_at())
            .flat_map(|frame| pointer.events(frame))
            .filter_map(|e| match e {
                Event::PointerButton { button, pressed, .. } => Some((button, pressed)),
                _ => None,
            })
            .collect();
        let primary = PointerButton::Primary;
        assert_eq!(
            buttons,
            [
                (primary, true),
                (primary, false),
                (primary, true),
                (primary, false)
            ]
        );
        assert!((pointer.settled_at()..500).all(|frame| pointer.events(frame).is_empty()));
    }

    #[test]
    fn a_click_after_scrolling_waits_for_the_scroll_to_settle() {
        let pointer = FakePointer {
            scroll: 300.0,
            click: true,
            ..FakePointer::resting(Pos2::new(5.0, 6.0))
        };
        assert!(matches!(pointer.events(PRESSES)[..], [Event::MouseWheel { .. }]));
        assert!(pointer.events(PRESSES + 1).is_empty());
        let press = pointer.events(PRESSES + GLIDES);
        assert!(matches!(press[..], [Event::PointerButton { pressed: true, .. }]));
        assert!(matches!(
            pointer.events(PRESSES + GLIDES + 1)[..],
            [Event::PointerButton { pressed: false, .. }]
        ));
        assert_eq!(pointer.settled_at(), PRESSES + GLIDES + 3);
    }
}
