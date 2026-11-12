//! What a drag on a picture's handles does, in frame pixels, and the
//! snapping that goes with it: a move ([`snap_move`]), a resize
//! ([`resize_factor`], [`snap_resize`]), a turn ([`turned_by`],
//! [`snap_turn`]) and a cut side ([`crop_step`], [`snap_crop`]). Plain
//! numbers, nothing on screen.

use super::Side;
use super::grab::Corner;
use render::{Area, Part, Point};

/// The smallest a drag makes a picture, as a share of its size when the
/// drag began. The page has its own limits; this only keeps it from
/// turning inside out when the pointer passes the opposite corner.
pub(super) const LEAST_FACTOR: f32 = 0.001;
/// With Shift held a turn goes in steps of this many degrees.
pub const TURN_STEP: f64 = 15.0;
/// A turn this many degrees or less from upright, a quarter turn or half a
/// turn lands on it.
pub const TURN_MAGNET: f64 = 2.0;

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
pub(super) fn bounds(area: &Area) -> (f32, f32, f32, f32) {
    let corners = Corner::ALL.map(|corner| corner.of(area));
    let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
    let least = |values: [f32; 4]| values.into_iter().fold(f32::INFINITY, f32::min);
    let most = |values: [f32; 4]| values.into_iter().fold(f32::NEG_INFINITY, f32::max);
    (least(xs), least(ys), most(xs), most(ys))
}

/// The middle of an area, wherever a turn has put it.
pub fn centre(area: &Area) -> Point {
    (
        area.origin.0 + (area.across.0 + area.down.0) / 2.0,
        area.origin.1 + (area.across.1 + area.down.1) / 2.0,
    )
}

/// Where `part` of a picture whose whole is at `whole` lands: the part
/// keeps its place within the whole, turned with it.
pub fn part_of(whole: &Area, part: Part) -> Area {
    let at = |u: f32, v: f32| {
        (
            whole.origin.0 + u * whole.across.0 + v * whole.down.0,
            whole.origin.1 + u * whole.across.1 + v * whole.down.1,
        )
    };
    Area {
        origin: at(part.left, part.top),
        across: (whole.across.0 * part.width, whole.across.1 * part.width),
        down: (whole.down.0 * part.height, whole.down.1 * part.height),
    }
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

/// How many degrees, clockwise (y runs down), the pointer turned about
/// `pivot` going from `from` to `to`: the shorter way round, so more than
/// -180 and at most 180. Nothing when either is on the pivot.
pub fn turned_by(pivot: Point, from: Point, to: Point) -> f64 {
    let direction = |(x, y): Point| f64::from(y - pivot.1).atan2(f64::from(x - pivot.0));
    let on_pivot = |(x, y): Point| (x - pivot.0).hypot(y - pivot.1) < 1e-6;
    if on_pivot(from) || on_pivot(to) {
        return 0.0;
    }
    let degrees = (direction(to) - direction(from)).to_degrees();
    // Into (-180, 180].
    let wrapped = (degrees + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped == -180.0 { 180.0 } else { wrapped }
}

/// A turn of `degrees` as it lands: with `steps` (Shift held) on the
/// nearest whole step of [`TURN_STEP`]; otherwise, with `magnet` (snapping
/// on and Alt not held), on upright or a quarter, half or three-quarter
/// turn when within [`TURN_MAGNET`] of it; otherwise where it is.
pub fn snap_turn(degrees: f64, steps: bool, magnet: bool) -> f64 {
    let nearest = |step: f64| (degrees / step).round() * step;
    if steps {
        return nearest(TURN_STEP);
    }
    let right_angle = nearest(90.0);
    if magnet && (degrees - right_angle).abs() <= TURN_MAGNET {
        right_angle
    } else {
        degrees
    }
}

/// How much more of `side` a drag of `by` (frame pixels) cuts off a picture
/// whose whole is at `whole`, as a share of the whole picture's width (left
/// and right) or height (top and bottom): how far the hand went across the
/// side, into the picture, measured along the picture's own edge, so it
/// holds however the picture is turned. Under 0 gives some back.
pub fn crop_step(whole: &Area, side: Side, by: Point) -> f32 {
    let axis = axis(whole, side);
    let length_squared = axis.0 * axis.0 + axis.1 * axis.1;
    if length_squared < 1e-6 {
        return 0.0;
    }
    let share = (by.0 * axis.0 + by.1 * axis.1) / length_squared;
    if side.cuts_forward() { share } else { -share }
}

/// The picture's edge a side is measured along: across for left and
/// right, down for top and bottom.
fn axis(whole: &Area, side: Side) -> Point {
    if side.runs_down() {
        whole.across
    } else {
        whole.down
    }
}

/// `cut` (see [`crop_step`]) adjusted so the cut side of a picture whose
/// whole is at `whole` and whose seen `part` it is taken from lands on an
/// edge or a centre line of the frame within `reach` frame pixels, and the
/// line it landed on. Only for a side that lies along the frame's lines
/// (the picture upright, or turned by a quarter, half or three quarters);
/// a slanted side is left where the hand put it.
pub fn snap_crop(
    whole: &Area,
    part: Part,
    side: Side,
    cut: f32,
    frame: (u32, u32),
    reach: f32,
) -> (f32, Guides) {
    let a = axis(whole, side);
    let length = a.0.hypot(a.1);
    if length < 1e-3 {
        return (cut, [None; 2]);
    }
    // The frame's direction the side moves along, if it moves along one.
    let x = if a.1.abs() <= length * 1e-3 {
        true
    } else if a.0.abs() <= length * 1e-3 {
        false
    } else {
        return (cut, [None; 2]);
    };
    // Where the side is, as a share along the axis, with `cut` taken off.
    let sign = if side.cuts_forward() { 1.0 } else { -1.0 };
    let base = match side {
        Side::Left => part.left,
        Side::Top => part.top,
        Side::Right => part.left + part.width,
        Side::Bottom => part.top + part.height,
    };
    let share = base + sign * cut;
    // The side's place along that direction of the frame.
    let (origin, step, side_length) = if x {
        (whole.origin.0, a.0, frame.0 as f32)
    } else {
        (whole.origin.1, a.1, frame.1 as f32)
    };
    let place = origin + share * step;
    let snapped = [0.0, side_length / 2.0, side_length]
        .into_iter()
        .filter(|line| (line - place).abs() <= reach)
        .min_by(|a, b| (a - place).abs().total_cmp(&(b - place).abs()));
    let Some(line) = snapped else {
        return (cut, [None; 2]);
    };
    let cut = cut + sign * (line - place) / step;
    let guides = if x {
        [Some(Guide::Vertical(line)), None]
    } else {
        [None, Some(Guide::Horizontal(line))]
    };
    (cut, guides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture_handles::fixtures::{FRAME, near, turned, upright};
    use render::Placement;

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
    fn a_part_lands_where_the_renderer_puts_it_turned_or_not() {
        let part = Part {
            left: 0.2,
            top: 0.1,
            width: 0.5,
            height: 0.7,
        };
        for degrees in [0.0, 30.0, 90.0, 200.0] {
            let placement = Placement {
                centre: (700.0, 400.0),
                size: (800.0, 450.0),
                degrees,
            };
            let whole = Area::of_part(placement, Part::WHOLE);
            let ours = part_of(&whole, part);
            let theirs = Area::of_part(placement, part);
            for corner in Corner::ALL {
                assert!(
                    near(corner.of(&ours), corner.of(&theirs)),
                    "{degrees}: {corner:?}"
                );
            }
        }
        assert_eq!(centre(&upright((700.0, 400.0), (800.0, 450.0))), (700.0, 400.0));
        assert!(near(
            centre(&turned((700.0, 400.0), (800.0, 450.0), 37.0)),
            (700.0, 400.0)
        ));
    }

    #[test]
    fn the_pointer_turns_the_picture_by_the_angle_it_goes_round_the_pivot() {
        let pivot = (100.0, 100.0);
        // From straight up to straight right is a quarter turn clockwise
        // on a screen, where y runs down.
        assert!((turned_by(pivot, (100.0, 0.0), (200.0, 100.0)) - 90.0).abs() < 1e-9);
        assert!((turned_by(pivot, (200.0, 100.0), (100.0, 0.0)) + 90.0).abs() < 1e-9);
        // How far from the pivot doesn't matter.
        assert!((turned_by(pivot, (100.0, 90.0), (300.0, 300.0)) - 135.0).abs() < 1e-9);
        // The shorter way round: across the left goes through 180, not -180.
        let a = turned_by(pivot, (0.0, 99.0), (0.0, 101.0));
        assert!(a < 0.0 && a > -2.0, "{a}");
        let b = turned_by(pivot, (0.0, 101.0), (0.0, 99.0));
        assert!(b > 0.0 && b < 2.0, "{b}");
        // Straight through the pivot is half a turn.
        assert!((turned_by(pivot, (100.0, 0.0), (100.0, 200.0)).abs() - 180.0).abs() < 1e-9);
        // On the pivot there is no direction: nothing.
        assert_eq!(turned_by(pivot, pivot, (200.0, 100.0)), 0.0);
        assert_eq!(turned_by(pivot, (200.0, 100.0), pivot), 0.0);
    }

    #[test]
    fn a_turn_snaps_to_steps_with_shift_and_to_right_angles_when_near() {
        // Shift: whole steps of 15 degrees, from anywhere.
        assert_eq!(snap_turn(37.4, true, false), 30.0);
        assert_eq!(snap_turn(37.6, true, true), 45.0);
        assert_eq!(snap_turn(-8.0, true, true), -15.0);
        assert_eq!(snap_turn(361.0, true, false), 360.0);
        // Near a right angle it lands on it, within two degrees.
        assert_eq!(snap_turn(1.5, false, true), 0.0);
        assert_eq!(snap_turn(-1.9, false, true), 0.0);
        assert_eq!(snap_turn(88.2, false, true), 90.0);
        assert_eq!(snap_turn(181.0, false, true), 180.0);
        assert_eq!(snap_turn(-271.5, false, true), -270.0);
        // Further off, or with snapping off (Alt): where the hand put it.
        assert_eq!(snap_turn(2.5, false, true), 2.5);
        assert_eq!(snap_turn(87.0, false, true), 87.0);
        assert_eq!(snap_turn(1.5, false, false), 1.5);
        assert_eq!(snap_turn(37.25, false, true), 37.25);
    }

    #[test]
    fn dragging_a_side_cuts_a_share_of_the_whole_picture() {
        // 800 x 400 pixels, upright.
        let whole = upright((960.0, 540.0), (800.0, 400.0));
        // The left side dragged 80 pixels right: a tenth of the width off.
        assert!((crop_step(&whole, Side::Left, (80.0, 30.0)) - 0.1).abs() < 1e-6);
        // The right side dragged the same way gives a tenth back.
        assert!((crop_step(&whole, Side::Right, (80.0, 0.0)) + 0.1).abs() < 1e-6);
        // Top down 40 pixels, bottom up 100: a tenth and a quarter off.
        assert!((crop_step(&whole, Side::Top, (500.0, 40.0)) - 0.1).abs() < 1e-6);
        assert!((crop_step(&whole, Side::Bottom, (0.0, -100.0)) - 0.25).abs() < 1e-6);
        // A picture with no size cuts nothing.
        let flat = Area {
            origin: (0.0, 0.0),
            across: (0.0, 0.0),
            down: (0.0, 0.0),
        };
        assert_eq!(crop_step(&flat, Side::Left, (10.0, 10.0)), 0.0);
    }

    #[test]
    fn a_side_of_a_turned_picture_is_cut_along_the_picture_not_the_frame() {
        // A quarter turn: the picture's left side is at the top, and its
        // width runs down the frame.
        let whole = turned((960.0, 540.0), (800.0, 400.0), 90.0);
        assert!((crop_step(&whole, Side::Left, (0.0, 80.0)) - 0.1).abs() < 1e-5);
        assert!(crop_step(&whole, Side::Left, (80.0, 0.0)).abs() < 1e-5);
        // Turned by 30 degrees: only the part of the move along the
        // picture's own top edge counts for its left side.
        let whole = turned((960.0, 540.0), (800.0, 400.0), 30.0);
        let (sin, cos) = 30f32.to_radians().sin_cos();
        let along = (80.0 * cos, 80.0 * sin);
        let square = (-50.0 * sin, 50.0 * cos);
        let by = (along.0 + square.0, along.1 + square.1);
        assert!((crop_step(&whole, Side::Left, by) - 0.1).abs() < 1e-5);
        // Its bottom side, dragged up the picture's own way by 40: a tenth.
        let up = (40.0 * sin, -40.0 * cos);
        assert!((crop_step(&whole, Side::Bottom, up) - 0.1).abs() < 1e-5);
    }

    #[test]
    fn a_cut_side_snaps_to_the_frames_edges_and_centre_lines() {
        // A picture wider than the frame, 2400 pixels from x = -240: its
        // left side cut by 0.095 is at -12, 12 short of the frame's edge.
        let whole = upright((960.0, 540.0), (2400.0, 1200.0));
        let part = Part::WHOLE;
        let (cut, guides) = snap_crop(&whole, part, Side::Left, 0.095, FRAME, 20.0);
        assert!((cut - 0.1).abs() < 1e-6, "{cut}");
        assert_eq!(guides, [Some(Guide::Vertical(0.0)), None]);
        // The right side, cut to 5 pixels right of the frame's centre line.
        let place = |cut: f32| -240.0 + (1.0 - cut) * 2400.0;
        let cut = 1.0 - (965.0 + 240.0) / 2400.0;
        assert!((place(cut) - 965.0).abs() < 1e-3);
        let (snapped, guides) = snap_crop(&whole, part, Side::Right, cut, FRAME, 20.0);
        assert!((place(snapped) - 960.0).abs() < 1e-2, "{}", place(snapped));
        assert_eq!(guides, [Some(Guide::Vertical(960.0)), None]);
        // The bottom side: the frame's bottom edge, already cut a little.
        let cut_part = Part {
            height: 0.95,
            ..Part::WHOLE
        };
        // Its bottom is at -60 + 0.95 * 1200 = 1080 with nothing more cut.
        let (snapped, guides) = snap_crop(&whole, cut_part, Side::Bottom, 0.01, FRAME, 20.0);
        assert!(snapped.abs() < 1e-6, "{snapped}");
        assert_eq!(guides, [None, Some(Guide::Horizontal(1080.0))]);
        // Out of reach: left alone.
        assert_eq!(
            snap_crop(&whole, part, Side::Left, 0.05, FRAME, 20.0),
            (0.05, [None; 2])
        );
    }

    #[test]
    fn a_cut_side_snaps_on_a_quarter_turn_but_not_on_a_slant() {
        // A quarter turn: the picture's left side runs across the frame at
        // y = 540 - 400 = 140 and moves down as it is cut.
        let whole = turned((960.0, 540.0), (800.0, 400.0), 90.0);
        // Cut by 0.5025 it is at 542, 2 pixels under the frame's centre line.
        let (cut, guides) = snap_crop(&whole, Part::WHOLE, Side::Left, 0.5025, FRAME, 20.0);
        assert!((cut - 0.5).abs() < 1e-5, "{cut}");
        assert_eq!(guides, [None, Some(Guide::Horizontal(540.0))]);
        // Turned by 10 degrees: no line is along the side.
        let slanted = turned((960.0, 540.0), (800.0, 400.0), 10.0);
        assert_eq!(
            snap_crop(&slanted, Part::WHOLE, Side::Left, 0.5025, FRAME, 20.0),
            (0.5025, [None; 2])
        );
    }
}
