//! Transitions on the timeline: each drawn as a small box across its cut
//! with a line from corner to corner, and the cut a dragged transition
//! would go onto, found from where the pointer is.

use super::TimelineTrack;
use egui::{Color32, Painter, Rect, Stroke, pos2};

/// Two clips this close (in seconds) meet at a cut.
const MEETS: f64 = 1e-6;

/// The cut of `track` a transition let go at `t` goes onto: the one at the
/// start of the clip under `t` when `t` is in its first half, else the one
/// at its end. The clip coming in across it and the cut's time; `None`
/// when no clip meets another there.
pub fn cut_near(track: &TimelineTrack, t: f64) -> Option<(u64, f64)> {
    let under = track
        .clips
        .iter()
        .find(|c| c.start <= t && t < c.start + c.duration)?;
    let end = under.start + under.duration;
    let starts_at = |at: f64| track.clips.iter().find(|c| (c.start - at).abs() < MEETS);
    let ends_at = |at: f64| {
        track
            .clips
            .iter()
            .any(|c| c.id != under.id && (c.start + c.duration - at).abs() < MEETS)
    };
    let into_this = (under.id, under.start);
    let into_next = starts_at(end).map(|next| (next.id, end));
    let first_half = t < under.start + under.duration / 2.0;
    let this = ends_at(under.start).then_some(into_this);
    if first_half {
        this.or(into_next)
    } else {
        into_next.or(this)
    }
}

/// Draws a transition across a cut, from `x0` to `x1` on screen, in `row`.
pub fn draw(p: &Painter, row: Rect, x0: f32, x1: f32, colour: Color32, fill: Color32) {
    let (left, right) = box_x(x0, x1);
    let r = Rect::from_min_max(pos2(left, row.top() + 3.0), pos2(right, row.bottom() - 3.0));
    p.rect_filled(r, 3.0, fill);
    p.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.0, colour));
    p.rect_stroke(r, 3.0, Stroke::new(1.0, colour), egui::StrokeKind::Inside);
}

/// Whether a transition from `x0` to `x1` on screen shows inside `seen`
/// at all: those out of view are not drawn.
pub fn in_view(x0: f32, x1: f32, seen: Rect) -> bool {
    let (left, right) = box_x(x0, x1);
    right >= seen.left() && left <= seen.right()
}

/// The left and right edge of a transition's box: from `x0` to `x1`, at
/// least 8 points wide around their middle.
fn box_x(x0: f32, x1: f32) -> (f32, f32) {
    let width = (x1 - x0).max(8.0);
    let middle = (x0 + x1) / 2.0;
    (middle - width / 2.0, middle + width / 2.0)
}

/// Where a dragged transition would go: a lit band across the cut.
pub fn draw_landing(p: &Painter, row: Rect, x: f32, colour: Color32) {
    let r = Rect::from_center_size(pos2(x, row.center().y), egui::vec2(14.0, row.height() - 4.0));
    p.rect_filled(r, 3.0, colour.gamma_multiply(0.3));
    p.rect_stroke(r, 3.0, Stroke::new(2.0, colour), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TimelineClip;

    fn track(clips: &[(u64, f64, f64)]) -> TimelineTrack {
        TimelineTrack {
            id: 1,
            name: "V1".into(),
            clips: clips
                .iter()
                .map(|&(id, start, duration)| TimelineClip {
                    id,
                    start,
                    duration,
                    name: String::new(),
                    info: String::new(),
                    selected: false,
                    missing: false,
                    link: None,
                    waveform: None,
                    pictures: None,
                    loading: None,
                    sound: Default::default(),
                    keyframes: Vec::new(),
                    speed_curve: None,
                    shows: Default::default(),
                    line: None,
                    slip_room: None,
                    plays_until: None,
                    transition_in: None,
                    graded: false,
                })
                .collect(),
            muted: false,
            solo: false,
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
        }
    }

    #[test]
    fn a_transition_goes_onto_the_nearer_cut_of_the_clip_under_it() {
        // 0-4, 4-8, then a gap, 10-12.
        let t = track(&[(1, 0.0, 4.0), (2, 4.0, 4.0), (3, 10.0, 2.0)]);
        assert_eq!(cut_near(&t, 3.0), Some((2, 4.0)), "near the first's end");
        assert_eq!(cut_near(&t, 1.0), Some((2, 4.0)), "its start has no cut");
        assert_eq!(cut_near(&t, 5.0), Some((2, 4.0)));
        assert_eq!(cut_near(&t, 7.0), Some((2, 4.0)), "its end meets a gap");
        assert_eq!(cut_near(&t, 11.0), None, "alone");
        assert_eq!(cut_near(&t, 9.0), None, "in the gap");
    }
}
