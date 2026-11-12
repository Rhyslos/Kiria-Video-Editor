//! Where a picture's outline and its handles are on screen, and which of
//! them the pointer is on: the frame's place on screen ([`FrameMap`]), the
//! corners and sides of a picture ([`Corner`], [`Side`]), the handles drawn
//! for it ([`Outline`]) and what a press takes hold of ([`grab_at`]).

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use render::{Area, Point};

/// The side of a corner handle, in points.
pub const HANDLE: f32 = 9.0;
/// How far from a corner, or from the turn handle, in points, the pointer
/// still takes it.
pub const HANDLE_REACH: f32 = 9.0;
/// How far from a side, in points, the pointer takes its crop handle.
pub const EDGE_REACH: f32 = 7.0;
/// How far the turn handle stands off the picture's top edge, in points.
pub const TURN_STEM: f32 = 24.0;
/// The turn handle's radius, in points.
pub const KNOB: f32 = 5.0;
/// Inside the outline a corner takes at most this share of each edge, so a
/// small picture keeps a middle to be moved by; a side's handle takes the
/// middle third of the side.
const CORNER_SHARE: f32 = 1.0 / 3.0;
/// Inside the outline a side's handle takes at most this share of the
/// picture's depth, so a thin picture keeps a middle too.
const EDGE_SHARE: f32 = 0.25;

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
    pub(super) fn place(self) -> (f32, f32) {
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

/// A side of a picture: of the picture itself, as [`Corner`]s are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Top, Side::Right, Side::Bottom];

    /// The corners at its ends.
    pub fn ends(self) -> (Corner, Corner) {
        match self {
            Side::Left => (Corner::BottomLeft, Corner::TopLeft),
            Side::Top => (Corner::TopLeft, Corner::TopRight),
            Side::Right => (Corner::TopRight, Corner::BottomRight),
            Side::Bottom => (Corner::BottomRight, Corner::BottomLeft),
        }
    }

    /// Whether it runs down the picture (left and right) rather than across.
    pub fn runs_down(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }

    /// Whether cutting it off moves it the way the picture's own edge from
    /// the top left corner runs (left and top), rather than back (right and
    /// bottom).
    pub fn cuts_forward(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

/// What of an outline the pointer has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// The picture itself: dragging moves it.
    Inside,
    /// A corner's handle: dragging resizes the picture.
    Corner(Corner),
    /// A side's handle: dragging cuts that side off, or gives it back.
    Edge(Side),
    /// The turn handle: dragging turns the picture.
    Turn,
}

/// An outline's handles on screen: its corners (in the order of
/// [`Corner::ALL`]) and the turn handle on its stem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outline {
    pub corners: [Pos2; 4],
    /// Where the turn handle's stem leaves the picture: the middle of its
    /// top edge.
    pub stem: Pos2,
    pub knob: Pos2,
}

impl Outline {
    /// The handles of `area` (frame pixels) on screen. The turn handle
    /// stands off the picture's top edge, away from the picture, unless
    /// that is outside `within` (the viewer, where the pointer can reach
    /// it); then it stands inside the picture instead.
    pub fn new(map: &FrameMap, area: &Area, within: Rect) -> Outline {
        let corners = map.corners(area);
        let [top_left, top_right, _, bottom_left] = corners;
        let stem = top_left + (top_right - top_left) / 2.0;
        let up = (top_left - bottom_left).normalized();
        let up = if up == Vec2::ZERO { vec2(0.0, -1.0) } else { up };
        let outside = stem + up * TURN_STEM;
        let knob = if within.shrink(KNOB + 1.0).contains(outside) {
            outside
        } else {
            stem - up * TURN_STEM
        };
        Outline { corners, stem, knob }
    }

    pub fn corner(&self, corner: Corner) -> Pos2 {
        let (u, v) = corner.place();
        let [top_left, top_right, _, bottom_left] = self.corners;
        top_left + (top_right - top_left) * u + (bottom_left - top_left) * v
    }

    /// The middle of a side, where its handle is.
    pub fn middle(&self, side: Side) -> Pos2 {
        let (a, b) = side.ends();
        let (a, b) = (self.corner(a), self.corner(b));
        a + (b - a) / 2.0
    }
}

/// What of `outline` is at `at`: the turn handle within [`HANDLE_REACH`] of
/// it, a corner's handle within [`HANDLE_REACH`] of the corner, a side's
/// handle within [`EDGE_REACH`] of the middle third of the side, else the
/// inside, else nothing. Inside the outline a corner only takes its third
/// of each edge and a side a quarter of the depth, so there is always a
/// middle to move the picture by.
pub fn grab_at(outline: &Outline, at: Pos2) -> Option<Grab> {
    if outline.knob.distance(at) <= HANDLE_REACH {
        return Some(Grab::Turn);
    }
    let corners = outline.corners;
    let [top_left, top_right, _, bottom_left] = corners;
    let shares = along(top_left, top_right - top_left, bottom_left - top_left, at);
    let inside = shares.filter(|(u, v)| (0.0..=1.0).contains(u) && (0.0..=1.0).contains(v));
    let nearest = Corner::ALL
        .into_iter()
        .zip(corners)
        .map(|(corner, place)| (corner, place.distance(at)))
        .filter(|(_, distance)| *distance <= HANDLE_REACH)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(corner, _)| corner);
    if let Some(corner) = nearest {
        let (cu, cv) = corner.place();
        let in_its_corner =
            inside.is_none_or(|(u, v)| (u - cu).abs() <= CORNER_SHARE && (v - cv).abs() <= CORNER_SHARE);
        if in_its_corner {
            return Some(Grab::Corner(corner));
        }
    }
    let size = (top_left.distance(top_right), top_left.distance(bottom_left));
    if let Some(side) = shares.and_then(|shares| edge_at(shares, size)) {
        return Some(Grab::Edge(side));
    }
    inside.map(|_| Grab::Inside)
}

/// The side whose handle is at `(u, v)` (shares along the top edge and
/// down the left edge) of an outline `size` points across and down: the
/// nearest whose middle third is within [`EDGE_REACH`].
fn edge_at((u, v): (f32, f32), (width, height): (f32, f32)) -> Option<Side> {
    Side::ALL
        .into_iter()
        .filter_map(|side| {
            // How far in from the side, how far along it, its length and
            // how deep the picture is from it.
            let (inward, along, length, depth) = match side {
                Side::Left => (u * width, v, height, width),
                Side::Top => (v * height, u, width, height),
                Side::Right => ((1.0 - u) * width, v, height, width),
                Side::Bottom => ((1.0 - v) * height, u, width, height),
            };
            let reach_along = HANDLE_REACH.max(length * (0.5 - CORNER_SHARE));
            let near = inward >= -EDGE_REACH
                && inward <= EDGE_REACH.min(depth * EDGE_SHARE)
                && (along - 0.5).abs() * length <= reach_along;
            near.then_some((side, inward.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(side, _)| side)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture_handles::fixtures::{map, near, turned, upright};

    /// A viewer much larger than the frame, so the turn handle stands
    /// outside the picture wherever it is.
    fn roomy() -> Rect {
        Rect::from_min_size(pos2(-1000.0, -1000.0), vec2(3000.0, 3000.0))
    }

    fn outline(area: &Area) -> Outline {
        Outline::new(&map(), area, roomy())
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
    fn the_pointer_takes_a_corner_a_side_the_inside_or_nothing() {
        let handles = outline(&upright((960.0, 540.0), (400.0, 200.0)));
        // 160 x 80 points on screen, from (312, 184).
        let grab = |x: f32, y: f32| grab_at(&handles, pos2(x, y));
        assert_eq!(grab(392.0, 224.0), Some(Grab::Inside));
        assert_eq!(grab(312.0, 184.0), Some(Grab::Corner(Corner::TopLeft)));
        assert_eq!(grab(476.0, 180.0), Some(Grab::Corner(Corner::TopRight)));
        assert_eq!(grab(468.0, 260.0), Some(Grab::Corner(Corner::BottomRight)));
        assert_eq!(grab(306.0, 270.0), Some(Grab::Corner(Corner::BottomLeft)));
        // On a side's middle third, either side of the line: its handle.
        assert_eq!(grab(392.0, 184.0), Some(Grab::Edge(Side::Top)));
        assert_eq!(grab(392.0, 180.0), Some(Grab::Edge(Side::Top)));
        assert_eq!(grab(410.0, 189.0), Some(Grab::Edge(Side::Top)));
        assert_eq!(grab(466.0, 224.0), Some(Grab::Edge(Side::Right)));
        assert_eq!(grab(392.0, 268.0), Some(Grab::Edge(Side::Bottom)));
        assert_eq!(grab(306.0, 230.0), Some(Grab::Edge(Side::Left)));
        // Further out than its reach, or along the side near a corner: not.
        assert_eq!(grab(392.0, 175.0), None);
        assert_eq!(grab(480.0, 224.0), None);
        assert_eq!(grab(340.0, 180.0), None);
        // Too far from a corner, outside.
        assert_eq!(grab(300.0, 172.0), None);
        // Inside, further than the side's reach from it: the picture.
        assert_eq!(grab(392.0, 192.0), Some(Grab::Inside));
    }

    #[test]
    fn a_small_picture_keeps_a_middle_to_move_it_by() {
        // 12 x 12 points on screen: every place in it is near a corner.
        let handles = outline(&upright((960.0, 540.0), (30.0, 30.0)));
        let grab = |x: f32, y: f32| grab_at(&handles, pos2(x, y));
        assert_eq!(grab(392.0, 224.0), Some(Grab::Inside));
        assert_eq!(grab(387.0, 219.0), Some(Grab::Corner(Corner::TopLeft)));
        assert_eq!(grab(397.0, 229.0), Some(Grab::Corner(Corner::BottomRight)));
        // Outside it the handles reach as far as ever.
        assert_eq!(grab(404.0, 236.0), Some(Grab::Corner(Corner::BottomRight)));
        // Its sides' handles are under the corners' there.
        assert_eq!(grab(392.0, 214.0), Some(Grab::Corner(Corner::TopLeft)));
        // A wide, thin picture: 160 x 8 points. Its middle is the inside.
        let thin = outline(&upright((960.0, 540.0), (400.0, 20.0)));
        assert_eq!(grab_at(&thin, pos2(392.0, 224.0)), Some(Grab::Inside));
        assert_eq!(grab_at(&thin, pos2(392.0, 217.0)), Some(Grab::Edge(Side::Top)));
    }

    #[test]
    fn a_turned_picture_is_hit_where_it_is() {
        // 400 x 200 turned by 45 degrees about (960, 540).
        let area = turned((960.0, 540.0), (400.0, 200.0), 45.0);
        let map = map();
        let handles = outline(&area);
        let grab = |place: Point| grab_at(&handles, map.to_screen(place));
        assert_eq!(grab((960.0, 540.0)), Some(Grab::Inside));
        // Along the long side from the centre: still inside 150 pixels out,
        // and outside the upright box's corner the turn left empty.
        let step = 150.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(grab((960.0 + step, 540.0 + step)), Some(Grab::Inside));
        assert_eq!(grab((1140.0, 460.0)), None);
        assert_eq!(grab((780.0, 620.0)), None);
        // The corners are where the turn put them.
        for corner in Corner::ALL {
            assert_eq!(grab(corner.of(&area)), Some(Grab::Corner(corner)));
        }
        // And the sides' middles.
        for side in Side::ALL {
            let (a, b) = side.ends();
            let (a, b) = (a.of(&area), b.of(&area));
            let middle = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            assert_eq!(grab(middle), Some(Grab::Edge(side)), "{side:?}");
            assert!((handles.middle(side) - map.to_screen(middle)).length() < 1e-3);
        }
        // The top right corner of the picture is now its rightmost point.
        let top_right = Corner::TopRight.of(&area);
        assert!(top_right.0 > 1160.0 && (top_right.1 - 610.71).abs() < 0.1);
        // An outline with no area takes nothing but its handles.
        let flat = Outline {
            corners: [pos2(10.0, 10.0); 4],
            stem: pos2(10.0, 10.0),
            knob: pos2(10.0, -14.0),
        };
        assert_eq!(grab_at(&flat, pos2(100.0, 100.0)), None);
    }

    #[test]
    fn the_turn_handle_stands_off_the_top_edge_and_turns_with_the_picture() {
        let area = upright((960.0, 540.0), (400.0, 200.0));
        let handles = outline(&area);
        // The top edge's middle is (392, 184) on screen.
        assert_eq!(handles.stem, pos2(392.0, 184.0));
        assert_eq!(handles.knob, pos2(392.0, 184.0 - TURN_STEM));
        assert_eq!(grab_at(&handles, handles.knob), Some(Grab::Turn));
        assert_eq!(grab_at(&handles, handles.knob + vec2(6.0, 5.0)), Some(Grab::Turn));
        // Between it and the picture: nothing.
        assert_eq!(grab_at(&handles, pos2(392.0, 172.0)), None);
        // Upside down, it is under the picture.
        let upside_down = outline_in(&turned((960.0, 540.0), (400.0, 200.0), 180.0), roomy());
        assert!((upside_down.knob - pos2(392.0, 264.0 + TURN_STEM)).length() < 1e-3);
        // A quarter turn: the picture's top is on the right.
        let quarter = outline_in(&turned((960.0, 540.0), (400.0, 200.0), 90.0), roomy());
        assert!((quarter.knob - pos2(432.0 + TURN_STEM, 224.0)).length() < 1e-3);
    }

    fn outline_in(area: &Area, within: Rect) -> Outline {
        Outline::new(&map(), area, within)
    }

    #[test]
    fn a_turn_handle_that_would_be_out_of_reach_stands_inside() {
        // A picture filling the frame, in a viewer 8 points larger all round:
        // above its top edge there is no room.
        let area = upright((960.0, 540.0), (1920.0, 1080.0));
        let viewer = Rect::from_min_size(Pos2::ZERO, vec2(784.0, 448.0));
        let handles = outline_in(&area, viewer);
        assert_eq!(handles.knob, pos2(392.0, 8.0 + TURN_STEM));
        assert_eq!(grab_at(&handles, handles.knob), Some(Grab::Turn));
        // With room, it stands outside.
        let small = upright((960.0, 540.0), (400.0, 200.0));
        assert_eq!(outline_in(&small, viewer).knob, pos2(392.0, 184.0 - TURN_STEM));
    }
}
