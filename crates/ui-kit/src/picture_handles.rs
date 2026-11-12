//! Handles on a picture in the viewer: an outline around the area a picture
//! has in the frame, with a square handle on each corner. Dragging inside
//! the outline moves the picture, dragging a corner resizes it about the
//! opposite corner, and a double-click inside asks for it fitted.
//!
//! This is the part every page with a viewer shares: where the frame's
//! pixels are on screen ([`FrameMap`]), what of an outline is under the
//! pointer ([`grab_at`]), snapping to the frame ([`snap_move`],
//! [`snap_resize`]), and the drag and the drawing ([`Handles`]). It says
//! what the hand did, in frame pixels ([`Change`]); which picture that is,
//! and what the change means for it, is the page's.
//!
//! Everything on screen is in points, as the pointer's place is, so how
//! many pixels the screen has to a point never comes into it.

use crate::Theme;
use egui::{CursorIcon, PointerButton, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2, pos2, vec2};
use render::{Area, Point};

/// How close, in points on screen, a picture must come to a line of the
/// frame to snap to it: the timeline's own reach.
pub const SNAP_REACH: f32 = 8.0;
/// The side of a corner handle, in points.
pub const HANDLE: f32 = 9.0;
/// How far from a corner, in points, the pointer still takes its handle.
pub const HANDLE_REACH: f32 = 9.0;
/// Inside the outline a corner takes at most this share of each edge, so a
/// small picture keeps a middle to be moved by.
const CORNER_SHARE: f32 = 1.0 / 3.0;
/// The smallest a drag makes a picture, as a share of its size when the
/// drag began. The page has its own limits; this only keeps it from
/// turning inside out when the pointer passes the opposite corner.
const LEAST_FACTOR: f32 = 0.001;

/// The viewer's frame on screen, and the timeline's frame it shows: turns
/// places on screen (points) into places in the frame (pixels) and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMap {
    /// The frame's rectangle on screen, in points.
    pub on_screen: Rect,
    /// The timeline's frame, in pixels.
    pub frame: (u32, u32),
}

impl FrameMap {
    fn frame_size(&self) -> (f32, f32) {
        (self.frame.0.max(1) as f32, self.frame.1.max(1) as f32)
    }

    /// The rectangle's size, never nothing, so a viewer folded to no size
    /// divides by something.
    fn screen_size(&self) -> (f32, f32) {
        (
            self.on_screen.width().max(1e-3),
            self.on_screen.height().max(1e-3),
        )
    }

    /// Where a place in the frame is on screen.
    pub fn to_screen(&self, (x, y): Point) -> Pos2 {
        let ((fw, fh), (sw, sh)) = (self.frame_size(), self.screen_size());
        pos2(
            self.on_screen.left() + x * sw / fw,
            self.on_screen.top() + y * sh / fh,
        )
    }

    /// Where a place on screen is in the frame. Outside the rectangle is
    /// outside the frame: under 0 or past its size.
    pub fn to_frame(&self, at: Pos2) -> Point {
        let step = self.step_to_frame(at - self.on_screen.left_top());
        (step.0, step.1)
    }

    /// A movement on screen as one in the frame.
    pub fn step_to_frame(&self, by: Vec2) -> Point {
        let ((fw, fh), (sw, sh)) = (self.frame_size(), self.screen_size());
        (by.x * fw / sw, by.y * fh / sh)
    }

    /// A length in points on screen as frame pixels.
    pub fn pixels(&self, points: f32) -> f32 {
        points * self.frame_size().0 / self.screen_size().0
    }

    /// An area's corners on screen: top left, top right, bottom right and
    /// bottom left of the picture, wherever a turn has put them.
    pub fn corners(&self, area: &Area) -> [Pos2; 4] {
        Corner::ALL.map(|corner| self.to_screen(corner.of(area)))
    }
}

/// A corner of a picture: of the picture itself, so a picture turned
/// upside down has its top left corner at the bottom right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Corner {
    /// In the order [`FrameMap::corners`] gives them: around the outline.
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
    ];

    pub fn opposite(self) -> Corner {
        match self {
            Corner::TopLeft => Corner::BottomRight,
            Corner::TopRight => Corner::BottomLeft,
            Corner::BottomRight => Corner::TopLeft,
            Corner::BottomLeft => Corner::TopRight,
        }
    }

    /// How far along the top edge and down the left edge it is: 0 or 1.
    fn place(self) -> (f32, f32) {
        match self {
            Corner::TopLeft => (0.0, 0.0),
            Corner::TopRight => (1.0, 0.0),
            Corner::BottomRight => (1.0, 1.0),
            Corner::BottomLeft => (0.0, 1.0),
        }
    }

    /// Where this corner of `area` is.
    pub fn of(self, area: &Area) -> Point {
        let (u, v) = self.place();
        (
            area.origin.0 + u * area.across.0 + v * area.down.0,
            area.origin.1 + u * area.across.1 + v * area.down.1,
        )
    }
}

/// What of an outline the pointer has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// The picture itself: dragging moves it.
    Inside,
    /// A corner's handle: dragging resizes the picture.
    Corner(Corner),
}

/// What of an outline with these `corners` (in the order of
/// [`Corner::ALL`], on screen) is at `at`: a corner's handle within `reach`
/// of the corner, else the inside, else nothing. Inside the outline a
/// handle only takes its third of each edge, so there is always a middle.
pub fn grab_at(corners: [Pos2; 4], at: Pos2, reach: f32) -> Option<Grab> {
    let [top_left, top_right, _, bottom_left] = corners;
    let inside = along(top_left, top_right - top_left, bottom_left - top_left, at)
        .filter(|(u, v)| (0.0..=1.0).contains(u) && (0.0..=1.0).contains(v));
    let nearest = Corner::ALL
        .into_iter()
        .zip(corners)
        .map(|(corner, place)| (corner, place.distance(at)))
        .filter(|(_, distance)| *distance <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(corner, _)| corner);
    match (nearest, inside) {
        (Some(corner), None) => Some(Grab::Corner(corner)),
        (Some(corner), Some((u, v))) => {
            let (cu, cv) = corner.place();
            if (u - cu).abs() <= CORNER_SHARE && (v - cv).abs() <= CORNER_SHARE {
                Some(Grab::Corner(corner))
            } else {
                Some(Grab::Inside)
            }
        }
        (None, Some(_)) => Some(Grab::Inside),
        (None, None) => None,
    }
}

/// How far `at` is along `across` and along `down` from `origin`, each as a
/// share of that edge. `None` for an outline with no area.
fn along(origin: Pos2, across: Vec2, down: Vec2, at: Pos2) -> Option<(f32, f32)> {
    let det = across.x * down.y - across.y * down.x;
    if det.abs() < 1e-6 {
        return None;
    }
    let d = at - origin;
    Some((
        (d.x * down.y - d.y * down.x) / det,
        (across.x * d.y - across.y * d.x) / det,
    ))
}

/// A line of the frame shown while a picture is snapped to it, in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Guide {
    /// From the top of the frame to its bottom, this far from its left edge.
    Vertical(f32),
    /// From the left of the frame to its right, this far from its top edge.
    Horizontal(f32),
}

/// The guides in force: at most one down the frame and one across it.
pub type Guides = [Option<Guide>; 2];

/// The upright box around an area: left, top, right, bottom.
fn bounds(area: &Area) -> (f32, f32, f32, f32) {
    let corners = Corner::ALL.map(|corner| corner.of(area));
    let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
    let least = |values: [f32; 4]| values.into_iter().fold(f32::INFINITY, f32::min);
    let most = |values: [f32; 4]| values.into_iter().fold(f32::NEG_INFINITY, f32::max);
    (least(xs), least(ys), most(xs), most(ys))
}

/// Along one side of the frame, `side` long: the step that brings the
/// middle of `low..high` to the frame's middle, or one of its ends to the
/// frame's end on that side, whichever is nearest within `reach`; and the
/// line it lands on. The middle wins when they are as near.
fn snap_along(low: f32, high: f32, side: f32, reach: f32) -> Option<(f32, f32)> {
    [((low + high) / 2.0, side / 2.0), (low, 0.0), (high, side)]
        .into_iter()
        .map(|(from, to)| (to - from, to))
        .filter(|(step, _)| step.abs() <= reach)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
}

/// How much further to move `area` so it lands on the frame's lines within
/// `reach` frame pixels: its centre on the frame's centre lines, its sides
/// on the frame's edges (for a turned picture, the sides of the upright box
/// around it). Across and down snap each on their own. Also the lines it
/// landed on, to show.
pub fn snap_move(area: &Area, frame: (u32, u32), reach: f32) -> (Point, Guides) {
    let (left, top, right, bottom) = bounds(area);
    let across = snap_along(left, right, frame.0 as f32, reach);
    let down = snap_along(top, bottom, frame.1 as f32, reach);
    (
        (across.map_or(0.0, |s| s.0), down.map_or(0.0, |s| s.0)),
        [
            across.map(|s| Guide::Vertical(s.1)),
            down.map(|s| Guide::Horizontal(s.1)),
        ],
    )
}

/// How many times as large a picture becomes when the corner that was at
/// `from` is dragged to `to` while the opposite corner stays at `anchor`:
/// how far `to` is along the line from the anchor through `from`. Moving
/// the pointer across that line changes nothing, so the size follows the
/// hand the same way whatever the picture's turn.
pub fn resize_factor(anchor: Point, from: Point, to: Point) -> f32 {
    let diagonal = (from.0 - anchor.0, from.1 - anchor.1);
    let length_squared = diagonal.0 * diagonal.0 + diagonal.1 * diagonal.1;
    if length_squared < 1e-6 {
        return 1.0;
    }
    let reached = (to.0 - anchor.0) * diagonal.0 + (to.1 - anchor.1) * diagonal.1;
    (reached / length_squared).max(LEAST_FACTOR)
}

/// `factor` (see [`resize_factor`]) adjusted so the dragged corner lands on
/// an edge of the frame when it is within `reach` frame pixels of one, and
/// the edges it landed on. `corner` is where the dragged corner was when
/// the drag began.
pub fn snap_resize(
    anchor: Point,
    corner: Point,
    factor: f32,
    frame: (u32, u32),
    reach: f32,
) -> (f32, Guides) {
    let diagonal = (corner.0 - anchor.0, corner.1 - anchor.1);
    let length = diagonal.0.hypot(diagonal.1);
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    // The factor that puts the corner on each edge it can reach.
    let onto = |edge: f32, from: f32, step: f32| (step.abs() > 1e-3).then(|| (edge - from) / step);
    let snapped = [
        onto(0.0, anchor.0, diagonal.0),
        onto(fw, anchor.0, diagonal.0),
        onto(0.0, anchor.1, diagonal.1),
        onto(fh, anchor.1, diagonal.1),
    ]
    .into_iter()
    .flatten()
    .filter(|wanted| *wanted >= LEAST_FACTOR && (wanted - factor).abs() * length <= reach)
    .min_by(|a, b| (a - factor).abs().total_cmp(&(b - factor).abs()));
    let Some(factor) = snapped else {
        return (factor, [None; 2]);
    };
    // A picture of the frame's shape reaches two edges at once.
    let at = (anchor.0 + factor * diagonal.0, anchor.1 + factor * diagonal.1);
    let on = |place: f32, edge: f32| (place - edge).abs() < 0.05;
    let vertical = [0.0, fw].into_iter().find(|edge| on(at.0, *edge));
    let horizontal = [0.0, fh].into_iter().find(|edge| on(at.1, *edge));
    (
        factor,
        [vertical.map(Guide::Vertical), horizontal.map(Guide::Horizontal)],
    )
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
    /// The picture's area when the drag began, in frame pixels.
    start: Area,
    /// The lines it is snapped to now.
    guides: Guides,
}

impl Drag {
    /// What the drag has done with the pointer at `at`.
    fn change(&mut self, at: Pos2, map: &FrameMap, snapping: bool) -> Change {
        let by = map.step_to_frame(at - self.pressed);
        let reach = map.pixels(SNAP_REACH);
        self.guides = [None; 2];
        match self.grab {
            Grab::Inside => {
                let mut by = by;
                if snapping {
                    let moved = Area {
                        origin: (self.start.origin.0 + by.0, self.start.origin.1 + by.1),
                        ..self.start
                    };
                    let (further, guides) = snap_move(&moved, map.frame, reach);
                    by = (by.0 + further.0, by.1 + further.1);
                    self.guides = guides;
                }
                Change::Move { by }
            }
            Grab::Corner(corner) => {
                let anchor = corner.opposite().of(&self.start);
                let from = corner.of(&self.start);
                // The handle keeps the distance from the pointer it was taken at.
                let mut factor = resize_factor(anchor, from, (from.0 + by.0, from.1 + by.1));
                if snapping {
                    (factor, self.guides) = snap_resize(anchor, from, factor, map.frame, reach);
                }
                Change::Resize { factor, anchor }
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

    /// Gives up the drag under way: the picture it was on is no longer there.
    pub fn let_go(&mut self) {
        self.drag = None;
        self.hot = None;
    }

    /// Reads what the pointer does to the picture at `area` (frame pixels).
    /// `response` is the viewer's own, sensing clicks and drags, so what is
    /// drawn over the viewer afterwards (the transport) keeps its clicks.
    /// `snapping` is the page's switch; Alt held skips it for the moment,
    /// as on the timeline.
    pub fn interact(
        &mut self,
        ui: &Ui,
        response: &Response,
        map: FrameMap,
        area: Area,
        snapping: bool,
    ) -> Handled {
        let corners = map.corners(&area);
        let primary = PointerButton::Primary;
        if response.drag_started_by(primary)
            && let Some(pressed) = ui.input(|i| i.pointer.press_origin())
            && let Some(grab) = grab_at(corners, pressed, HANDLE_REACH)
        {
            self.drag = Some(Drag {
                grab,
                pressed,
                start: area,
                guides: [None; 2],
            });
        }
        let mut handled = Handled::default();
        if let Some(drag) = &mut self.drag {
            handled.busy = response.dragged_by(primary);
            if (handled.busy || response.drag_stopped())
                && let Some(at) = response.interact_pointer_pos()
            {
                let snapping = snapping && !ui.input(|i| i.modifiers.alt);
                handled.change = Some(drag.change(at, &map, snapping));
            }
            self.hot = Some(drag.grab);
            ui.ctx()
                .set_cursor_icon(cursor(drag.grab, &map.corners(&drag.start)));
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
            .and_then(|at| grab_at(corners, at, HANDLE_REACH));
        if let Some(grab) = self.hot {
            ui.ctx().set_cursor_icon(cursor(grab, &corners));
        }
        if response.double_clicked_by(primary)
            && let Some(at) = response.interact_pointer_pos()
            && grab_at(corners, at, HANDLE_REACH) == Some(Grab::Inside)
        {
            handled.change = Some(Change::Fit);
        }
        handled
    }

    /// Draws the outline of `area` (frame pixels) with its corner handles,
    /// and the lines the drag under way is snapped to, clipped to `within`
    /// (the viewer). Call it after [`Handles::interact`], with the area as
    /// it is after what the hand did, so the outline never trails the pointer.
    pub fn draw(&self, ui: &Ui, theme: &Theme, within: Rect, map: FrameMap, area: Area) {
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
        let corners = map.corners(&area);
        painter.add(Shape::closed_line(corners.to_vec(), Stroke::new(3.5, under)));
        painter.add(Shape::closed_line(
            corners.to_vec(),
            Stroke::new(1.5, theme.accent),
        ));
        // The handles are turned with the picture.
        let [top_left, top_right, _, bottom_left] = corners;
        let across = (top_right - top_left).normalized();
        let down = (bottom_left - top_left).normalized();
        let (across, down) = if across == Vec2::ZERO || down == Vec2::ZERO {
            (vec2(1.0, 0.0), vec2(0.0, 1.0))
        } else {
            (across, down)
        };
        for (corner, at) in Corner::ALL.into_iter().zip(corners) {
            let hot = self.hot == Some(Grab::Corner(corner));
            let half = if hot { HANDLE / 2.0 + 1.5 } else { HANDLE / 2.0 };
            let (a, d) = (across * half, down * half);
            painter.add(Shape::convex_polygon(
                vec![at - a - d, at + a - d, at + a + d, at - a + d],
                theme.accent,
                Stroke::new(1.0, theme.accent_ink),
            ));
        }
    }
}

/// The pointer over the inside or a corner of an outline with `corners`.
fn cursor(grab: Grab, corners: &[Pos2; 4]) -> CursorIcon {
    match grab {
        Grab::Inside => CursorIcon::Move,
        Grab::Corner(corner) => {
            let place = |corner: Corner| {
                Corner::ALL
                    .into_iter()
                    .zip(corners)
                    .find(|(c, _)| *c == corner)
                    .map_or(Pos2::ZERO, |(_, at)| *at)
            };
            resize_cursor(place(corner) - place(corner.opposite()))
        }
    }
}

/// The two-headed arrow nearest the way a corner is dragged: `along` is the
/// diagonal on screen, so a turned picture's handles point the way they move.
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

#[cfg(test)]
mod tests {
    use super::*;
    use render::{Part, Placement};

    const FRAME: (u32, u32) = (1920, 1080);

    /// A viewer's frame 768 points wide with its corner at (8, 8): 0.4
    /// points to a frame pixel.
    fn map() -> FrameMap {
        FrameMap {
            on_screen: Rect::from_min_size(pos2(8.0, 8.0), vec2(768.0, 432.0)),
            frame: FRAME,
        }
    }

    fn upright(centre: Point, size: (f32, f32)) -> Area {
        turned(centre, size, 0.0)
    }

    fn turned(centre: Point, size: (f32, f32), degrees: f32) -> Area {
        Area::of_part(
            Placement {
                centre,
                size,
                degrees,
            },
            Part::WHOLE,
        )
    }

    fn near(a: Point, b: Point) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn the_frames_corners_are_the_rectangles_corners_and_places_come_back() {
        let map = map();
        assert_eq!(map.to_screen((0.0, 0.0)), pos2(8.0, 8.0));
        assert_eq!(map.to_screen((1920.0, 1080.0)), pos2(776.0, 440.0));
        assert_eq!(map.to_screen((960.0, 540.0)), pos2(392.0, 224.0));
        assert_eq!(map.to_frame(pos2(8.0, 8.0)), (0.0, 0.0));
        assert_eq!(map.to_frame(pos2(776.0, 440.0)), (1920.0, 1080.0));
        // Outside the rectangle is outside the frame.
        assert!(near(map.to_frame(pos2(0.0, 448.0)), (-20.0, 1100.0)));
        // There and back, also from places that are no whole number of anything.
        for place in [
            (0.0, 0.0),
            (1.0, 1.0),
            (333.3, 777.7),
            (1919.5, 0.25),
            (-250.0, 2000.0),
        ] {
            let back = map.to_frame(map.to_screen(place));
            assert!(near(back, place), "{place:?} came back as {back:?}");
        }
        for at in [pos2(8.0, 8.0), pos2(123.4, 56.7), pos2(900.0, -3.0)] {
            let back = map.to_screen(map.to_frame(at));
            assert!((back - at).length() < 1e-3, "{at:?} came back as {back:?}");
        }
        // A step and a length use the same measure: 2.5 pixels to a point.
        assert!(near(map.step_to_frame(vec2(10.0, -4.0)), (25.0, -10.0)));
        assert!((map.pixels(8.0) - 20.0).abs() < 1e-4);
    }

    #[test]
    fn a_frame_of_another_shape_maps_the_same_way() {
        // A tall timeline in a tall rectangle, a third of a point to a pixel.
        let map = FrameMap {
            on_screen: Rect::from_min_size(pos2(100.0, 20.0), vec2(360.0, 640.0)),
            frame: (1080, 1920),
        };
        assert_eq!(map.to_screen((540.0, 960.0)), pos2(280.0, 340.0));
        assert!(near(map.to_frame(pos2(460.0, 660.0)), (1080.0, 1920.0)));
        assert!(near(map.step_to_frame(vec2(1.0, 1.0)), (3.0, 3.0)));
    }

    #[test]
    fn corners_go_around_the_picture_and_turn_with_it() {
        let area = upright((960.0, 540.0), (400.0, 200.0));
        assert_eq!(Corner::TopLeft.of(&area), (760.0, 440.0));
        assert_eq!(Corner::TopRight.of(&area), (1160.0, 440.0));
        assert_eq!(Corner::BottomRight.of(&area), (1160.0, 640.0));
        assert_eq!(Corner::BottomLeft.of(&area), (760.0, 640.0));
        for corner in Corner::ALL {
            assert_eq!(corner.opposite().opposite(), corner);
            assert_ne!(corner.opposite(), corner);
        }
        // Half a turn: the picture's top left corner is at the bottom right.
        let upside_down = turned((960.0, 540.0), (400.0, 200.0), 180.0);
        assert!(near(Corner::TopLeft.of(&upside_down), (1160.0, 640.0)));
        // On screen they are where the map puts them.
        let on_screen = map().corners(&area);
        assert_eq!(on_screen[0], pos2(312.0, 184.0));
        assert_eq!(on_screen[2], pos2(472.0, 264.0));
    }

    #[test]
    fn the_pointer_takes_a_corner_the_inside_or_nothing() {
        let corners = map().corners(&upright((960.0, 540.0), (400.0, 200.0)));
        // 160 x 80 points on screen, from (312, 184).
        let grab = |x: f32, y: f32| grab_at(corners, pos2(x, y), HANDLE_REACH);
        assert_eq!(grab(392.0, 224.0), Some(Grab::Inside));
        assert_eq!(grab(312.0, 184.0), Some(Grab::Corner(Corner::TopLeft)));
        assert_eq!(grab(476.0, 180.0), Some(Grab::Corner(Corner::TopRight)));
        assert_eq!(grab(468.0, 260.0), Some(Grab::Corner(Corner::BottomRight)));
        assert_eq!(grab(306.0, 270.0), Some(Grab::Corner(Corner::BottomLeft)));
        // Just outside an edge, away from the corners: nothing.
        assert_eq!(grab(392.0, 180.0), None);
        assert_eq!(grab(480.0, 224.0), None);
        // Too far from a corner, outside.
        assert_eq!(grab(300.0, 172.0), None);
        // On the edge itself counts as inside.
        assert_eq!(grab(392.0, 184.0), Some(Grab::Inside));
    }

    #[test]
    fn a_small_picture_keeps_a_middle_to_move_it_by() {
        // 12 x 12 points on screen: every place in it is near a corner.
        let corners = map().corners(&upright((960.0, 540.0), (30.0, 30.0)));
        let grab = |x: f32, y: f32| grab_at(corners, pos2(x, y), HANDLE_REACH);
        assert_eq!(grab(392.0, 224.0), Some(Grab::Inside));
        assert_eq!(grab(387.0, 219.0), Some(Grab::Corner(Corner::TopLeft)));
        assert_eq!(grab(397.0, 229.0), Some(Grab::Corner(Corner::BottomRight)));
        // Outside it the handles reach as far as ever.
        assert_eq!(grab(404.0, 236.0), Some(Grab::Corner(Corner::BottomRight)));
    }

    #[test]
    fn a_turned_picture_is_hit_where_it_is() {
        // 400 x 200 turned by 45 degrees about (960, 540).
        let area = turned((960.0, 540.0), (400.0, 200.0), 45.0);
        let map = map();
        let corners = map.corners(&area);
        let grab = |place: Point| grab_at(corners, map.to_screen(place), HANDLE_REACH);
        assert_eq!(grab((960.0, 540.0)), Some(Grab::Inside));
        // Along the long side from the centre: still inside 190 pixels out,
        // and outside the upright box's corner the turn left empty.
        let step = 190.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(grab((960.0 + step, 540.0 + step)), Some(Grab::Inside));
        assert_eq!(grab((1140.0, 460.0)), None);
        assert_eq!(grab((780.0, 620.0)), None);
        // The corners are where the turn put them.
        for corner in Corner::ALL {
            assert_eq!(grab(corner.of(&area)), Some(Grab::Corner(corner)));
        }
        // The top right corner of the picture is now its rightmost point.
        let top_right = Corner::TopRight.of(&area);
        assert!(top_right.0 > 1160.0 && (top_right.1 - 610.71).abs() < 0.1);
        // An outline with no area takes nothing but its corners.
        let flat = [pos2(10.0, 10.0); 4];
        assert_eq!(grab_at(flat, pos2(100.0, 100.0), HANDLE_REACH), None);
    }

    #[test]
    fn a_moved_picture_snaps_its_centre_and_its_sides_to_the_frame() {
        let reach = 20.0;
        // Centre 12 pixels left of the frame's and 5 under it.
        let (by, guides) = snap_move(&upright((948.0, 545.0), (400.0, 200.0)), FRAME, reach);
        assert!(near(by, (12.0, -5.0)));
        assert_eq!(
            guides,
            [Some(Guide::Vertical(960.0)), Some(Guide::Horizontal(540.0))]
        );
        // Left side 7 pixels past the frame's left edge; down is out of reach.
        let (by, guides) = snap_move(&upright((193.0, 300.0), (400.0, 200.0)), FRAME, reach);
        assert!(near(by, (7.0, 0.0)));
        assert_eq!(guides, [Some(Guide::Vertical(0.0)), None]);
        // Right and bottom sides to the frame's right and bottom edge.
        let (by, guides) = snap_move(&upright((1715.0, 990.0), (400.0, 200.0)), FRAME, reach);
        assert!(near(by, (5.0, -10.0)));
        assert_eq!(
            guides,
            [Some(Guide::Vertical(1920.0)), Some(Guide::Horizontal(1080.0))]
        );
        // Out of reach of everything: left alone.
        let (by, guides) = snap_move(&upright((500.0, 300.0), (400.0, 200.0)), FRAME, reach);
        assert_eq!((by, guides), ((0.0, 0.0), [None; 2]));
        // Just outside the reach, and just inside.
        let (by, _) = snap_move(&upright((939.0, 300.0), (400.0, 200.0)), FRAME, reach);
        assert_eq!(by.0, 0.0);
        let (by, _) = snap_move(&upright((941.0, 300.0), (400.0, 200.0)), FRAME, reach);
        assert!((by.0 - 19.0).abs() < 1e-3);
    }

    #[test]
    fn the_nearest_line_wins_and_the_centre_when_they_are_as_near() {
        // A picture as wide as the frame, 6 pixels right of centred: its
        // centre and both sides are all 6 away; the centre is shown.
        let (by, guides) = snap_move(&upright((966.0, 300.0), (1920.0, 200.0)), FRAME, 20.0);
        assert!(near(by, (-6.0, 0.0)));
        assert_eq!(guides[0], Some(Guide::Vertical(960.0)));
        // Wider than the frame: the left side is 4 from the edge, the
        // centre 14 from the middle. The side wins.
        let (by, guides) = snap_move(&upright((974.0, 300.0), (1940.0, 200.0)), FRAME, 20.0);
        assert!(near(by, (-4.0, 0.0)), "{by:?}");
        assert_eq!(guides[0], Some(Guide::Vertical(0.0)));
    }

    #[test]
    fn a_turned_picture_snaps_by_the_upright_box_around_it() {
        // A quarter turn: 200 wide and 400 high as it stands.
        let area = turned((105.0, 540.0), (400.0, 200.0), 90.0);
        let (by, guides) = snap_move(&area, FRAME, 20.0);
        assert!(near(by, (-5.0, 0.0)), "{by:?}");
        assert_eq!(
            guides,
            [Some(Guide::Vertical(0.0)), Some(Guide::Horizontal(540.0))]
        );
    }

    #[test]
    fn a_corner_dragged_along_the_diagonal_sizes_the_picture() {
        let (anchor, from) = ((100.0, 100.0), (500.0, 300.0));
        assert_eq!(resize_factor(anchor, from, from), 1.0);
        assert!((resize_factor(anchor, from, (900.0, 500.0)) - 2.0).abs() < 1e-6);
        assert!((resize_factor(anchor, from, (300.0, 200.0)) - 0.5).abs() < 1e-6);
        // Across the diagonal changes nothing: (-200, 400) is square to it.
        assert!((resize_factor(anchor, from, (400.0, 500.0)) - 1.0).abs() < 1e-6);
        // Past the anchor it stays the right way round, and tiny.
        assert_eq!(resize_factor(anchor, from, (-500.0, -300.0)), LEAST_FACTOR);
        // A picture with no size has nothing to measure by.
        assert_eq!(resize_factor(anchor, anchor, (900.0, 500.0)), 1.0);
    }

    #[test]
    fn a_resized_corner_snaps_to_the_frames_edges() {
        // Anchored at the top left of the frame, a 16:9 picture: its bottom
        // right corner reaches the right and the bottom edge together.
        let (anchor, corner) = ((0.0, 0.0), (960.0, 540.0));
        let (factor, guides) = snap_resize(anchor, corner, 1.99, FRAME, 20.0);
        assert!((factor - 2.0).abs() < 1e-6);
        assert_eq!(
            guides,
            [Some(Guide::Vertical(1920.0)), Some(Guide::Horizontal(1080.0))]
        );
        // Too far: 0.1 of a 1101-pixel diagonal is 110 pixels.
        assert_eq!(snap_resize(anchor, corner, 1.9, FRAME, 20.0), (1.9, [None; 2]));
        // A wider picture meets the right edge first, and only that.
        let (factor, guides) = snap_resize((100.0, 100.0), (1010.0, 400.0), 2.01, FRAME, 20.0);
        assert!((factor - 2.0).abs() < 1e-6);
        assert_eq!(guides, [Some(Guide::Vertical(1920.0)), None]);
        // Dragged up and left, it meets the frame's left edge.
        let (factor, guides) = snap_resize((1000.0, 800.0), (500.0, 600.0), 1.98, FRAME, 20.0);
        assert!((factor - 2.0).abs() < 1e-6);
        assert_eq!(guides, [Some(Guide::Vertical(0.0)), None]);
    }

    #[test]
    fn handles_point_the_way_they_are_dragged() {
        assert_eq!(resize_cursor(vec2(160.0, 90.0)), CursorIcon::ResizeNwSe);
        assert_eq!(resize_cursor(vec2(-160.0, -90.0)), CursorIcon::ResizeNwSe);
        assert_eq!(resize_cursor(vec2(160.0, -90.0)), CursorIcon::ResizeNeSw);
        assert_eq!(resize_cursor(vec2(200.0, 5.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(vec2(-200.0, 5.0)), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(vec2(3.0, -200.0)), CursorIcon::ResizeVertical);
    }

    /// What a page does with a change: the picture as it is after it.
    fn changed(start: Area, change: Change) -> Area {
        match change {
            Change::Move { by } => Area {
                origin: (start.origin.0 + by.0, start.origin.1 + by.1),
                ..start
            },
            Change::Resize { factor, anchor } => Area {
                origin: (
                    anchor.0 + factor * (start.origin.0 - anchor.0),
                    anchor.1 + factor * (start.origin.1 - anchor.1),
                ),
                across: (start.across.0 * factor, start.across.1 * factor),
                down: (start.down.0 * factor, start.down.1 * factor),
            },
            Change::Fit => Area::fitted(FRAME, FRAME),
        }
    }

    /// A viewer in a window, with a picture that follows the handles.
    struct Rig {
        ctx: egui::Context,
        handles: Handles,
        /// The picture as it is, and as it was when the drag began.
        area: Area,
        start: Option<Area>,
        snapping: bool,
        modifiers: egui::Modifiers,
        /// The transport's play button was clicked.
        played: bool,
    }

    impl Rig {
        fn new(area: Area) -> Rig {
            let mut rig = Rig {
                ctx: egui::Context::default(),
                handles: Handles::default(),
                area,
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
                            .interact(ui, &response, map, self.area, self.snapping);
                        if let Some(change) = handled.change {
                            let start = *self.start.get_or_insert(self.area);
                            self.area = changed(start, change);
                        }
                        if !handled.busy {
                            self.start = None;
                        }
                        self.handles.draw(ui, &Theme::ASH, response.rect, map, self.area);
                        let shortcuts = crate::ShortcutTable::default();
                        let click = crate::transport::transport(ui, &Theme::ASH, frame, false, &shortcuts);
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

        fn centre(&self) -> Point {
            let (left, top, right, bottom) = bounds(&self.area);
            ((left + right) / 2.0, (top + bottom) / 2.0)
        }
    }

    #[test]
    fn dragging_inside_moves_the_picture_with_the_pointer() {
        // 480 x 270 pixels, centred at (500, 300): on screen its centre is
        // at (208, 128).
        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
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
        let start = upright((500.0, 300.0), (480.0, 270.0));
        // The frame's centre is at (392, 224) on screen: 184 right and 96
        // down from the picture's. Stop 5 points short each way.
        let (from, to) = (pos2(208.0, 128.0), pos2(387.0, 219.0));
        let mut rig = Rig::new(start);
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
        let mut rig = Rig::new(start);
        rig.modifiers = egui::Modifiers::ALT;
        rig.drag(from, to);
        assert!(near(rig.centre(), (947.5, 527.5)), "{:?}", rig.centre());

        // The page's snapping switched off: the same.
        let mut rig = Rig::new(start);
        rig.snapping = false;
        rig.drag(from, to);
        assert!(near(rig.centre(), (947.5, 527.5)), "{:?}", rig.centre());
    }

    #[test]
    fn dragging_a_corner_resizes_about_the_opposite_corner() {
        // Top left corner at (260, 165) pixels; bottom right at (740, 435),
        // which is (304, 182) on screen.
        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
        rig.snapping = false;
        // Out along the diagonal by half its length again: 96 x 54 points.
        let (held, released) = rig.drag(pos2(306.0, 184.0), pos2(402.0, 238.0));
        let Some(Change::Resize { factor, anchor }) = held.change else {
            panic!("a corner resizes: {held:?}");
        };
        assert!((factor - 1.5).abs() < 1e-4, "{factor}");
        assert!(near(anchor, (260.0, 165.0)));
        assert!(released.ended);
        assert!(near(Corner::TopLeft.of(&rig.area), (260.0, 165.0)));
        assert!(near(Corner::BottomRight.of(&rig.area), (980.0, 570.0)));
        // The shape is the same.
        let (w, h) = rig.area.size();
        assert!((w / h - 16.0 / 9.0).abs() < 1e-4);

        // The top left corner, dragged in: the bottom right stays.
        let (held, _) = rig.drag(pos2(104.0, 74.0), pos2(200.0, 128.0));
        let Some(Change::Resize { factor, anchor }) = held.change else {
            panic!("a corner resizes: {held:?}");
        };
        assert!((factor - 2.0 / 3.0).abs() < 1e-4, "{factor}");
        assert!(near(anchor, (980.0, 570.0)));
        assert!(near(Corner::TopLeft.of(&rig.area), (500.0, 300.0)));
    }

    #[test]
    fn a_turned_picture_resizes_along_its_own_diagonal() {
        // A quarter turn: the picture's bottom right corner is at the
        // bottom left as it stands, at (365, 540) pixels: (154, 224) on screen.
        let start = turned((500.0, 300.0), (480.0, 270.0), 90.0);
        assert!(near(Corner::BottomRight.of(&start), (365.0, 540.0)));
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
        let (w, h) = rig.area.size();
        assert!((w - 960.0).abs() < 0.5 && (h - 540.0).abs() < 0.5);
        // Still a quarter turn: its top edge runs straight down.
        assert!(rig.area.across.0.abs() < 1e-2 && rig.area.across.1 > 0.0);
    }

    #[test]
    fn a_resize_snaps_its_corner_to_the_frames_edge() {
        // Its top left corner on the frame's; the bottom right at
        // (960, 540), the frame's centre: (392, 224) on screen.
        let mut rig = Rig::new(upright((480.0, 270.0), (960.0, 540.0)));
        // To 4 points short of the frame's bottom right corner.
        rig.drag(pos2(392.0, 224.0), pos2(772.0, 437.75));
        assert!(near(Corner::BottomRight.of(&rig.area), (1920.0, 1080.0)));
        assert!(near(Corner::TopLeft.of(&rig.area), (0.0, 0.0)));
    }

    #[test]
    fn a_double_click_inside_asks_for_the_fit_and_outside_for_nothing() {
        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
        let inside = pos2(208.0, 128.0);
        rig.move_to(inside);
        rig.button(inside, true);
        assert_eq!(rig.button(inside, false).change, None, "one click is nothing");
        rig.button(inside, true);
        assert_eq!(rig.button(inside, false).change, Some(Change::Fit));
        assert_eq!(rig.area, Area::fitted(FRAME, FRAME));

        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
        let outside = pos2(600.0, 300.0);
        rig.move_to(outside);
        for pressed in [true, false, true, false] {
            assert_eq!(rig.button(outside, pressed), Handled::default());
        }
    }

    #[test]
    fn a_drag_that_begins_outside_the_outline_does_nothing() {
        let start = upright((500.0, 300.0), (480.0, 270.0));
        let mut rig = Rig::new(start);
        // From the empty frame across the picture.
        let (held, released) = rig.drag(pos2(600.0, 300.0), pos2(208.0, 128.0));
        assert_eq!((held, released), (Handled::default(), Handled::default()));
        assert_eq!(rig.area, start);
    }

    #[test]
    fn the_transport_over_the_picture_keeps_its_clicks() {
        // A picture that fills the frame lies under the transport, whose
        // play button is centred 36 points above the frame's bottom edge.
        let start = Area::fitted(FRAME, FRAME);
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
        assert_eq!(rig.area, start);
        rig.drag(pos2(335.0, 404.0), pos2(300.0, 200.0));
        assert_eq!(rig.area, start);
        // Beside the transport the picture is taken as anywhere else.
        rig.move_to(pos2(200.0, 404.0));
        assert_eq!(rig.handles.hot, Some(Grab::Inside));
    }

    #[test]
    fn the_hand_shows_what_it_would_take() {
        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
        rig.move_to(pos2(208.0, 128.0));
        assert_eq!(rig.handles.hot, Some(Grab::Inside));
        rig.move_to(pos2(304.0, 182.0));
        assert_eq!(rig.handles.hot, Some(Grab::Corner(Corner::BottomRight)));
        rig.move_to(pos2(600.0, 300.0));
        assert_eq!(rig.handles.hot, None);
    }

    #[test]
    fn letting_go_of_a_picture_that_went_away_ends_the_drag() {
        let mut rig = Rig::new(upright((500.0, 300.0), (480.0, 270.0)));
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
