//! Keyframe marks: small diamonds along the bottom edge of a clip, one at
//! each moment where its look has a keyframe (the page fills them in for
//! the selected clip). A click on one moves the playhead there.

use crate::keyframe;
use egui::{Color32, Painter, Pos2, Rect, pos2};

/// How far a mark's centre is above the clip's bottom edge, and its size.
const ABOVE_BOTTOM: f32 = 9.0;
const RADIUS: f32 = 5.0;
/// How near the pointer must be to a mark's centre to take it.
const REACH: (f32, f32) = (7.0, 8.0);

/// Where the mark for `at` (seconds on the timeline) goes on a clip drawn
/// in `clip`, `x_of` placing times across the timeline.
fn centre(clip: Rect, at: f64, x_of: &impl Fn(f64) -> f32) -> Pos2 {
    pos2(x_of(at), clip.bottom() - ABOVE_BOTTOM)
}

/// Draws a mark for each of `times` that falls inside the clip.
pub(super) fn draw(painter: &Painter, clip: Rect, times: &[f64], x_of: impl Fn(f64) -> f32, colour: Color32) {
    if clip.height() < 2.0 * ABOVE_BOTTOM {
        return;
    }
    for at in times {
        let c = centre(clip, *at, &x_of);
        if clip.x_range().contains(c.x) {
            keyframe::paint_mark(painter, c, RADIUS, colour);
        }
    }
}

/// The time of the mark under `pointer`, the nearest when two are close.
pub(super) fn hit(clip: Rect, times: &[f64], x_of: impl Fn(f64) -> f32, pointer: Pos2) -> Option<f64> {
    if clip.height() < 2.0 * ABOVE_BOTTOM {
        return None;
    }
    times
        .iter()
        .map(|at| (*at, centre(clip, *at, &x_of)))
        .filter(|(_, c)| clip.x_range().contains(c.x))
        .filter(|(_, c)| (c.x - pointer.x).abs() <= REACH.0 && (c.y - pointer.y).abs() <= REACH.1)
        .min_by(|a, b| (a.1.x - pointer.x).abs().total_cmp(&(b.1.x - pointer.x).abs()))
        .map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_taken_near_its_diamond_only() {
        let clip = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 100.0));
        let x_of = |t: f64| 100.0 + t as f32 * 10.0;
        let times = [1.0, 2.0, 20.0, 40.0];
        assert_eq!(hit(clip, &times, x_of, pos2(111.0, 94.0)), Some(1.0));
        assert_eq!(
            hit(clip, &times, x_of, pos2(119.0, 94.0)),
            Some(2.0),
            "the nearer one"
        );
        assert_eq!(hit(clip, &times, x_of, pos2(110.0, 70.0)), None, "too high");
        assert_eq!(hit(clip, &times, x_of, pos2(500.0, 94.0)), None, "past the clip");
        let thin = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 58.0));
        assert_eq!(
            hit(thin, &times, x_of, pos2(110.0, 52.0)),
            None,
            "no room on a thin clip"
        );
    }
}
