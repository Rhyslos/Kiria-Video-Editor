//! The Slip tool's drag, shown as it goes: the clip's pictures, waveform
//! and keyframe marks move under it, the drag is held to the part of the
//! file there is (never refused afterwards), and how much of the file is
//! left before its first frame and after its last is written at its edges.

use super::{TimelineClip, TimelineData};
use crate::theme::Theme;
use egui::{Align2, FontId, Rect, pos2};

/// How far a clip's picture can slip, in seconds on the timeline: how much
/// of its file lies before its first frame (`before`: a drag to the right
/// shows it) and after its last (`after`: a drag to the left). The page
/// fills it in; `None` is a clip that can slip as far as it likes or not
/// at all (a still, an adjustment layer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlipRoom {
    pub before: f64,
    pub after: f64,
}

/// The room every clip that slips together has: the dragged clip and the
/// clips linked to it, the least of each.
pub(super) fn room_of(data: &TimelineData, moves: impl Fn(&TimelineClip) -> bool) -> Option<SlipRoom> {
    data.video
        .iter()
        .chain(data.audio)
        .flat_map(|t| t.clips.iter())
        .filter(|c| moves(c))
        .filter_map(|c| c.slip_room)
        .reduce(|a, b| SlipRoom {
            before: a.before.min(b.before),
            after: a.after.min(b.after),
        })
}

/// A drag of `delta` seconds (right is positive) held inside `room` and to
/// whole frames towards zero, so the frame the page rounds it to always
/// fits: what was seen when letting go is what happens, however fast the
/// hand moved.
pub(super) fn held(delta: f64, room: Option<SlipRoom>, fps: f64) -> f64 {
    let delta = match room {
        Some(room) => delta.clamp(-room.after.max(0.0), room.before.max(0.0)),
        None => delta,
    };
    let fps = fps.max(1.0);
    (delta.abs() * fps + 1e-6).floor() / fps * delta.signum()
}

/// `clip` as it looks slipped by `delta` (right is positive): the same
/// place on the timeline, an earlier or later part of its file inside.
pub(super) fn shifted(clip: &TimelineClip, delta: f64) -> TimelineClip {
    let mut c = clip.clone();
    if let Some(p) = &mut c.pictures {
        p.source_in -= delta * p.speed;
    }
    if let Some(w) = &mut c.waveform {
        w.source_in -= delta * w.speed;
    }
    let end = clip.start + clip.duration;
    c.keyframes = clip
        .keyframes
        .iter()
        .map(|m| super::KeyframeMark {
            at: m.at + delta,
            ..m.clone()
        })
        .filter(|m| m.at >= clip.start && m.at < end)
        .collect();
    c
}

/// Writes how much file is left before the first frame (at the left edge)
/// and after the last (at the right) on a clip drawn in `r` while it is
/// slipped `delta` seconds; a side with none left is written in the
/// warning colour.
pub(super) fn draw_room(p: &egui::Painter, theme: &Theme, r: Rect, room: SlipRoom, delta: f64) {
    let (before, after) = ((room.before - delta).max(0.0), (room.after + delta).max(0.0));
    let colour = |left: f64| {
        if left < 1e-3 {
            crate::theme::status::WARNING
        } else {
            theme.text
        }
    };
    let y = r.bottom() - 9.0;
    let font = FontId::proportional(11.0);
    p.text(
        pos2(r.left() + 4.0, y),
        Align2::LEFT_CENTER,
        format!("◀ {:.2} s", before),
        font.clone(),
        colour(before),
    );
    p.text(
        pos2(r.right() - 4.0, y),
        Align2::RIGHT_CENTER,
        format!("{:.2} s ▶", after),
        font,
        colour(after),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slip_is_held_inside_the_file_in_whole_frames() {
        let room = Some(SlipRoom {
            before: 1.5,
            after: 0.25,
        });
        // A fast drag far past the room stops at its edge.
        assert_eq!(held(4.0, room, 30.0), 1.5);
        assert!(
            (held(-4.0, room, 30.0) + 0.233_333).abs() < 1e-3,
            "{}",
            held(-4.0, room, 30.0)
        );
        // Whole frames towards zero.
        assert!((held(0.51, room, 10.0) - 0.5).abs() < 1e-9);
        assert_eq!(held(3.0, None, 10.0), 3.0);
    }
}
