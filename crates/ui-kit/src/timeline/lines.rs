//! Volume and pan lines drawn on audio tracks and clips, with points to
//! add, drag and remove.
//!
//! Which line is shown and edited is the page's choice
//! ([`LinesShown`]: volume or pan, on the track or on its clips). The page
//! hands each track's line ([`super::TimelineTrack::line`]) and each
//! clip's ([`super::TimelineClip::line`]) as points of (seconds on the
//! timeline, value): decibels added to the volume, or a pan from -1 (all
//! left) to 1 (all right). A track's line runs across its whole row, a
//! clip's inside the clip. With no points a line is flat at nothing added
//! (0 dB) or the centre.
//!
//! While lines are shown, the pointer over the audio tracks works the line
//! of the row under it: a click adds a point there, a point can be
//! dragged (in whole frames, never past its neighbours, a clip's points
//! never off the clip), and a double-click or its right-click menu removes
//! it. Letting go sends one [`super::TimelineEvent::Line`] with every
//! point of that line, so each change is one undo step; while dragging,
//! [`super::TimelineView::line_being_set`] says where the points are now,
//! so the page can be heard following them.
//!
//! The volume line sits on the same scale as the level line ([`super::level`]:
//! 0 dB two thirds of the way up, +12 dB at the top, a fader's law below
//! with silence at the very bottom); a point dragged with Shift or Alt
//! held moves a fifth as fast, for fine changes. The pan
//! line has the centre in the middle, all left at the top and all right
//! at the bottom.

use super::level;
use egui::{Color32, FontId, Pos2, Rect, Stroke, pos2, vec2};
use std::sync::Arc;

/// Points of (seconds on the timeline, value), in time order.
pub type LinePoints = Arc<[(f64, f32)]>;

/// Which line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineKind {
    /// Decibels added to the volume (-60 to +12).
    #[default]
    Volume,
    /// The pan: -1 all left, 0 the centre, 1 all right.
    Pan,
}

impl LineKind {
    /// Its value with no points.
    pub fn flat(self) -> f32 {
        0.0
    }

    /// The lowest and highest values.
    pub fn range(self) -> (f32, f32) {
        match self {
            LineKind::Volume => (level::SILENT_DB, level::LOUDEST_DB),
            LineKind::Pan => (-1.0, 1.0),
        }
    }

    /// A value as the interface writes it: "−6.0 dB", "Off", "L 40", "C", "R 20".
    pub fn words(self, value: f32) -> String {
        match self {
            LineKind::Volume => level::words(f64::from(value)),
            LineKind::Pan => pan_words(value),
        }
    }

    /// The colour it is drawn in.
    fn colour(self) -> Color32 {
        match self {
            LineKind::Volume => Color32::from_rgb(0xF2, 0xC9, 0x4C),
            LineKind::Pan => Color32::from_rgb(0x6C, 0xC4, 0xF0),
        }
    }
}

/// A pan as the interface writes it: "L 40", "C", "R 20" (hundredths of
/// the way to a side).
pub fn pan_words(pan: f32) -> String {
    let hundredths = (pan * 100.0).round() as i32;
    match hundredths {
        0 => "C".to_owned(),
        h if h < 0 => format!("L {}", -h),
        h => format!("R {h}"),
    }
}

/// Whose line it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOwner {
    Track(u64),
    Clip(u64),
}

/// Which lines the timeline shows and edits: the page's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinesShown {
    pub kind: LineKind,
    /// On the clips (each clip's own line) rather than across the track.
    pub on_clips: bool,
}

/// How near a point the pointer must be to take it, in points.
pub const GRAB: f32 = 6.0;

/// Where in `rect` (a row or a clip) a line moves: as the level line.
pub fn travel(rect: Rect) -> Rect {
    level::travel(rect)
}

/// The y of `value` in `rect`.
pub fn y_of(kind: LineKind, rect: Rect, value: f32) -> f32 {
    let t = travel(rect);
    let height = match kind {
        LineKind::Volume => level::height_of(value),
        LineKind::Pan => 0.5 - value.clamp(-1.0, 1.0) / 2.0,
    };
    t.bottom() - height * t.height()
}

/// The value at `y` in `rect`, settled as a drag stops on it: tenths of a
/// decibel (0 dB within half a decibel, the bottom silent), or hundredths
/// of a pan (the centre within 0.03).
pub fn value_at(kind: LineKind, rect: Rect, y: f32) -> f32 {
    let t = travel(rect);
    value_at_height(kind, ((t.bottom() - y) / t.height().max(1.0)).clamp(0.0, 1.0))
}

/// Where `value` sits in a row or clip, from its bottom (0) to its top (1).
pub fn height_of(kind: LineKind, value: f32) -> f32 {
    match kind {
        LineKind::Volume => level::height_of(value),
        LineKind::Pan => 0.5 - value.clamp(-1.0, 1.0) / 2.0,
    }
}

/// The value at `height` (0 the bottom, 1 the top), settled as a drag
/// stops on it.
pub fn value_at_height(kind: LineKind, height: f32) -> f32 {
    match kind {
        LineKind::Volume => level::settled_db(height),
        LineKind::Pan => {
            let pan = (0.5 - height) * 2.0;
            if pan.abs() < 0.03 {
                0.0
            } else {
                (pan * 100.0).round() / 100.0
            }
        }
    }
}

/// The line's value at `t`: straight between points, flat outside them,
/// `None` without points.
pub fn value_on(points: &[(f64, f32)], t: f64) -> Option<f32> {
    let (first, last) = (points.first()?, points.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    let next = points.partition_point(|p| p.0 <= t);
    let (a, b) = (points.get(next.checked_sub(1)?)?, points.get(next)?);
    let share = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
    Some(a.1 + (b.1 - a.1) * share as f32)
}

/// The point under `pos`, if any (the nearest of close ones).
pub fn hit(points: &[(f64, f32)], place: impl Fn(f64, f32) -> Pos2, pos: Pos2) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, &(t, v))| (i, place(t, v).distance(pos)))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Two points closer than this (seconds) are at one moment.
const SAME_MOMENT: f64 = 1e-6;

/// The points with one added at `t`: in its place in time, taking the
/// place of one already at that moment. Returns them and the new point's index.
pub fn added(points: &[(f64, f32)], t: f64, value: f32) -> (Vec<(f64, f32)>, usize) {
    let mut out: Vec<(f64, f32)> = points
        .iter()
        .copied()
        .filter(|p| (p.0 - t).abs() > SAME_MOMENT)
        .collect();
    let at = out.partition_point(|p| p.0 < t);
    out.insert(at, (t, value));
    (out, at)
}

/// The points with the `i`-th moved to `t` and `value`: never onto or past
/// its neighbours (it stops a frame short of them, `frame` seconds), and
/// within `bounds` (a clip's span, or the timeline).
pub fn moved(
    points: &[(f64, f32)],
    i: usize,
    (t, value): (f64, f32),
    frame: f64,
    bounds: (f64, f64),
) -> Vec<(f64, f32)> {
    let mut out = points.to_vec();
    let low = match i.checked_sub(1).and_then(|j| points.get(j)) {
        Some(p) => p.0 + frame,
        None => bounds.0,
    };
    let high = match points.get(i + 1) {
        Some(p) => p.0 - frame,
        None => bounds.1,
    };
    if let Some(p) = out.get_mut(i) {
        let t = if low <= high { t.clamp(low, high) } else { p.0 };
        *p = (t, value);
    }
    out
}

/// The points without the `i`-th.
pub fn removed(points: &[(f64, f32)], i: usize) -> Vec<(f64, f32)> {
    points
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .map(|(_, p)| *p)
        .collect()
}

/// A line being set by a drag, as it is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineBeingSet<'a> {
    pub owner: LineOwner,
    pub kind: LineKind,
    /// Every point of it, in seconds on the timeline.
    pub points: &'a [(f64, f32)],
}

/// A line's point being dragged.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LineDrag {
    pub owner: LineOwner,
    pub kind: LineKind,
    /// Which point.
    pub index: usize,
    /// Every point of the line as it is now.
    pub points: Vec<(f64, f32)>,
    /// Where the point may go: the clip's span, or the timeline.
    pub bounds: (f64, f64),
    /// The row or clip the line is drawn in when the drag began.
    pub rect: Rect,
    /// Where the point is (0 bottom, 1 top), unrounded, so slow and fine
    /// moves add up.
    pub height: f32,
    /// The pointer's y on the last frame.
    pub last_y: f32,
}

impl LineDrag {
    /// The pointer moved to `y`; `fine` (Shift or Alt held) moves the point
    /// a fifth as far as the pointer. Returns the value it is at now.
    /// Without it the point is where the pointer is.
    pub fn follow(&mut self, y: f32, fine: bool) -> f32 {
        let t = travel(self.rect);
        self.height = if fine {
            self.height + (self.last_y - y) / t.height().max(1.0) * level::FINE
        } else {
            (t.bottom() - y) / t.height().max(1.0)
        }
        .clamp(0.0, 1.0);
        self.last_y = y;
        value_at_height(self.kind, self.height)
    }
}

/// How a line is drawn.
pub(super) struct Look {
    pub kind: LineKind,
    /// Its points are shown and can be taken.
    pub editable: bool,
    /// The point pointed at or dragged.
    pub lit: Option<usize>,
    /// While dragging: the tag with the value goes by this x.
    pub tag_x: Option<f32>,
}

/// Draws a line with `points` in `rect` (a row or a clip), from `x0` to
/// `x1` on screen, with `x_of` turning seconds into x.
pub(super) fn draw(
    p: &egui::Painter,
    rect: Rect,
    points: &[(f64, f32)],
    x_of: impl Fn(f64) -> f32,
    look: &Look,
) {
    let kind = look.kind;
    let colour = if look.editable {
        kind.colour()
    } else {
        kind.colour().gamma_multiply(0.55)
    };
    let y = |v: f32| y_of(kind, rect, v);
    let (left, right) = (rect.left(), rect.right());
    let mut line: Vec<Pos2> = Vec::with_capacity(points.len() + 2);
    match (points.first(), points.last()) {
        (Some(first), Some(last)) => {
            line.push(pos2(left, y(first.1)));
            for &(t, v) in points {
                let x = x_of(t);
                // Points out of sight are left out; the ends below
                // carry the line to the edges at its value there.
                if x > left && x < right {
                    line.push(pos2(x, y(v)));
                }
            }
            line.push(pos2(right, y(last.1)));
            // Where the line meets the edges of what is shown.
            fix_edges(&mut line, points, &x_of, &y, left, right);
        }
        _ => {
            let flat = y(kind.flat());
            line.push(pos2(left, flat));
            line.push(pos2(right, flat));
        }
    }
    let shadow: Vec<Pos2> = line.iter().map(|q| *q + vec2(0.0, 1.0)).collect();
    p.add(egui::Shape::line(
        shadow,
        Stroke::new(2.0, Color32::from_black_alpha(110)),
    ));
    p.add(egui::Shape::line(
        line,
        Stroke::new(if look.editable { 2.0 } else { 1.5 }, colour),
    ));
    if !look.editable {
        return;
    }
    for (i, &(t, v)) in points.iter().enumerate() {
        let x = x_of(t);
        if x < left - GRAB || x > right + GRAB {
            continue;
        }
        let c = pos2(x, y(v));
        let lit = look.lit == Some(i);
        p.circle_filled(c, if lit { 5.0 } else { 3.5 }, colour);
        p.circle_stroke(
            c,
            if lit { 5.0 } else { 3.5 },
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        );
        if lit && let Some(tag_x) = look.tag_x {
            tag(p, rect, kind, v, pos2(tag_x, c.y), colour);
        }
    }
}

/// The ends of `line` at the edges of what is shown, at the line's value there.
fn fix_edges(
    line: &mut [Pos2],
    points: &[(f64, f32)],
    x_of: &impl Fn(f64) -> f32,
    y: &impl Fn(f32) -> f32,
    left: f32,
    right: f32,
) {
    // Seconds at the edges, from two points' places (x is straight in time).
    let (Some(a), Some(b)) = (points.first(), points.last()) else {
        return;
    };
    let (xa, xb) = (x_of(a.0), x_of(b.0));
    let time_at = |x: f32| {
        if (xb - xa).abs() < 1e-3 {
            a.0
        } else {
            a.0 + (b.0 - a.0) * f64::from((x - xa) / (xb - xa))
        }
    };
    let n = line.len();
    if let Some(v) = value_on(points, time_at(left))
        && let Some(first) = line.first_mut()
    {
        *first = pos2(left, y(v));
    }
    if n > 0
        && let Some(v) = value_on(points, time_at(right))
        && let Some(last) = line.last_mut()
    {
        *last = pos2(right, y(v));
    }
}

/// The value on a small tag beside a point being dragged.
fn tag(p: &egui::Painter, rect: Rect, kind: LineKind, value: f32, at: Pos2, colour: Color32) {
    let galley = p.layout_no_wrap(
        kind.words(value),
        FontId::proportional(11.0),
        Color32::from_gray(20),
    );
    let size = galley.size() + vec2(10.0, 4.0);
    let top = if at.y - size.y - 6.0 >= rect.top() {
        at.y - size.y - 6.0
    } else {
        at.y + 6.0
    };
    let left = (at.x + 8.0).min(rect.right() - size.x).max(rect.left());
    let r = Rect::from_min_size(pos2(left, top), size);
    p.rect_filled(r, 3.0, colour);
    p.galley(r.center() - galley.size() / 2.0, galley, Color32::from_gray(20));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_places_round_trip_on_both_scales() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 80.0));
        for (kind, v) in [
            (LineKind::Volume, -6.0),
            (LineKind::Volume, 3.0),
            (LineKind::Volume, 0.0),
            (LineKind::Pan, -0.4),
            (LineKind::Pan, 0.25),
        ] {
            let back = value_at(kind, rect, y_of(kind, rect, v));
            assert!((back - v).abs() < 0.11, "{kind:?} {v} came back {back}");
        }
        assert!(
            y_of(LineKind::Pan, rect, -1.0) < y_of(LineKind::Pan, rect, 1.0),
            "left on top"
        );
        assert_eq!(
            value_at(LineKind::Pan, rect, 40.0 + 0.5),
            0.0,
            "the centre is easy to find"
        );
        assert_eq!(value_at(LineKind::Volume, rect, 200.0), level::SILENT_DB);
        assert_eq!(pan_words(-0.4), "L 40");
        assert_eq!(pan_words(0.0), "C");
        assert_eq!(pan_words(0.2), "R 20");
    }

    #[test]
    fn a_line_is_straight_between_points_and_flat_outside() {
        let points = [(1.0, -6.0), (3.0, 0.0)];
        assert_eq!(value_on(&points, 0.0), Some(-6.0));
        assert_eq!(value_on(&points, 2.0), Some(-3.0));
        assert_eq!(value_on(&points, 9.0), Some(0.0));
        assert_eq!(value_on(&[], 1.0), None);
    }

    #[test]
    fn points_are_added_in_order_moved_between_neighbours_and_removed() {
        let points = [(1.0, -6.0), (3.0, 0.0)];
        let (more, at) = added(&points, 2.0, -3.0);
        assert_eq!(
            (more.as_slice(), at),
            (&[(1.0, -6.0), (2.0, -3.0), (3.0, 0.0)][..], 1)
        );
        let (same, _) = added(&points, 3.0, -1.0);
        assert_eq!(same, [(1.0, -6.0), (3.0, -1.0)], "one point at a moment");
        let frame = 0.1;
        let moved_far = moved(&more, 1, (9.0, 2.0), frame, (0.0, 10.0));
        assert_eq!(moved_far[1], (2.9, 2.0), "a frame short of the next");
        let first_out = moved(&more, 0, (-5.0, 0.0), frame, (0.5, 10.0));
        assert_eq!(first_out[0].0, 0.5, "within the clip");
        assert_eq!(removed(&more, 1), points);
        let place = |t: f64, v: f32| pos2(t as f32 * 10.0, -v);
        assert_eq!(hit(&more, place, pos2(21.0, 4.0)), Some(1));
        assert_eq!(hit(&more, place, pos2(15.0, -5.0)), None);
    }
}
