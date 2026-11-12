//! Handles on the selected clip's picture in the Edit page's viewer: drag
//! the picture to move it, a corner to resize it, a side to crop it, the
//! turn handle to turn it, double-click it to fit it.
//!
//! The outline, the hit-testing, the snapping and the drag itself are
//! `ui_kit::picture_handles`, shared by every page with a viewer. This is
//! the Edit page's part: which clip it is (the one the inspector is about,
//! while the playhead is on it), what a drag means for its look ([`moved`],
//! [`resized`], [`turned`], [`cropped`]), what the viewer shows while a
//! side is dragged ([`uncropped`]: the whole picture, the handles veiling
//! what is cut off), and handing the change to the inspector's look in waiting,
//! so the panel's numbers follow the hand and letting go is one undo step.
//! A picture whose place or size changes over time is held as it is at the
//! playhead, and what the hand does there becomes keyframes at the playhead
//! for the numbers it changed.

use crate::inspector::{Inspector, Subject};
use project::look::ONE;
use project::{ClipId, Command, Look, LookField, Project, Time};
use ui_kit::Theme;
use ui_kit::picture_handles::{Area, Change, FrameMap, Handles, Part, Picture, Side};

#[derive(Debug, Default)]
pub struct PictureHandles {
    handles: Handles,
    /// The clip being dragged and its look when the drag began: what the
    /// drag's changes are counted from.
    from: Option<(ClipId, Look)>,
}

impl PictureHandles {
    /// Draws the handles over the viewer's frame (`on_screen`, in points)
    /// and lets the picture be taken hold of; `response` is the viewer's
    /// own. Returns a finished change: one undo step.
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &egui::Ui,
        theme: &Theme,
        on_screen: egui::Rect,
        response: &egui::Response,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
        inspector: &mut Inspector,
        snapping: bool,
    ) -> Option<Command> {
        let subject = inspector
            .subject(project, selected, playhead)
            .filter(|subject| subject.on_show_at(playhead));
        let Some(subject) = subject else {
            // Nothing to hold any more (the playhead left the clip, or
            // another was selected): what was being changed is finished.
            return self.drop_drag(project, inspector);
        };
        if self.from.is_some_and(|(clip, _)| clip != subject.clip) {
            let finished = self.drop_drag(project, inspector);
            if finished.is_some() {
                return finished;
            }
        }
        let map = FrameMap {
            on_screen,
            frame: subject.frame,
        };
        let picture = |look: &Look| picture(look, subject.picture, subject.frame);
        let handled = self
            .handles
            .interact(ui, response, map, picture(&subject.look), snapping);
        let mut look = subject.look;
        let mut finished = None;
        match handled.change {
            Some(Change::Fit) => {
                look = crate::inspector::fitted(&subject.look);
                finished = inspector.changed_outside(project, subject.clip, subject.at, look, false);
            }
            Some(change) => {
                let (_, from) = *self.from.get_or_insert((subject.clip, subject.look));
                look = changed(&from, change, &subject);
                finished = inspector.changed_outside(project, subject.clip, subject.at, look, handled.busy);
            }
            // Let go without having moved since the last frame.
            None if handled.ended && self.from.is_some() => finished = inspector.let_go(project),
            None => {}
        }
        if handled.ended {
            self.from = None;
        }
        if handled.change.is_some() || handled.ended {
            // The panel was drawn before this: its numbers follow in the next frame.
            ui.ctx().request_repaint();
        }
        self.handles.draw(ui, theme, response.rect, map, picture(&look));
        finished
    }

    /// The clip whose side is being dragged, if one is: the viewer shows
    /// its whole picture meanwhile ([`uncropped`]).
    pub fn cropping(&self) -> Option<ClipId> {
        self.from
            .filter(|_| self.handles.cropping())
            .map(|(clip, _)| clip)
    }

    /// Ends a drag whose picture is no longer there to hold.
    fn drop_drag(&mut self, project: &Project, inspector: &mut Inspector) -> Option<Command> {
        self.handles.let_go();
        self.from.take().and_then(|_| inspector.let_go(project))
    }
}

/// A clip's picture as the handles take it: where the whole of it is, the
/// part of it that is seen and its turn, for a picture of `picture` pixels
/// in a frame of `frame`.
fn picture(look: &Look, picture: (u32, u32), frame: (u32, u32)) -> Picture {
    let placement = engine::frame::placement(look, picture, frame);
    Picture {
        whole: Area::of_part(placement, Part::WHOLE),
        part: engine::frame::shown_part(look),
        degrees: f64::from(look.turn) / ONE_F,
    }
}

/// `project` with `clip` shown whole, as if nothing were cut off it: what
/// the viewer shows while one of its sides is dragged, so the part being
/// cut off or given back can be seen (the handles veil it). Only for
/// showing; `None` when the clip isn't there.
pub fn uncropped(project: &Project, clip: ClipId) -> Option<Project> {
    Command::ResetLook {
        sequence: project.active_sequence,
        clip,
        fields: CROP.to_vec(),
    }
    .apply(project)
    .ok()
}

const CROP: [LookField; 4] = [
    LookField::CropLeft,
    LookField::CropTop,
    LookField::CropRight,
    LookField::CropBottom,
];

/// `from` after `change`, a drag counted from where it began.
fn changed(from: &Look, change: Change, subject: &Subject) -> Look {
    match change {
        Change::Move { by } => moved(from, by, subject.frame),
        Change::Resize { factor, anchor } => resized(from, factor, anchor, subject.frame),
        Change::Turn { degrees, about } => turned(from, degrees, about, subject.frame),
        Change::Crop { side, cut } => cropped(from, side, cut),
        Change::Fit => crate::inspector::fitted(from),
    }
}

const ONE_F: f64 = ONE as f64;

/// A share of the frame as whole millionths, kept inside what a look's
/// numbers hold whatever the pointer did.
fn millionths(share: f64) -> i32 {
    (share * ONE_F)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn sides(frame: (u32, u32)) -> (f64, f64) {
    (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)))
}

/// `from` with the picture moved `by` frame pixels, in a frame of `frame`
/// pixels, within a look's limits. The turn doesn't come into it: right is
/// right in the frame however the picture is turned.
pub fn moved(from: &Look, by: (f32, f32), frame: (u32, u32)) -> Look {
    let (fw, fh) = sides(frame);
    Look {
        x: from.x.saturating_add(millionths(f64::from(by.0) / fw)),
        y: from.y.saturating_add(millionths(f64::from(by.1) / fh)),
        ..*from
    }
    .within_limits()
}

/// `from` with the picture `factor` times as large, the place `anchor`
/// (frame pixels) staying where it is. A look sizes a picture about its
/// centre, so the centre moves away from the anchor by the same factor;
/// that holds for a turned or cut picture as for any other. The size stays
/// within a look's limits, and the centre follows the size it really got.
pub fn resized(from: &Look, factor: f32, anchor: (f32, f32), frame: (u32, u32)) -> Look {
    let (fw, fh) = sides(frame);
    let scale = (f64::from(from.scale) * f64::from(factor))
        .round()
        .clamp(f64::from(Look::SCALE.0), f64::from(Look::SCALE.1));
    let factor = scale / f64::from(from.scale.max(1));
    // The centre, and the anchor, in shares of the frame from its centre.
    let centre = (f64::from(from.x) / ONE_F, f64::from(from.y) / ONE_F);
    let anchor = (f64::from(anchor.0) / fw - 0.5, f64::from(anchor.1) / fh - 0.5);
    Look {
        x: millionths(anchor.0 + factor * (centre.0 - anchor.0)),
        y: millionths(anchor.1 + factor * (centre.1 - anchor.1)),
        scale: scale as i32,
        ..*from
    }
    .within_limits()
}

/// `from` turned to `degrees` (clockwise; past a whole turn is kept as a
/// look keeps it) about the place `about` (frame pixels), which stays
/// where it is: a look turns a picture about its own centre, so the centre
/// goes round `about` by as much as the turn changes. For a picture with
/// nothing cut off `about` is its centre, and only the turn changes.
pub fn turned(from: &Look, degrees: f64, about: (f32, f32), frame: (u32, u32)) -> Look {
    let (fw, fh) = sides(frame);
    let turn = (degrees * ONE_F)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let by = (f64::from(turn) - f64::from(from.turn)) / ONE_F;
    let (sin, cos) = by.to_radians().sin_cos();
    // The centre, from `about`, in frame pixels.
    let centre = (
        fw * (0.5 + f64::from(from.x) / ONE_F),
        fh * (0.5 + f64::from(from.y) / ONE_F),
    );
    let (dx, dy) = (centre.0 - f64::from(about.0), centre.1 - f64::from(about.1));
    let step = (dx * cos - dy * sin - dx, dx * sin + dy * cos - dy);
    Look {
        x: from.x.saturating_add(millionths(step.0 / fw)),
        y: from.y.saturating_add(millionths(step.1 / fh)),
        turn,
        ..*from
    }
    .within_limits()
}

/// `from` with `cut` more of `side` cut off (a share of the picture's
/// width for left and right, of its height for top and bottom; under 0
/// gives some back): never less than nothing, and never so much that less
/// than a look's least is left beside what the opposite side cuts.
pub fn cropped(from: &Look, side: Side, cut: f32) -> Look {
    let mut look = *from;
    let (this, opposite) = match side {
        Side::Left => (&mut look.crop_left, from.crop_right),
        Side::Top => (&mut look.crop_top, from.crop_bottom),
        Side::Right => (&mut look.crop_right, from.crop_left),
        Side::Bottom => (&mut look.crop_bottom, from.crop_top),
    };
    let most = (Look::MOST_CROP - opposite).max(0);
    *this = this.saturating_add(millionths(f64::from(cut))).clamp(0, most);
    look.within_limits()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspector::tests::project_with_a_clip;
    use egui::{Rect, pos2, vec2};
    use ui_kit::picture_handles::Corner;

    const FRAME: (u32, u32) = (1920, 1080);
    const PICTURE: (u32, u32) = (1920, 1080);

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn a_move_is_counted_in_millionths_of_the_frame() {
        let from = Look {
            x: 100_000,
            y: -50_000,
            scale: 300_000,
            turn: 30 * ONE,
            opacity: 700_000,
            ..Look::PLAIN
        };
        // 192 pixels is a tenth of the width, 54 a twentieth of the height.
        let to = moved(&from, (192.0, -54.0), FRAME);
        assert_eq!((to.x, to.y), (200_000, -100_000));
        assert_eq!(
            Look {
                x: from.x,
                y: from.y,
                ..to
            },
            from,
            "nothing else changes, turned or not"
        );
        assert_eq!(moved(&from, (0.0, 0.0), FRAME), from, "no move is no change");
        // A fraction of a pixel still moves it, to the nearest millionth.
        assert_eq!(moved(&Look::PLAIN, (0.5, 0.0), FRAME).x, 260);
        // Never further than a look goes.
        assert_eq!(moved(&from, (1e9, -1e9), FRAME).x, Look::AWAY);
        assert_eq!(moved(&from, (1e9, -1e9), FRAME).y, -Look::AWAY);
    }

    #[test]
    fn a_resize_keeps_the_opposite_corner_where_it_is() {
        // A third of the size, towards the bottom right.
        let from = Look {
            scale: 300_000,
            x: 320_000,
            y: 280_000,
            ..Look::PLAIN
        };
        let area = |look: &Look| engine::frame::shown_area(look, PICTURE, FRAME);
        let before = area(&from);
        for corner in Corner::ALL {
            let anchor = corner.opposite().of(&before);
            for factor in [0.5, 1.5, 2.0] {
                let to = resized(&from, factor, anchor, FRAME);
                assert_eq!(to.scale, (300_000.0 * f64::from(factor)).round() as i32);
                let after = area(&to);
                assert!(
                    near(corner.opposite().of(&after), anchor),
                    "{corner:?} x{factor}: {after:?}"
                );
                let (w, h) = after.size();
                assert!(near((w, h), (576.0 * factor, 324.0 * factor)));
            }
        }
        // Once the size: nothing changes.
        assert_eq!(resized(&from, 1.0, Corner::TopLeft.of(&before), FRAME), from);
    }

    #[test]
    fn a_turned_and_cut_picture_keeps_its_opposite_corner_too() {
        let from = Look {
            scale: 400_000,
            x: -150_000,
            y: 100_000,
            turn: 30 * ONE,
            crop_left: 200_000,
            crop_bottom: 100_000,
            ..Look::PLAIN
        };
        let area = |look: &Look| engine::frame::shown_area(look, PICTURE, FRAME);
        let before = area(&from);
        let anchor = Corner::TopLeft.of(&before);
        let to = resized(&from, 1.75, anchor, FRAME);
        assert_eq!(to.scale, 700_000);
        assert_eq!(
            (to.turn, to.crop_left, to.crop_bottom),
            (from.turn, from.crop_left, from.crop_bottom)
        );
        let after = area(&to);
        assert!(near(Corner::TopLeft.of(&after), anchor), "{after:?}");
        // The dragged corner is 1.75 times as far from it, the same way.
        let (was, is) = (Corner::BottomRight.of(&before), Corner::BottomRight.of(&after));
        assert!(near(
            (is.0 - anchor.0, is.1 - anchor.1),
            ((was.0 - anchor.0) * 1.75, (was.1 - anchor.1) * 1.75)
        ));
    }

    #[test]
    fn a_resize_stops_at_a_looks_limits_and_the_corner_still_stays() {
        let from = Look {
            scale: 50_000,
            ..Look::PLAIN
        };
        let area = |look: &Look| engine::frame::shown_area(look, PICTURE, FRAME);
        let anchor = Corner::TopLeft.of(&area(&from));
        // A thousandth of 5 % would be far under the least size, 1 %.
        let to = resized(&from, 0.001, anchor, FRAME);
        assert_eq!(to.scale, Look::SCALE.0);
        assert!(near(Corner::TopLeft.of(&area(&to)), anchor));
        assert!(to.check().is_ok());
    }

    #[test]
    fn a_picture_snapped_to_the_centre_is_at_nought_exactly() {
        // What the snap hands over: the step from where the centre was.
        let from = Look {
            x: 123_457,
            y: -98_765,
            scale: 250_000,
            ..Look::PLAIN
        };
        let before = engine::frame::shown_area(&from, PICTURE, FRAME);
        let (by, _) = ui_kit::picture_handles::snap_move(&before, FRAME, 1e4);
        let to = moved(&from, by, FRAME);
        assert_eq!((to.x, to.y), (0, 0));
    }

    /// The Edit page's viewer for one frame without a window: a viewer 784
    /// by 448 points whose frame is 768 points wide from (8, 8), 0.4 points
    /// to a frame pixel, with the handles over it.
    struct Rig {
        ctx: egui::Context,
        project: Project,
        selected: Vec<ClipId>,
        playhead: Time,
        inspector: Inspector,
        handles: PictureHandles,
        modifiers: egui::Modifiers,
    }

    impl Rig {
        fn new() -> Rig {
            let project = project_with_a_clip();
            let clip = project.active().unwrap().video_tracks[0].clips[0].id;
            Rig {
                ctx: egui::Context::default(),
                project,
                selected: vec![clip],
                playhead: Time::seconds(1),
                inspector: Inspector::default(),
                handles: PictureHandles::default(),
                modifiers: egui::Modifiers::NONE,
            }
        }

        fn clip(&self) -> ClipId {
            self.project.active().unwrap().video_tracks[0].clips[0].id
        }

        fn stored(&self) -> Look {
            self.project.active().unwrap().video_tracks[0].clips[0].look
        }

        /// The look the viewer and the panel show.
        fn shown(&self) -> Look {
            self.inspector
                .subject(&self.project, &self.selected, self.playhead)
                .unwrap()
                .look
        }

        fn set(&mut self, look: Look) {
            self.project = Command::SetLook {
                sequence: self.project.active_sequence,
                clip: self.clip(),
                look,
            }
            .apply(&self.project)
            .unwrap();
        }

        /// One frame, the way the page draws it: the inspector first, then
        /// the viewer with the handles. Returns the undo steps it made,
        /// which are applied, as the page does.
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
            let events = std::iter::once(egui::Event::ModifiersChanged(self.modifiers))
                .chain(events)
                .collect();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1100.0, 600.0))),
                events,
                ..Default::default()
            };
            let theme = ui_kit::ThemeName::default().theme();
            let mut commands = Vec::new();
            let mut output = self.ctx.run_ui(input, |ui| {
                egui::Panel::right("inspector").exact_size(292.0).show(ui, |ui| {
                    commands = self
                        .inspector
                        .ui(ui, &theme, &self.project, &self.selected, self.playhead);
                });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                        let (frame, response) =
                            ui_kit::viewer::viewer(ui, &theme, vec2(784.0, 448.0), FRAME, None, "");
                        assert_eq!(frame.min, pos2(8.0, 8.0));
                        commands.extend(self.handles.ui(
                            ui,
                            &theme,
                            frame,
                            &response,
                            &self.project,
                            &self.selected,
                            self.playhead,
                            &mut self.inspector,
                            true,
                        ));
                    });
            });
            // No window to hand the fonts' texture to.
            output.textures_delta.clear();
            for command in &commands {
                self.project = command.clone().apply(&self.project).unwrap();
            }
            commands
        }

        fn move_to(&mut self, at: egui::Pos2) -> Vec<Command> {
            self.frame(vec![egui::Event::PointerMoved(at)])
        }

        fn button(&mut self, at: egui::Pos2, pressed: bool) -> Vec<Command> {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: self.modifiers,
            }])
        }
    }

    /// A quarter of the size, up and left of the centre: its picture is
    /// 480 by 270 pixels around (576, 324), which is (238.4, 137.6) on screen.
    const SMALL: Look = Look {
        scale: 250_000,
        x: -200_000,
        y: -200_000,
        ..Look::PLAIN
    };

    #[test]
    fn a_drag_on_the_picture_is_shown_at_once_and_is_one_undo_step_when_let_go() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        rig.modifiers = egui::Modifiers::ALT;
        let from = pos2(238.4, 137.6);
        assert!(rig.move_to(from).is_empty());
        assert!(rig.button(from, true).is_empty());
        // 76.8 points right is 192 pixels: a tenth of the frame's width.
        assert!(rig.move_to(pos2(280.0, 137.6)).is_empty());
        assert!(rig.move_to(pos2(315.2, 137.6)).is_empty());
        let moved = Look { x: -100_000, ..SMALL };
        assert_eq!(rig.shown(), moved, "the viewer and the panel follow the hand");
        assert_eq!(
            rig.inspector
                .shown(&rig.project)
                .unwrap()
                .active()
                .unwrap()
                .video_tracks[0]
                .clips[0]
                .look,
            moved
        );
        assert_eq!(rig.stored(), SMALL, "and it is no undo step yet");
        // Frames go by with the button held: still none.
        assert!(rig.frame(Vec::new()).is_empty());
        assert!(rig.frame(Vec::new()).is_empty());
        let steps = rig.button(pos2(315.2, 137.6), false);
        assert_eq!(
            steps,
            vec![Command::SetLookAt {
                sequence: rig.project.active_sequence,
                clip: rig.clip(),
                at: rig.playhead,
                look: moved
            }],
            "letting go is the one step"
        );
        assert_eq!(rig.stored(), moved);
        // And nothing follows it.
        assert!(rig.frame(Vec::new()).is_empty());
        assert!(rig.move_to(pos2(500.0, 300.0)).is_empty());
        assert_eq!(rig.shown(), moved);
    }

    #[test]
    fn a_move_snaps_to_the_middle_of_the_frame() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        let from = pos2(238.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(300.0, 180.0));
        // The frame's centre is at (392, 224): 5 points short both ways.
        rig.move_to(pos2(387.0, 219.0));
        let steps = rig.button(pos2(387.0, 219.0), false);
        assert_eq!(steps.len(), 1);
        let look = rig.stored();
        assert_eq!((look.x, look.y, look.scale), (0, 0, SMALL.scale));
    }

    #[test]
    fn a_corner_drag_resizes_and_is_one_undo_step() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        rig.modifiers = egui::Modifiers::ALT;
        // The bottom right corner is at (816, 459) pixels: (334.4, 191.6)
        // on screen. Twice as far from the top left one, at (336, 189)
        // pixels, is (1296, 729): (526.4, 299.6) on screen.
        let from = pos2(334.4, 191.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(430.0, 245.0));
        assert!(rig.move_to(pos2(526.4, 299.6)).is_empty());
        let shown = rig.shown();
        assert!((shown.scale - 500_000).abs() <= 50, "{shown:?}");
        let steps = rig.button(pos2(526.4, 299.6), false);
        assert_eq!(steps.len(), 1);
        let look = rig.stored();
        assert_eq!(look, shown);
        // The top left corner stayed at (336, 189).
        let area = engine::frame::shown_area(&look, PICTURE, FRAME);
        let corner = Corner::TopLeft.of(&area);
        assert!((corner.0 - 336.0).abs() < 0.1 && (corner.1 - 189.0).abs() < 0.1);
    }

    #[test]
    fn a_double_click_on_the_picture_fits_it_in_one_step() {
        let mut rig = Rig::new();
        let turned = Look {
            turn: 5 * ONE,
            opacity: 600_000,
            ..SMALL
        };
        rig.set(turned);
        let at = pos2(238.4, 137.6);
        rig.move_to(at);
        assert!(rig.button(at, true).is_empty());
        assert!(rig.button(at, false).is_empty());
        assert!(rig.button(at, true).is_empty());
        let steps = rig.button(at, false);
        let fit = Look {
            x: 0,
            y: 0,
            scale: ONE,
            ..turned
        };
        assert_eq!(
            steps,
            vec![Command::SetLookAt {
                sequence: rig.project.active_sequence,
                clip: rig.clip(),
                at: rig.playhead,
                look: fit
            }]
        );
        assert!(rig.frame(Vec::new()).is_empty());
    }

    #[test]
    fn nothing_is_held_without_a_selected_clip_under_the_playhead() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        let from = pos2(238.4, 137.6);
        let drag = |rig: &mut Rig| {
            rig.move_to(from);
            rig.button(from, true);
            rig.move_to(pos2(300.0, 180.0));
            rig.move_to(pos2(340.0, 200.0));
            rig.button(pos2(340.0, 200.0), false)
        };
        // Nothing selected.
        rig.selected.clear();
        assert!(drag(&mut rig).is_empty());
        assert_eq!(rig.stored(), SMALL);
        // Selected, but the playhead is past the clip (10 seconds long).
        rig.selected = vec![rig.clip()];
        rig.playhead = Time::seconds(10);
        assert!(drag(&mut rig).is_empty());
        assert_eq!(rig.stored(), SMALL);
        // On its last frame's side of the end, it is held again.
        rig.playhead = Time::seconds(9);
        assert_eq!(drag(&mut rig).len(), 1);
        // A hidden track's clip isn't in the picture, so not held either.
        let look = rig.stored();
        rig.project.sequences[0].video_tracks[0].hidden = true;
        assert!(drag(&mut rig).is_empty());
        assert_eq!(rig.stored(), look);
    }

    #[test]
    fn a_drag_whose_clip_leaves_the_playhead_is_finished_not_lost() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        rig.modifiers = egui::Modifiers::ALT;
        let from = pos2(238.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(280.0, 137.6));
        rig.move_to(pos2(315.2, 137.6));
        // Playback carries the playhead off the clip with the button held.
        rig.playhead = Time::seconds(12);
        let steps = rig.frame(Vec::new());
        assert_eq!(steps.len(), 1, "what was dragged so far is the step");
        assert_eq!(rig.stored().x, -100_000);
        // Letting go afterwards adds nothing.
        assert!(rig.button(pos2(315.2, 137.6), false).is_empty());
        assert!(rig.frame(Vec::new()).is_empty());
    }

    #[test]
    fn a_drag_on_a_picture_that_moves_over_time_keys_where_the_playhead_is() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        rig.modifiers = egui::Modifiers::ALT;
        // Its place has a keyframe at 0 s: it is animated, held there.
        let (sequence, clip) = (rig.project.active_sequence, rig.clip());
        rig.project = Command::Keyframe {
            sequence,
            clip,
            at: Time::ZERO,
            fields: LookField::X.group().to_vec(),
            change: project::animate::KeyframeChange::Add,
        }
        .apply(&rig.project)
        .unwrap();
        let from = pos2(238.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(280.0, 137.6));
        rig.move_to(pos2(315.2, 137.6));
        let steps = rig.button(pos2(315.2, 137.6), false);
        assert_eq!(steps.len(), 1);
        let stored = &rig.project.active().unwrap().video_tracks[0].clips[0];
        let xs: Vec<i32> = stored
            .keyframes
            .of(LookField::X)
            .iter()
            .map(|k| k.value)
            .collect();
        assert_eq!(xs, [SMALL.x, -100_000], "a second keyframe, at the playhead");
        assert_eq!(
            stored.keyframes.of(LookField::Y).len(),
            2,
            "its place's Y with it"
        );
        assert_eq!(stored.look_at(Time::ZERO).x, SMALL.x, "the first one stays");
        assert_eq!(stored.look.x, SMALL.x, "the clip's own value is not what changed");
        assert_eq!(rig.shown().x, -100_000);
    }

    #[test]
    fn a_press_and_release_without_moving_changes_nothing() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        let at = pos2(238.4, 137.6);
        rig.move_to(at);
        assert!(rig.button(at, true).is_empty());
        assert!(rig.button(at, false).is_empty());
        assert!(rig.frame(Vec::new()).is_empty());
        assert_eq!(rig.stored(), SMALL);
    }

    #[test]
    fn a_turn_is_kept_to_the_millionth_and_a_whole_picture_turns_in_place() {
        let from = Look {
            x: 123_456,
            y: -65_432,
            scale: 400_000,
            turn: 10 * ONE,
            ..Look::PLAIN
        };
        let about = ui_kit::picture_handles::centre(&engine::frame::shown_area(&from, PICTURE, FRAME));
        let to = turned(&from, 90.0, about, FRAME);
        assert_eq!(to.turn, 90 * ONE);
        assert_eq!(
            (to.x, to.y, to.scale),
            (from.x, from.y, from.scale),
            "nothing else moves"
        );
        assert_eq!(turned(&from, -37.25, about, FRAME).turn, -37_250_000);
        // Past a whole turn it is kept as a look keeps it.
        assert_eq!(turned(&from, 400.0, about, FRAME).turn, 40 * ONE);
        assert_eq!(turned(&from, 10.0, about, FRAME), from, "no turn is no change");
    }

    #[test]
    fn a_cut_picture_turns_about_the_middle_of_what_is_seen() {
        let from = Look {
            scale: 500_000,
            x: 100_000,
            turn: 20 * ONE,
            crop_left: 400_000,
            crop_bottom: 200_000,
            ..Look::PLAIN
        };
        let area = |look: &Look| engine::frame::shown_area(look, PICTURE, FRAME);
        let centre = ui_kit::picture_handles::centre;
        let about = centre(&area(&from));
        for degrees in [-45.0, 0.0, 65.0, 180.0, 380.0] {
            let to = turned(&from, degrees, about, FRAME);
            assert!(
                near(centre(&area(&to)), about),
                "{degrees}: {:?}",
                centre(&area(&to))
            );
            assert_eq!(
                (to.crop_left, to.crop_bottom, to.scale),
                (400_000, 200_000, 500_000)
            );
        }
    }

    #[test]
    fn a_crop_is_kept_within_the_picture() {
        let from = Look {
            crop_left: 300_000,
            crop_top: 50_000,
            ..Look::PLAIN
        };
        assert_eq!(cropped(&from, Side::Left, 0.1).crop_left, 400_000);
        assert_eq!(cropped(&from, Side::Top, -0.02).crop_top, 30_000);
        assert_eq!(cropped(&from, Side::Bottom, 0.25).crop_bottom, 250_000);
        // Never under nothing.
        assert_eq!(cropped(&from, Side::Left, -1.0).crop_left, 0);
        // Never past the other side: a hundredth of the picture is left.
        let to = cropped(&from, Side::Right, 2.0);
        assert_eq!(to.crop_right, Look::MOST_CROP - 300_000);
        assert_eq!(to.crop_left, 300_000, "the other side stays");
        let to = cropped(&from, Side::Left, 2.0);
        assert_eq!(to.crop_left, Look::MOST_CROP);
        assert!(to.check().is_ok());
        // Nothing else changes.
        assert_eq!(
            Look {
                crop_left: from.crop_left,
                ..cropped(&from, Side::Left, 0.1)
            },
            from
        );
    }

    /// SMALL's turn handle: 24 points above its top edge's middle, which is
    /// at (576, 189) pixels.
    const KNOB: (f32, f32) = (238.4, 83.6 - 24.0);

    #[test]
    fn a_turn_drag_turns_the_picture_and_is_one_undo_step() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        let knob = pos2(KNOB.0, KNOB.1);
        rig.move_to(knob);
        rig.button(knob, true);
        // Round to straight right of the middle: a quarter turn.
        rig.move_to(pos2(300.0, 80.0));
        assert!(rig.move_to(pos2(320.0, 138.6)).is_empty());
        assert_eq!(rig.shown().turn, 90 * ONE, "near a right angle it lands on it");
        let steps = rig.button(pos2(320.0, 138.6), false);
        let turned = Look {
            turn: 90 * ONE,
            ..SMALL
        };
        assert_eq!(
            steps,
            vec![Command::SetLookAt {
                sequence: rig.project.active_sequence,
                clip: rig.clip(),
                at: rig.playhead,
                look: turned
            }]
        );
        assert_eq!(rig.stored(), turned);
        assert!(rig.frame(Vec::new()).is_empty());
    }

    #[test]
    fn a_side_drag_crops_the_picture_shows_it_whole_meanwhile_and_is_one_step() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        rig.modifiers = egui::Modifiers::ALT;
        // The left side is at 336 pixels: (142.4, 137.6) on screen. 19.2
        // points right is 48 pixels, a tenth of the picture's width.
        let from = pos2(142.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(150.0, 140.0));
        assert!(rig.move_to(pos2(161.6, 145.0)).is_empty());
        assert_eq!(rig.shown().crop_left, 100_000);
        assert_eq!(
            rig.handles.cropping(),
            Some(rig.clip()),
            "the viewer shows it whole"
        );
        let shown = rig.inspector.shown(&rig.project).unwrap();
        let whole = uncropped(&shown, rig.clip()).unwrap();
        assert_eq!(whole.active().unwrap().video_tracks[0].clips[0].look, SMALL);
        let steps = rig.button(pos2(161.6, 145.0), false);
        assert_eq!(steps.len(), 1);
        assert_eq!(rig.handles.cropping(), None);
        let cut = Look {
            crop_left: 100_000,
            ..SMALL
        };
        assert_eq!(rig.stored(), cut);
        // The rest of the picture stayed where it was.
        let area = engine::frame::shown_area(&cut, PICTURE, FRAME);
        assert!(near(Corner::TopLeft.of(&area), (384.0, 189.0)));
        assert!(near(Corner::BottomRight.of(&area), (816.0, 459.0)));
    }

    #[test]
    fn a_side_of_a_turned_picture_is_cropped_along_the_picture() {
        let mut rig = Rig::new();
        // A quarter turn: the picture's top side is on the right as it
        // stands, at 576 + 135 = 711 pixels: (292.4, 137.6) on screen.
        let quarter = Look {
            turn: 90 * ONE,
            ..SMALL
        };
        rig.set(quarter);
        rig.modifiers = egui::Modifiers::ALT;
        // 10.8 points left, into the picture: 27 pixels, a tenth of its height.
        let from = pos2(292.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(287.0, 137.6));
        rig.move_to(pos2(281.6, 137.6));
        let steps = rig.button(pos2(281.6, 137.6), false);
        assert_eq!(steps.len(), 1);
        let stored = rig.stored();
        assert_eq!((stored.crop_top, stored.turn), (100_000, 90 * ONE));
    }

    #[test]
    fn a_crop_or_a_turn_on_a_picture_that_changes_over_time_keys_the_playhead() {
        let mut rig = Rig::new();
        rig.set(SMALL);
        let (sequence, clip) = (rig.project.active_sequence, rig.clip());
        for field in [LookField::CropLeft, LookField::Turn] {
            rig.project = Command::Keyframe {
                sequence,
                clip,
                at: Time::ZERO,
                fields: field.group().to_vec(),
                change: project::animate::KeyframeChange::Add,
            }
            .apply(&rig.project)
            .unwrap();
        }
        let from = pos2(142.4, 137.6);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(150.0, 137.6));
        rig.move_to(pos2(161.6, 137.6));
        assert_eq!(rig.button(pos2(161.6, 137.6), false).len(), 1);
        let knob = pos2(KNOB.0 + 9.6, KNOB.1);
        rig.move_to(knob);
        rig.button(knob, true);
        rig.move_to(pos2(320.0, 80.0));
        rig.move_to(pos2(340.0, 138.6));
        assert_eq!(rig.button(pos2(340.0, 138.6), false).len(), 1);
        let stored = &rig.project.active().unwrap().video_tracks[0].clips[0];
        let values =
            |field: LookField| -> Vec<i32> { stored.keyframes.of(field).iter().map(|k| k.value).collect() };
        assert_eq!(
            values(LookField::CropLeft),
            [0, 100_000],
            "a second keyframe, at the playhead"
        );
        assert_eq!(values(LookField::Turn), [0, 90 * ONE]);
        assert_eq!(stored.look_at(Time::ZERO).crop_left, 0, "the first ones stay");
        assert_eq!(stored.look.turn, 0, "the clip's own value is not what changed");
        assert_eq!((rig.shown().crop_left, rig.shown().turn), (100_000, 90 * ONE));
    }
}
