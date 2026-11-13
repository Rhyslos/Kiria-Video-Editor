//! Handles on a picture in the viewer: an outline around the area a picture
//! has in the frame, with a square handle on each corner, a bar on the
//! middle of each side and a round turn handle on a short stem off the top
//! edge. Dragging inside the outline moves the picture, dragging a corner
//! resizes it about the opposite corner, dragging a side cuts that side off
//! (or gives it back), dragging the turn handle turns it about its middle,
//! and a double-click inside asks for it fitted.
//!
//! This is the part every page with a viewer shares: where the frame's
//! pixels are on screen and what of an outline is under the pointer
//! ([`grab`]: [`FrameMap`], [`Outline`], [`grab_at`]), what a drag does and
//! how it snaps ([`measure`]: [`snap_move`], [`snap_resize`],
//! [`snap_turn`], [`crop_step`], [`snap_crop`]), and the drag and the
//! drawing ([`Handles`]). It says what the hand did, in frame pixels and
//! degrees ([`Change`]); which picture that is, and what the change means
//! for it, is the page's.
//!
//! Everything on screen is in points, as the pointer's place is, so how
//! many pixels the screen has to a point never comes into it.

mod grab;
mod measure;

pub use grab::{
    Corner, EDGE_REACH, FrameMap, Grab, HANDLE, HANDLE_REACH, KNOB, Outline, Side, TURN_STEM, grab_at,
};
pub use measure::{
    Guide, Guides, TURN_MAGNET, TURN_STEP, centre, crop_step, part_of, resize_factor, snap_crop, snap_move,
    snap_resize, snap_turn, turned_by,
};
pub use render::{Area, Part, Placement, Point};

use crate::Theme;
use egui::{
    CursorIcon, FontId, Id, LayerId, Order, Painter, PointerButton, Pos2, Rect, Response, Shape, Stroke, Ui,
    Vec2, pos2, vec2,
};

/// How close, in points on screen, a picture must come to a line of the
/// frame to snap to it: the timeline's own reach.
pub const SNAP_REACH: f32 = 8.0;

/// A picture as its handles see it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picture {
    /// Where the whole picture is, before anything is cut off it, in
    /// frame pixels.
    pub whole: Area,
    /// The part of it that is seen: what is left after cropping.
    pub part: Part,
    /// How far it is turned, clockwise, in degrees, as the page keeps it,
    /// so that a turn snapped to a whole number of degrees lands on it
    /// exactly.
    pub degrees: f64,
}

impl Picture {
    /// The part that is seen, where it is: what the outline goes round.
    pub fn shown(&self) -> Area {
        part_of(&self.whole, self.part)
    }
}

/// What the hand did to the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    /// Dragged: the picture is this far, in frame pixels, from where it
    /// was when the drag began.
    Move { by: Point },
    /// A corner dragged: the picture is `factor` times as large as when
    /// the drag began, and the place `anchor` (the opposite corner, in
    /// frame pixels) has stayed where it was.
    Resize { factor: f32, anchor: Point },
    /// The turn handle dragged: the picture is turned `degrees` (clockwise,
    /// counted on from where it was, so it may pass a whole turn) about the
    /// place `about`: the middle of what is seen of it when the drag
    /// began, in frame pixels, which stays where it was.
    Turn { degrees: f64, about: Point },
    /// A side dragged: `cut` more of the whole picture's width (left and
    /// right) or height (top and bottom) is cut off that side than when the
    /// drag began; under 0 gives some back. The rest stays where it was.
    Crop { side: Side, cut: f32 },
    /// Double-clicked: as large as fits the frame, centred.
    Fit,
}

/// What happened to the handles this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Handled {
    pub change: Option<Change>,
    /// The hand is still on it: more changes may follow, so this is not
    /// the moment to make an undo step.
    pub busy: bool,
    /// A drag ended in this frame.
    pub ended: bool,
}

/// A drag under way.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    grab: Grab,
    /// Where the button went down, on screen.
    pressed: Pos2,
    /// The picture when the drag began.
    start: Picture,
    /// The lines it is snapped to now.
    guides: Guides,
    /// For a turn: where the pointer was last seen, how far it has gone
    /// round the middle since the drag began, and the turn it gave.
    last: Pos2,
    turned: f64,
    degrees: f64,
}

impl Drag {
    fn new(grab: Grab, pressed: Pos2, start: Picture) -> Drag {
        Drag {
            grab,
            pressed,
            start,
            guides: [None; 2],
            last: pressed,
            turned: 0.0,
            degrees: start.degrees,
        }
    }

    /// What the drag has done with the pointer at `at`. `snapping` is
    /// whether to snap to the frame's lines and to right angles; `steps`
    /// (Shift held) turns in whole steps.
    fn change(&mut self, at: Pos2, map: &FrameMap, snapping: bool, steps: bool) -> Change {
        let by = map.step_to_frame(at - self.pressed);
        let reach = map.pixels(SNAP_REACH);
        let shown = self.start.shown();
        self.guides = [None; 2];
        match self.grab {
            Grab::Inside => {
                let mut by = by;
                if snapping {
                    let moved = Area {
                        origin: (shown.origin.0 + by.0, shown.origin.1 + by.1),
                        ..shown
                    };
                    let (further, guides) = snap_move(&moved, map.frame, reach);
                    by = (by.0 + further.0, by.1 + further.1);
                    self.guides = guides;
                }
                Change::Move { by }
            }
            Grab::Corner(corner) => {
                let anchor = corner.opposite().of(&shown);
                let from = corner.of(&shown);
                // The handle keeps the distance from the pointer it was taken at.
                let mut factor = resize_factor(anchor, from, (from.0 + by.0, from.1 + by.1));
                if snapping {
                    (factor, self.guides) = snap_resize(anchor, from, factor, map.frame, reach);
                }
                Change::Resize { factor, anchor }
            }
            Grab::Edge(side) => {
                let mut cut = crop_step(&self.start.whole, side, by);
                if snapping {
                    (cut, self.guides) =
                        snap_crop(&self.start.whole, self.start.part, side, cut, map.frame, reach);
                }
                Change::Crop { side, cut }
            }
            Grab::Turn => {
                let about = centre(&shown);
                let pivot = map.to_screen(about);
                // Right by the middle there is no telling which way the
                // pointer points: wait until it is out of there.
                if at.distance(pivot) >= 2.0 {
                    let point = |p: Pos2| (p.x, p.y);
                    self.turned += turned_by(point(pivot), point(self.last), point(at));
                    self.last = at;
                }
                self.degrees = snap_turn(self.start.degrees + self.turned, steps, snapping);
                Change::Turn {
                    degrees: self.degrees,
                    about,
                }
            }
        }
    }
}

/// The handles on one picture: the drag under way, kept by the page from
/// frame to frame.
#[derive(Debug, Default)]
pub struct Handles {
    drag: Option<Drag>,
    /// What the pointer is over or has hold of, for drawing.
    hot: Option<Grab>,
}

impl Handles {
    /// Whether a drag is under way.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether a side is being dragged: the page may show the whole
    /// picture meanwhile, and [`Handles::draw`] veils what is cut off.
    pub fn cropping(&self) -> bool {
        matches!(
            self.drag,
            Some(Drag {
                grab: Grab::Edge(_),
                ..
            })
        )
    }

    /// Gives up the drag under way: the picture it was on is no longer there.
    pub fn let_go(&mut self) {
        self.drag = None;
        self.hot = None;
    }

    /// Reads what the pointer does to `picture`. `response` is the
    /// viewer's own, sensing clicks and drags, so what is drawn over the
    /// viewer afterwards (the transport) keeps its clicks. `snapping` is
    /// the page's switch; Alt held skips it for the moment, as on the
    /// timeline. Shift held turns in whole steps.
    pub fn interact(
        &mut self,
        ui: &Ui,
        response: &Response,
        map: FrameMap,
        picture: Picture,
        snapping: bool,
    ) -> Handled {
        let outline = Outline::new(&map, &picture.shown(), response.rect);
        let primary = PointerButton::Primary;
        if response.drag_started_by(primary)
            && let Some(pressed) = ui.input(|i| i.pointer.press_origin())
            && let Some(grab) = grab_at(&outline, pressed)
        {
            self.drag = Some(Drag::new(grab, pressed, picture));
        }
        let mut handled = Handled::default();
        if let Some(drag) = &mut self.drag {
            handled.busy = response.dragged_by(primary);
            if (handled.busy || response.drag_stopped())
                && let Some(at) = response.interact_pointer_pos()
            {
                let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
                handled.change = Some(drag.change(at, &map, snapping && !alt, shift));
            }
            self.hot = Some(drag.grab);
            let start = Outline::new(&map, &drag.start.shown(), response.rect);
            ui.ctx().set_cursor_icon(cursor(drag.grab, &start));
            if !handled.busy {
                handled.ended = true;
                self.drag = None;
            }
            return handled;
        }
        // Only where nothing drawn over the viewer is in the way.
        self.hot = response
            .hovered()
            .then(|| response.hover_pos())
            .flatten()
            .and_then(|at| grab_at(&outline, at));
        if let Some(grab) = self.hot {
            ui.ctx().set_cursor_icon(cursor(grab, &outline));
        }
        if response.double_clicked_by(primary)
            && let Some(at) = response.interact_pointer_pos()
            && grab_at(&outline, at) == Some(Grab::Inside)
        {
            handled.change = Some(Change::Fit);
        }
        handled
    }

    /// Draws the outline of `picture` with its handles, and the lines the
    /// drag under way is snapped to, clipped to `within` (the viewer, as
    /// the response handed to [`Handles::interact`] covers it). While a
    /// side is dragged, what is cut off the whole picture is veiled; while
    /// the turn handle is held, the pointer is a turning arrow and the
    /// turn is written beside it. Call it after [`Handles::interact`], with
    /// the picture as it is after what the hand did, so the outline never
    /// trails the pointer.
    pub fn draw(&self, ui: &Ui, theme: &Theme, within: Rect, map: FrameMap, picture: Picture) {
        let painter = ui.painter_at(within);
        // A dark line under the accent keeps both readable on any picture.
        let under = theme.accent_ink.gamma_multiply(0.6);
        let frame = map.on_screen;
        for guide in self.drag.iter().flat_map(|drag| drag.guides).flatten() {
            let ends = match guide {
                Guide::Vertical(x) => {
                    let x = map.to_screen((x, 0.0)).x;
                    [pos2(x, frame.top()), pos2(x, frame.bottom())]
                }
                Guide::Horizontal(y) => {
                    let y = map.to_screen((0.0, y)).y;
                    [pos2(frame.left(), y), pos2(frame.right(), y)]
                }
            };
            painter.line_segment(ends, Stroke::new(3.0, under));
            painter.line_segment(ends, Stroke::new(1.0, theme.accent));
        }
        if self.cropping() {
            veil(&painter, theme, &map, &picture);
        }
        let outline = Outline::new(&map, &picture.shown(), within);
        let corners = outline.corners.to_vec();
        painter.add(Shape::closed_line(corners.clone(), Stroke::new(3.5, under)));
        painter.add(Shape::closed_line(corners, Stroke::new(1.5, theme.accent)));
        // The handles are turned with the picture.
        let [top_left, top_right, _, bottom_left] = outline.corners;
        let across = (top_right - top_left).normalized();
        let down = (bottom_left - top_left).normalized();
        let (across, down) = if across == Vec2::ZERO || down == Vec2::ZERO {
            (vec2(1.0, 0.0), vec2(0.0, 1.0))
        } else {
            (across, down)
        };
        let hot = |grab: Grab| self.hot == Some(grab);
        // The turn handle on its stem.
        let reach = (outline.knob - outline.stem).normalized() * KNOB;
        painter.line_segment([outline.stem, outline.knob - reach], Stroke::new(3.0, under));
        painter.line_segment(
            [outline.stem, outline.knob - reach],
            Stroke::new(1.5, theme.accent),
        );
        let radius = if hot(Grab::Turn) { KNOB + 1.5 } else { KNOB };
        painter.circle(
            outline.knob,
            radius,
            theme.accent,
            Stroke::new(1.0, theme.accent_ink),
        );
        // A bar on the middle of each side, along it.
        for side in Side::ALL {
            let along = if side.runs_down() { down } else { across };
            let square = if side.runs_down() { across } else { down };
            let grow = if hot(Grab::Edge(side)) { 1.5 } else { 0.0 };
            let (a, s) = (along * (HANDLE * 0.8 + grow), square * (2.5 + grow / 2.0));
            let at = outline.middle(side);
            painter.add(Shape::convex_polygon(
                vec![at - a - s, at + a - s, at + a + s, at - a + s],
                theme.accent,
                Stroke::new(1.0, theme.accent_ink),
            ));
        }
        for (corner, at) in Corner::ALL.into_iter().zip(outline.corners) {
            let half = if hot(Grab::Corner(corner)) {
                HANDLE / 2.0 + 1.5
            } else {
                HANDLE / 2.0
            };
            let (a, d) = (across * half, down * half);
            painter.add(Shape::convex_polygon(
                vec![at - a - d, at + a - d, at + a + d, at - a + d],
                theme.accent,
                Stroke::new(1.0, theme.accent_ink),
            ));
        }
        if hot(Grab::Turn) {
            self.draw_turning(ui, theme);
        }
    }

    /// The turning pointer (the system has none, so it is hidden and drawn
    /// here, over everything) and, while turning, the turn beside it.
    fn draw_turning(&self, ui: &Ui, theme: &Theme) {
        let Some(at) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let painter = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Tooltip, Id::unique("picture turn pointer")));
        turn_pointer(&painter, theme, at);
        let Some(drag) = self.drag.filter(|drag| drag.grab == Grab::Turn) else {
            return;
        };
        let galley = painter.layout_no_wrap(turn_text(drag.degrees), FontId::proportional(12.0), theme.text);
        let pill = Rect::from_min_size(at + vec2(16.0, 14.0), galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(pill, pill.height() / 2.0, theme.viewport.gamma_multiply(0.9));
        painter.galley(pill.min + vec2(6.0, 3.0), galley, theme.text);
    }
}

/// A turn as the label beside the pointer writes it: as the picture keeps
/// it (a whole turn is no turn, the way round kept), to a tenth of a degree.
pub fn turn_text(degrees: f64) -> String {
    let kept = degrees % 360.0;
    let tenths = (kept * 10.0).round() / 10.0;
    // No "-0°".
    let tenths = if tenths == 0.0 { 0.0 } else { tenths };
    let text = format!("{tenths:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text}°")
}

/// Shades what is cut off the whole picture (between its whole area and
/// the part that is seen), and outlines the whole with a dashed line, so a
/// side being cut shows what it leaves out.
fn veil(painter: &Painter, theme: &Theme, map: &FrameMap, picture: &Picture) {
    let whole = picture.whole;
    let at = |u: f32, v: f32| {
        map.to_screen((
            whole.origin.0 + u * whole.across.0 + v * whole.down.0,
            whole.origin.1 + u * whole.across.1 + v * whole.down.1,
        ))
    };
    let part = picture.part;
    let (left, top) = (part.left, part.top);
    let (right, bottom) = (left + part.width, top + part.height);
    // Above and below the part the whole width; beside it, its height.
    let strips = [
        (0.0, 0.0, 1.0, top),
        (0.0, bottom, 1.0, 1.0),
        (0.0, top, left, bottom),
        (right, top, 1.0, bottom),
    ];
    let shade = theme.viewport.gamma_multiply(0.6);
    for (u0, v0, u1, v1) in strips {
        if u1 - u0 > 1e-4 && v1 - v0 > 1e-4 {
            painter.add(Shape::convex_polygon(
                vec![at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)],
                shade,
                Stroke::NONE,
            ));
        }
    }
    let mut around = vec![at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0)];
    around.push(around[0]);
    painter.extend(Shape::dashed_line(
        &around,
        Stroke::new(1.0, theme.accent.gamma_multiply(0.8)),
        5.0,
        4.0,
    ));
}

/// A small circular arrow with a head at each end, round `at`: the pointer
/// over the turn handle.
fn turn_pointer(painter: &Painter, theme: &Theme, at: Pos2) {
    const RADIUS: f32 = 7.0;
    // Round the top, from lower left to lower right.
    let (from, to) = (150f32.to_radians(), 390f32.to_radians());
    let point = |angle: f32| at + vec2(angle.cos(), -angle.sin()) * RADIUS;
    let arc: Vec<Pos2> = (0..=16)
        .map(|i| point(from + (to - from) * i as f32 / 16.0))
        .collect();
    let head = |angle: f32, way: f32| {
        // The arrow's tip, and its two barbs back along the arc.
        let tip = point(angle);
        let tangent = vec2(-angle.sin(), -angle.cos()) * way;
        let normal = vec2(angle.cos(), -angle.sin());
        vec![
            tip + tangent * 4.0,
            tip - tangent * 1.0 + normal * 3.5,
            tip - tangent * 1.0 - normal * 3.5,
        ]
    };
    let dark = theme.accent_ink;
    for (width, colour) in [(4.0, dark), (1.8, theme.text)] {
        painter.add(Shape::line(arc.clone(), Stroke::new(width, colour)));
    }
    for (angle, way) in [(from, -1.0), (to, 1.0)] {
        painter.add(Shape::convex_polygon(
            head(angle, way),
            theme.text,
            Stroke::new(1.0, dark),
        ));
    }
    painter.circle_filled(at, 1.5, theme.text);
}

/// The pointer over a handle of an outline.
fn cursor(grab: Grab, outline: &Outline) -> CursorIcon {
    match grab {
        Grab::Inside => CursorIcon::Move,
        Grab::Corner(corner) => resize_cursor(outline.corner(corner) - outline.corner(corner.opposite())),
        Grab::Edge(side) => {
            let (a, b) = side.ends();
            let along = outline.corner(b) - outline.corner(a);
            // Square to the side: the way it moves.
            resize_cursor(vec2(-along.y, along.x))
        }
        // Drawn by the handles instead.
        Grab::Turn => CursorIcon::None,
    }
}

/// The two-headed arrow nearest the way a handle is dragged: `along` is
/// that way on screen, so a turned picture's handles point the way they move.
fn resize_cursor(along: Vec2) -> CursorIcon {
    // 0 is sideways, 90 straight down; a line has no direction, so 0 to 180.
    let degrees = along.y.atan2(along.x).to_degrees().rem_euclid(180.0);
    match degrees {
        d if !(22.5..157.5).contains(&d) => CursorIcon::ResizeHorizontal,
        d if d < 67.5 => CursorIcon::ResizeNwSe,
        d if d < 112.5 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// What the tests of the handles' parts share: a frame, its place on
/// screen, and pictures in it.
#[cfg(test)]
mod fixtures {
    use super::*;
    use render::Placement;

    pub const FRAME: (u32, u32) = (1920, 1080);

    /// A viewer's frame 768 points wide with its corner at (8, 8): 0.4
    /// points to a frame pixel.
    pub fn map() -> FrameMap {
        FrameMap {
            on_screen: Rect::from_min_size(pos2(8.0, 8.0), vec2(768.0, 432.0)),
            frame: FRAME,
        }
    }

    pub fn upright(centre: Point, size: (f32, f32)) -> Area {
        turned(centre, size, 0.0)
    }

    pub fn turned(centre: Point, size: (f32, f32), degrees: f32) -> Area {
        Area::of_part(
            Placement {
                centre,
                size,
                degrees,
            },
            Part::WHOLE,
        )
    }

    /// A whole picture, nothing cut off.
    pub fn picture(centre: Point, size: (f32, f32), degrees: f32) -> Picture {
        Picture {
            whole: turned(centre, size, degrees),
            part: Part::WHOLE,
            degrees: f64::from(degrees),
        }
    }

    pub fn near(a: Point, b: Point) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{FRAME, map, near, picture};
    use super::*;

    #[test]
    fn handles_point_the_way_they_are_dragged() {
        assert_eq!(resize_cursor(vec2(160.0, 90.0)), CursorIcon::ResizeNwSe);
        assert_eq!(resize_cursor(vec2(-160.0, -90.0)), CursorIcon::ResizeNwSe);
        assert_eq!(resize_cursor(vec2(160.0, -90.0)), CursorIcon::ResizeNeSw);
        assert_eq!(resize_cursor(vec2(200.0, 5.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(vec2(-200.0, 5.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(vec2(3.0, -200.0)), CursorIcon::ResizeVertical);
        // A side's handle points across the side.
        let outline = Outline::new(
            &map(),
            &picture((960.0, 540.0), (400.0, 200.0), 0.0).shown(),
            Rect::EVERYTHING,
        );
        assert_eq!(
            cursor(Grab::Edge(Side::Left), &outline),
            CursorIcon::ResizeHorizontal
        );
        assert_eq!(
            cursor(Grab::Edge(Side::Bottom), &outline),
            CursorIcon::ResizeVertical
        );
        assert_eq!(cursor(Grab::Turn, &outline), CursorIcon::None);
        let turned = Outline::new(
            &map(),
            &picture((960.0, 540.0), (400.0, 200.0), 90.0).shown(),
            Rect::EVERYTHING,
        );
        assert_eq!(
            cursor(Grab::Edge(Side::Left), &turned),
            CursorIcon::ResizeVertical
        );
    }

    #[test]
    fn a_turn_is_written_as_the_picture_keeps_it() {
        assert_eq!(turn_text(0.0), "0°");
        assert_eq!(turn_text(-0.02), "0°");
        assert_eq!(turn_text(90.0), "90°");
        assert_eq!(turn_text(37.46), "37.5°");
        assert_eq!(turn_text(-12.5), "-12.5°");
        assert_eq!(turn_text(370.0), "10°");
        assert_eq!(turn_text(-450.0), "-90°");
    }

    /// What a page does with a change: the picture as it is after it.
    fn changed(start: Picture, change: Change) -> Picture {
        let scale = |(x, y): Point, by: f32| (x * by, y * by);
        match change {
            Change::Move { by } => Picture {
                whole: Area {
                    origin: (start.whole.origin.0 + by.0, start.whole.origin.1 + by.1),
                    ..start.whole
                },
                ..start
            },
            Change::Resize { factor, anchor } => Picture {
                whole: Area {
                    origin: (
                        anchor.0 + factor * (start.whole.origin.0 - anchor.0),
                        anchor.1 + factor * (start.whole.origin.1 - anchor.1),
                    ),
                    across: scale(start.whole.across, factor),
                    down: scale(start.whole.down, factor),
                },
                ..start
            },
            Change::Turn { degrees, about } => {
                let (sin, cos) = ((degrees - start.degrees) as f32).to_radians().sin_cos();
                let turn = |(x, y): Point| (x * cos - y * sin, x * sin + y * cos);
                let from = (start.whole.origin.0 - about.0, start.whole.origin.1 - about.1);
                let to = turn(from);
                Picture {
                    whole: Area {
                        origin: (about.0 + to.0, about.1 + to.1),
                        across: turn(start.whole.across),
                        down: turn(start.whole.down),
                    },
                    degrees,
                    ..start
                }
            }
            Change::Crop { side, cut } => {
                let mut part = start.part;
                match side {
                    Side::Left => (part.left, part.width) = (part.left + cut, part.width - cut),
                    Side::Top => (part.top, part.height) = (part.top + cut, part.height - cut),
                    Side::Right => part.width -= cut,
                    Side::Bottom => part.height -= cut,
                }
                Picture { part, ..start }
            }
            Change::Fit => Picture {
                whole: Area::fitted(FRAME, FRAME),
                part: Part::WHOLE,
                degrees: 0.0,
            },
        }
    }

    /// A viewer in a window, with a picture that follows the handles.
    struct Rig {
        ctx: egui::Context,
        handles: Handles,
        /// The picture as it is, and as it was when the drag began.
        picture: Picture,
        start: Option<Picture>,
        snapping: bool,
        modifiers: egui::Modifiers,
        /// The transport's play button was clicked.
        played: bool,
    }

    impl Rig {
        fn new(picture: Picture) -> Rig {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                handles: Handles::default(),
                picture,
                start: None,
                snapping: true,
                modifiers: egui::Modifiers::NONE,
                played: false,
            };
            // A first frame, so the pointer finds the window laid out.
            rig.frame(Vec::new());
            rig
        }

        /// One frame: the viewer 784 x 448 points at the window's top left
        /// corner, so its frame is the one of `map()`.
        fn frame(&mut self, events: Vec<egui::Event>) -> Handled {
            let events = std::iter::once(egui::Event::ModifiersChanged(self.modifiers))
                .chain(events)
                .collect();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut handled = Handled::default();
            let output = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let (frame, response) =
                            crate::viewer::viewer(ui, &Theme::ASH, vec2(784.0, 448.0), FRAME, None, "");
                        assert_eq!(frame, map().on_screen);
                        let map = FrameMap {
                            on_screen: frame,
                            frame: FRAME,
                        };
                        handled = self
                            .handles
                            .interact(ui, &response, map, self.picture, self.snapping);
                        if let Some(change) = handled.change {
                            let start = *self.start.get_or_insert(self.picture);
                            self.picture = changed(start, change);
                        }
                        if !handled.busy {
                            self.start = None;
                        }
                        self.handles
                            .draw(ui, &Theme::ASH, response.rect, map, self.picture);
                        let shortcuts = crate::ShortcutTable::default();
                        let click = crate::transport::transport(
                            ui,
                            &Theme::ASH,
                            frame,
                            false,
                            Some(false),
                            &shortcuts,
                        );
                        self.played |= click == Some(crate::TransportClick::PlayPause);
                    });
            });
            // Nothing draws these frames.
            output.drop_without_applying_deltas();
            handled
        }

        fn move_to(&mut self, at: Pos2) -> Handled {
            self.frame(vec![egui::Event::PointerMoved(at)])
        }

        fn button(&mut self, at: Pos2, pressed: bool) -> Handled {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: self.modifiers,
            }])
        }

        /// Presses at `from`, moves to `to` in two steps and lets go.
        /// Returns what the last frame of holding and the frame of letting go said.
        fn drag(&mut self, from: Pos2, to: Pos2) -> (Handled, Handled) {
            self.move_to(from);
            self.button(from, true);
            self.move_to(from + (to - from) * 0.5);
            let held = self.move_to(to);
            (held, self.button(to, false))
        }

        fn area(&self) -> Area {
            self.picture.shown()
        }

        fn centre(&self) -> Point {
            let (left, top, right, bottom) = measure::bounds(&self.area());
            ((left + right) / 2.0, (top + bottom) / 2.0)
        }
    }

    /// 480 x 270 pixels, centred at (500, 300): on screen its centre is at
    /// (208, 128), its top edge's middle at (208, 74), and its turn handle
    /// at (208, 50).
    fn small() -> Picture {
        picture((500.0, 300.0), (480.0, 270.0), 0.0)
    }

    #[test]
    fn dragging_inside_moves_the_picture_with_the_pointer() {
        let mut rig = Rig::new(small());
        rig.snapping = false;
        // Taken a little off its centre, moved 100 points right and 40 down.
        let (held, released) = rig.drag(pos2(220.0, 120.0), pos2(320.0, 160.0));
        assert!(held.busy && !held.ended);
        assert_eq!(held.change, Some(Change::Move { by: (250.0, 100.0) }));
        assert!(!released.busy && released.ended, "letting go ends it");
        assert!(near(rig.centre(), (750.0, 400.0)), "{:?}", rig.centre());
        assert!(!rig.handles.dragging());
        // Nothing more happens with the button up.
        assert_eq!(rig.move_to(pos2(500.0, 300.0)), Handled::default());
        assert!(near(rig.centre(), (750.0, 400.0)));
    }

    #[test]
    fn a_move_snaps_to_the_frames_centre_unless_alt_is_held_or_snapping_is_off() {
        // The frame's centre is at (392, 224) on screen: 184 right and 96
        // down from the picture's. Stop 5 points short each way.
        let (from, to) = (pos2(208.0, 128.0), pos2(387.0, 219.0));
        let mut rig = Rig::new(small());
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(300.0, 170.0));
        rig.move_to(to);
        assert!(near(rig.centre(), (960.0, 540.0)), "{:?}", rig.centre());
        let drag = rig.handles.drag.unwrap();
        assert_eq!(
            drag.guides,
            [Some(Guide::Vertical(960.0)), Some(Guide::Horizontal(540.0))]
        );
        // Further than the reach, it lets go of the lines.
        rig.move_to(pos2(420.0, 219.0));
        assert!((rig.centre().0 - 1030.0).abs() < 1e-2, "{:?}", rig.centre());
        assert_eq!(rig.handles.drag.unwrap().guides[0], None);
        rig.button(pos2(420.0, 219.0), false);

        // Alt held: where the pointer put it, 12.5 pixels short.
        let mut rig = Rig::new(small());
        rig.modifiers = egui::Modifiers::ALT;
        rig.drag(from, to);
        assert!(near(rig.centre(), (947.5, 527.5)), "{:?}", rig.centre());

        // The page's snapping switched off: the same.
        let mut rig = Rig::new(small());
        rig.snapping = false;
        rig.drag(from, to);
        assert!(near(rig.centre(), (947.5, 527.5)), "{:?}", rig.centre());
    }

    #[test]
    fn dragging_a_corner_resizes_about_the_opposite_corner() {
        // Top left corner at (260, 165) pixels; bottom right at (740, 435),
        // which is (304, 182) on screen.
        let mut rig = Rig::new(small());
        rig.snapping = false;
        // Out along the diagonal by half its length again: 96 x 54 points.
        let (held, released) = rig.drag(pos2(306.0, 184.0), pos2(402.0, 238.0));
        let Some(Change::Resize { factor, anchor }) = held.change else {
            panic!("a corner resizes: {held:?}");
        };
        assert!((factor - 1.5).abs() < 1e-4, "{factor}");
        assert!(near(anchor, (260.0, 165.0)));
        assert!(released.ended);
        assert!(near(Corner::TopLeft.of(&rig.area()), (260.0, 165.0)));
        assert!(near(Corner::BottomRight.of(&rig.area()), (980.0, 570.0)));
        // The shape is the same.
        let (w, h) = rig.area().size();
        assert!((w / h - 16.0 / 9.0).abs() < 1e-4);

        // The top left corner, dragged in: the bottom right stays.
        let (held, _) = rig.drag(pos2(104.0, 74.0), pos2(200.0, 128.0));
        let Some(Change::Resize { factor, anchor }) = held.change else {
            panic!("a corner resizes: {held:?}");
        };
        assert!((factor - 2.0 / 3.0).abs() < 1e-4, "{factor}");
        assert!(near(anchor, (980.0, 570.0)));
        assert!(near(Corner::TopLeft.of(&rig.area()), (500.0, 300.0)));
    }

    #[test]
    fn a_turned_picture_resizes_along_its_own_diagonal() {
        // A quarter turn: the picture's bottom right corner is at the
        // bottom left as it stands, at (365, 540) pixels: (154, 224) on screen.
        let start = picture((500.0, 300.0), (480.0, 270.0), 90.0);
        assert!(near(Corner::BottomRight.of(&start.shown()), (365.0, 540.0)));
        let mut rig = Rig::new(start);
        rig.snapping = false;
        // Twice as far from the opposite corner, which is at (635, 60).
        let to = map().to_screen((95.0, 1020.0));
        let (held, _) = rig.drag(pos2(154.0, 224.0), to);
        let Some(Change::Resize { factor, anchor }) = held.change else {
            panic!("a corner resizes: {held:?}");
        };
        assert!((factor - 2.0).abs() < 1e-3, "{factor}");
        assert!(near(anchor, (635.0, 60.0)));
        let (w, h) = rig.area().size();
        assert!((w - 960.0).abs() < 0.5 && (h - 540.0).abs() < 0.5);
        // Still a quarter turn: its top edge runs straight down.
        assert!(rig.area().across.0.abs() < 1e-2 && rig.area().across.1 > 0.0);
    }

    #[test]
    fn a_resize_snaps_its_corner_to_the_frames_edge() {
        // Its top left corner on the frame's; the bottom right at
        // (960, 540), the frame's centre: (392, 224) on screen.
        let mut rig = Rig::new(picture((480.0, 270.0), (960.0, 540.0), 0.0));
        // To 4 points short of the frame's bottom right corner.
        rig.drag(pos2(392.0, 224.0), pos2(772.0, 437.75));
        assert!(near(Corner::BottomRight.of(&rig.area()), (1920.0, 1080.0)));
        assert!(near(Corner::TopLeft.of(&rig.area()), (0.0, 0.0)));
    }

    #[test]
    fn dragging_the_turn_handle_turns_the_picture_about_its_middle() {
        let mut rig = Rig::new(small());
        rig.modifiers = egui::Modifiers::ALT;
        // From straight above the middle to up and right of it, at 45
        // degrees: an eighth of a turn, clockwise.
        let (held, released) = rig.drag(pos2(208.0, 50.0), pos2(258.0, 78.0));
        let Some(Change::Turn { degrees, about }) = held.change else {
            panic!("the turn handle turns: {held:?}");
        };
        assert!((degrees - 45.0).abs() < 1e-3, "{degrees}");
        assert!(near(about, (500.0, 300.0)));
        assert!(released.ended);
        assert!(near(centre(&rig.area()), (500.0, 300.0)), "it turned in place");
        let (w, h) = rig.area().size();
        assert!((w - 480.0).abs() < 0.01 && (h - 270.0).abs() < 0.01);
        // The handle went round with it: up and right of the middle now.
        let outline = Outline::new(&map(), &rig.area(), Rect::EVERYTHING);
        let knob = outline.knob - pos2(208.0, 128.0);
        assert!((knob.x - knob.y.abs()).abs() < 1e-2 && knob.x > 0.0, "{knob:?}");
        // Taken again and carried on round, past the bottom and up the
        // left, a turn counted on from 45: 270 more, to 315.
        rig.move_to(outline.knob);
        rig.button(outline.knob, true);
        for at in [pos2(300.0, 128.0), pos2(208.0, 220.0), pos2(116.0, 128.0)] {
            rig.move_to(at);
        }
        let held = rig.move_to(pos2(158.0, 78.0));
        rig.button(pos2(158.0, 78.0), false);
        let Some(Change::Turn { degrees, .. }) = held.change else {
            panic!("{held:?}");
        };
        assert!((degrees - 315.0).abs() < 1e-3, "{degrees}");
    }

    #[test]
    fn a_turn_snaps_to_right_angles_and_to_steps_with_shift() {
        // Straight right of the middle, a little under it: about 91.5
        // degrees, which lands on a quarter turn.
        let mut rig = Rig::new(small());
        let (held, _) = rig.drag(pos2(208.0, 50.0), pos2(308.0, 130.6));
        assert_eq!(
            held.change
                .map(|c| matches!(c, Change::Turn { degrees, .. } if degrees == 90.0)),
            Some(true),
            "{held:?}"
        );
        // Alt: where the hand put it.
        let mut rig = Rig::new(small());
        rig.modifiers = egui::Modifiers::ALT;
        let (held, _) = rig.drag(pos2(208.0, 50.0), pos2(308.0, 130.6));
        let Some(Change::Turn { degrees, .. }) = held.change else {
            panic!("{held:?}");
        };
        assert!((degrees - 91.5).abs() < 0.05, "{degrees}");
        // Shift: whole steps of 15 degrees, here 37 to 30.
        let mut rig = Rig::new(small());
        rig.modifiers = egui::Modifiers::SHIFT;
        let angle = (-90.0f32 + 37.0).to_radians();
        let to = pos2(208.0, 128.0) + vec2(angle.cos(), angle.sin()) * 60.0;
        let (held, _) = rig.drag(pos2(208.0, 50.0), to);
        assert_eq!(
            held.change
                .map(|c| matches!(c, Change::Turn { degrees, .. } if degrees == 30.0)),
            Some(true),
            "{held:?}"
        );
        // A picture already turned lands on the steps, not 15 from its turn.
        let mut rig = Rig::new(picture((500.0, 300.0), (480.0, 270.0), 7.0));
        rig.modifiers = egui::Modifiers::SHIFT;
        let knob = Outline::new(&map(), &rig.area(), Rect::EVERYTHING).knob;
        let angle = (-90.0f32 + 7.0 + 20.0).to_radians();
        let to = pos2(208.0, 128.0) + vec2(angle.cos(), angle.sin()) * 60.0;
        let (held, _) = rig.drag(knob, to);
        assert_eq!(
            held.change
                .map(|c| matches!(c, Change::Turn { degrees, .. } if degrees == 30.0)),
            Some(true),
            "{held:?}"
        );
    }

    #[test]
    fn dragging_a_side_cuts_it_off_and_leaves_the_rest_where_it_was() {
        // The left side is at 260 pixels, (112, 128) on screen.
        let mut rig = Rig::new(small());
        rig.snapping = false;
        // 19.2 points right is 48 pixels: a tenth of the width.
        rig.move_to(pos2(112.0, 128.0));
        rig.button(pos2(112.0, 128.0), true);
        rig.move_to(pos2(120.0, 140.0));
        let held = rig.move_to(pos2(131.2, 150.0));
        assert!(rig.handles.cropping(), "a side is being dragged");
        let released = rig.button(pos2(131.2, 150.0), false);
        assert!(!rig.handles.cropping());
        let Some(Change::Crop { side, cut }) = held.change else {
            panic!("a side crops: {held:?}");
        };
        assert_eq!(side, Side::Left);
        assert!((cut - 0.1).abs() < 1e-4, "{cut}");
        assert!(released.ended);
        assert!(near(Corner::TopLeft.of(&rig.area()), (308.0, 165.0)));
        assert!(
            near(Corner::BottomRight.of(&rig.area()), (740.0, 435.0)),
            "the rest stays"
        );
        // The bottom side, dragged down past where it was: under 0, given
        // back (the page keeps it to the picture).
        let (held, _) = rig.drag(pos2(208.0, 182.0), pos2(208.0, 192.8));
        let Some(Change::Crop { side, cut }) = held.change else {
            panic!("a side crops: {held:?}");
        };
        assert_eq!(side, Side::Bottom);
        assert!((cut + 0.1).abs() < 1e-4, "{cut}");
    }

    #[test]
    fn a_side_of_a_turned_picture_is_cut_along_the_picture() {
        // A quarter turn: the picture's left side is at the top as it
        // stands, from (365, 60) to (635, 60), its middle (208, 32) on screen.
        let mut rig = Rig::new(picture((500.0, 300.0), (480.0, 270.0), 90.0));
        rig.snapping = false;
        // Down 19.2 points: 48 pixels along the picture's own width.
        let (held, _) = rig.drag(pos2(208.0, 32.0), pos2(208.0, 51.2));
        let Some(Change::Crop { side, cut }) = held.change else {
            panic!("a side crops: {held:?}");
        };
        assert_eq!(side, Side::Left);
        assert!((cut - 0.1).abs() < 1e-4, "{cut}");
        let top = Corner::TopLeft.of(&rig.area());
        assert!(near(top, (635.0, 108.0)), "{top:?}");
    }

    #[test]
    fn a_cut_side_snaps_to_the_frames_centre_line() {
        // A picture filling the frame: its right side, dragged to 4
        // points right of the frame's centre line, lands on it.
        let mut rig = Rig::new(picture((960.0, 540.0), (1920.0, 1080.0), 0.0));
        rig.drag(pos2(776.0, 224.0), pos2(396.0, 224.0));
        assert!(
            near(Corner::TopRight.of(&rig.area()), (960.0, 0.0)),
            "{:?}",
            rig.area()
        );
        assert!((rig.picture.part.width - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_double_click_inside_asks_for_the_fit_and_outside_for_nothing() {
        let mut rig = Rig::new(small());
        let inside = pos2(208.0, 128.0);
        rig.move_to(inside);
        rig.button(inside, true);
        assert_eq!(rig.button(inside, false).change, None, "one click is nothing");
        rig.button(inside, true);
        assert_eq!(rig.button(inside, false).change, Some(Change::Fit));
        assert_eq!(rig.area(), Area::fitted(FRAME, FRAME));

        let mut rig = Rig::new(small());
        let outside = pos2(600.0, 300.0);
        rig.move_to(outside);
        for pressed in [true, false, true, false] {
            assert_eq!(rig.button(outside, pressed), Handled::default());
        }
    }

    #[test]
    fn a_drag_that_begins_outside_the_outline_does_nothing() {
        let start = small();
        let mut rig = Rig::new(start);
        // From the empty frame across the picture.
        let (held, released) = rig.drag(pos2(600.0, 300.0), pos2(208.0, 128.0));
        assert_eq!((held, released), (Handled::default(), Handled::default()));
        assert_eq!(rig.picture, start);
    }

    #[test]
    fn the_transport_over_the_picture_keeps_its_clicks() {
        // A picture that fills the frame lies under the transport, whose
        // play button is centred 36 points above the frame's bottom edge.
        let start = Picture {
            whole: Area::fitted(FRAME, FRAME),
            part: Part::WHOLE,
            degrees: 0.0,
        };
        let mut rig = Rig::new(start);
        let play = pos2(392.0, 404.0);
        rig.move_to(pos2(392.0, 200.0));
        assert_eq!(rig.handles.hot, Some(Grab::Inside));
        rig.move_to(play);
        assert_eq!(rig.handles.hot, None, "the button is in the way");
        // So is the bar between its buttons.
        rig.move_to(pos2(335.0, 404.0));
        assert_eq!(rig.handles.hot, None);
        rig.button(play, true);
        rig.button(play, false);
        assert!(rig.played);
        // Pressing there and pulling away moves nothing.
        rig.drag(play, pos2(300.0, 200.0));
        assert_eq!(rig.picture, start);
        rig.drag(pos2(335.0, 404.0), pos2(300.0, 200.0));
        assert_eq!(rig.picture, start);
        // Beside the transport the picture is taken as anywhere else.
        rig.move_to(pos2(200.0, 404.0));
        assert_eq!(rig.handles.hot, Some(Grab::Inside));
        // With no room above it, the turn handle stands inside the picture.
        rig.move_to(pos2(392.0, 8.0 + TURN_STEM));
        assert_eq!(rig.handles.hot, Some(Grab::Turn));
    }

    #[test]
    fn the_hand_shows_what_it_would_take() {
        let mut rig = Rig::new(small());
        rig.move_to(pos2(208.0, 128.0));
        assert_eq!(rig.handles.hot, Some(Grab::Inside));
        rig.move_to(pos2(304.0, 182.0));
        assert_eq!(rig.handles.hot, Some(Grab::Corner(Corner::BottomRight)));
        rig.move_to(pos2(304.0, 128.0));
        assert_eq!(rig.handles.hot, Some(Grab::Edge(Side::Right)));
        rig.move_to(pos2(208.0, 50.0));
        assert_eq!(rig.handles.hot, Some(Grab::Turn));
        rig.move_to(pos2(600.0, 300.0));
        assert_eq!(rig.handles.hot, None);
    }

    #[test]
    fn letting_go_of_a_picture_that_went_away_ends_the_drag() {
        let mut rig = Rig::new(small());
        let from = pos2(208.0, 128.0);
        rig.move_to(from);
        rig.button(from, true);
        rig.move_to(pos2(260.0, 150.0));
        assert!(rig.handles.dragging());
        rig.handles.let_go();
        assert!(!rig.handles.dragging());
        // The button is still down, but there is nothing to drag.
        assert_eq!(rig.move_to(pos2(300.0, 170.0)).change, None);
    }
}
