//! Gaps between clips, as things to select: a click on the empty stretch
//! between two clips of a track selects it (drawn lit), and the page can
//! close it (Delete or Backspace, or "Close the gap" in its right-click
//! menu), which moves everything after it back on every track.

use super::TimelineTrack;
use egui::{Rect, Stroke, pos2};

/// A selected gap: its track and where it lies, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    pub track: u64,
    pub start: f64,
    pub end: f64,
}

/// The gap of `track` at `t`: from the end of the clip before (or the
/// start of the timeline) to the start of the clip after. `None` where
/// there is a clip, or nothing after (the open end of the track).
pub fn gap_at(track: &TimelineTrack, t: f64) -> Option<Gap> {
    if track
        .clips
        .iter()
        .any(|c| c.start <= t && t < c.start + c.duration)
    {
        return None;
    }
    let start = track
        .clips
        .iter()
        .map(|c| c.start + c.duration)
        .filter(|end| *end <= t)
        .fold(0.0, f64::max);
    let end = track
        .clips
        .iter()
        .map(|c| c.start)
        .filter(|s| *s > t)
        .fold(f64::INFINITY, f64::min);
    (end.is_finite() && end - start > 1e-6).then_some(Gap {
        track: track.id,
        start,
        end,
    })
}

/// Draws a selected gap across `row` between `x0` and `x1` on screen.
pub fn draw(p: &egui::Painter, row: Rect, x0: f32, x1: f32, colour: egui::Color32) {
    let r = Rect::from_min_max(
        pos2(x0, row.top() + 2.0),
        pos2(x1.max(x0 + 1.0), row.bottom() - 2.0),
    );
    p.rect_filled(r, 3.0, colour.gamma_multiply(0.18));
    p.rect_stroke(r, 3.0, Stroke::new(1.0, colour), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(clips: &[(f64, f64)]) -> TimelineTrack {
        TimelineTrack {
            id: 4,
            name: "V1".into(),
            clips: clips
                .iter()
                .enumerate()
                .map(|(i, (start, duration))| super::super::TimelineClip {
                    id: i as u64,
                    start: *start,
                    duration: *duration,
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
    fn a_gap_runs_from_the_clip_before_to_the_clip_after() {
        let t = track(&[(2.0, 3.0), (9.0, 1.0)]);
        assert_eq!(
            gap_at(&t, 6.0),
            Some(Gap {
                track: 4,
                start: 5.0,
                end: 9.0
            })
        );
        assert_eq!(
            gap_at(&t, 1.0).map(|g| (g.start, g.end)),
            Some((0.0, 2.0)),
            "from the start"
        );
        assert_eq!(gap_at(&t, 3.0), None, "on a clip");
        assert_eq!(gap_at(&t, 12.0), None, "after the last clip");
    }
}
