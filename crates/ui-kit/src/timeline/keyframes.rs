//! Keyframe marks: small diamonds along the bottom edge of a clip, one at
//! each frame where its look has keyframes (the page fills them in for the
//! selected clip). A mark stands for every keyframe of its frame, whatever
//! numbers they are for.
//!
//! A click on a mark moves the playhead there; Shift-click (or Ctrl-click)
//! adds it to the marks chosen, or takes it out. A drag that starts in the
//! strip the marks lie along (the clip's bottom edge), away from a mark,
//! draws a box: the marks inside it are chosen (with Shift or Ctrl, added
//! to those chosen already). Higher up the clip a drag still moves the
//! clip, so the strip is only taken on a clip with marks, the selected
//! one. Chosen marks are what Copy copies. Dragging a mark moves it
//! (with the other chosen marks) a whole frame at a time, snapping to the
//! playhead and the clip's other marks unless snapping is off or Alt is
//! held, and never off the clip. Dropped where a number would get a second
//! keyframe in one frame, it is shown refused. The right-click menu goes
//! there, removes the keyframes or sets how they go on to the next.
//!
//! The pure parts (where a drag lands, whether it clashes, where a mark is
//! drawn) are here; the timeline turns the pointer into them.

use crate::keyframe;
use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2};

/// How far a mark's centre is above the clip's bottom edge, and its size.
const ABOVE_BOTTOM: f32 = 9.0;
const RADIUS: f32 = 5.0;
/// How near the pointer must be to a mark's centre to take it.
const REACH: (f32, f32) = (7.0, 8.0);
/// How near (in points) a dragged mark snaps to the playhead or a mark.
pub(super) const SNAP_REACH: f32 = 8.0;

/// One mark: a frame of the clip with keyframes.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyframeMark {
    /// The start of its frame, in seconds on the timeline.
    pub at: f64,
    /// The numbers keyed there, in plain words ("position", "opacity").
    /// Two marks with a word in common can't be dropped on one frame.
    pub keyed: Vec<&'static str>,
    /// How its keyframes go on to the next (an index of the eases the
    /// timeline is given), when they all go the same way.
    pub ease: Option<usize>,
}

impl KeyframeMark {
    /// A mark at `at` with nothing said about it (for tests and pages that
    /// only show where keyframes are).
    pub fn at(at: f64) -> KeyframeMark {
        KeyframeMark {
            at,
            keyed: Vec::new(),
            ease: None,
        }
    }

    /// What its tooltip says.
    pub fn says(&self) -> String {
        let keyed = match self.keyed.as_slice() {
            [] => "Keyframes".to_owned(),
            keyed => format!("Keyframes: {}", keyed.join(", ")),
        };
        format!(
            "{keyed}\nClick: go here. Drag: move (Alt: no snapping). Shift-click: choose several, or drag a box along the clip's bottom edge. Right-click: more"
        )
    }

    fn clashes_with(&self, other: &KeyframeMark) -> bool {
        self.keyed.iter().any(|word| other.keyed.contains(word))
    }
}

/// Whether two times are the same frame's start, at `fps`.
pub(super) fn same(a: f64, b: f64, fps: f64) -> bool {
    (a - b).abs() < 0.25 / fps.max(1.0)
}

/// Marks chosen to act on together, all on one clip.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Chosen {
    pub clip: u64,
    pub at: Vec<f64>,
}

impl Chosen {
    /// The chosen marks of `clip` that are still among `marks`.
    pub fn of(&self, clip: u64, marks: &[KeyframeMark], fps: f64) -> Vec<f64> {
        if self.clip != clip {
            return Vec::new();
        }
        marks
            .iter()
            .map(|m| m.at)
            .filter(|at| self.at.iter().any(|c| same(*c, *at, fps)))
            .collect()
    }

    /// A click on the mark at `at` of `clip`: alone, or (`add`) added to
    /// the chosen marks or taken out of them.
    pub fn click(&mut self, clip: u64, at: f64, add: bool, fps: f64) {
        if self.clip != clip || !add {
            *self = Chosen { clip, at: vec![at] };
        } else if let Some(i) = self.at.iter().position(|c| same(*c, at, fps)) {
            self.at.remove(i);
        } else {
            self.at.push(at);
        }
    }
}

/// A box being dragged along a clip's marks to choose them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MarkBox {
    pub clip: u64,
    /// Where it was pressed and where the pointer is now, in seconds on
    /// the timeline.
    pub from: f64,
    pub to: f64,
    /// The marks chosen before it began, which it adds to (Shift or Ctrl
    /// held when it began); empty when it chooses afresh.
    pub kept: Vec<f64>,
}

impl MarkBox {
    /// The marks chosen with the box where it is now: those it holds, and
    /// those kept.
    pub fn chosen(&self, marks: &[KeyframeMark], fps: f64) -> Chosen {
        let (low, high) = (self.from.min(self.to), self.from.max(self.to));
        let slack = 0.25 / fps.max(1.0);
        let mut at: Vec<f64> = self.kept.clone();
        for mark in marks {
            let inside = low - slack <= mark.at && mark.at <= high + slack;
            if inside && !at.iter().any(|c| same(*c, mark.at, fps)) {
                at.push(mark.at);
            }
        }
        at.sort_by(f64::total_cmp);
        Chosen { clip: self.clip, at }
    }
}

/// Whether `pointer` is in the strip along the bottom of a clip drawn in
/// `clip` where its marks lie, where a drag draws a box to choose them.
pub(super) fn in_strip(clip: Rect, pointer: Pos2) -> bool {
    clip.height() >= 2.0 * ABOVE_BOTTOM && strip(clip).contains(pointer)
}

/// The strip along the bottom of a clip drawn in `clip` that its marks lie in.
fn strip(clip: Rect) -> Rect {
    let centre = clip.bottom() - ABOVE_BOTTOM;
    Rect::from_x_y_ranges(clip.x_range(), (centre - REACH.1)..=clip.bottom())
}

/// Draws the box of `chosen_box` on a clip drawn in `clip`: across the
/// strip of marks, from where it was pressed to the pointer.
pub(super) fn draw_box(
    painter: &Painter,
    clip: Rect,
    chosen_box: &MarkBox,
    x_of: impl Fn(f64) -> f32,
    colour: Color32,
) {
    let band = strip(clip);
    let (a, b) = (x_of(chosen_box.from), x_of(chosen_box.to));
    let shape = Rect::from_x_y_ranges(a.min(b)..=a.max(b).max(a.min(b) + 1.0), band.y_range());
    // Dark under the light edge, so it reads over the clip's pictures.
    painter.rect_filled(shape, 2.0, Color32::from_black_alpha(110));
    painter.rect_filled(shape, 2.0, colour.gamma_multiply(0.15));
    painter.rect_stroke(shape, 2.0, Stroke::new(1.5, colour), egui::StrokeKind::Inside);
}

/// A mark (with the other chosen marks) being dragged.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MarkDrag {
    pub clip: u64,
    /// The marks moving, the grabbed one first.
    pub moving: Vec<f64>,
    /// The pointer's time when it was pressed.
    pub from: f64,
    /// How far they have moved, in seconds (whole frames).
    pub by: f64,
    /// Let go now, the move would be refused: a number would get two
    /// keyframes in one frame.
    pub refused: bool,
}

/// What a dragged mark can land on, and where it must stay.
#[derive(Debug, Clone, Copy)]
pub(super) struct Room<'a> {
    pub fps: f64,
    /// The starts of the clip's first and last frames.
    pub first: f64,
    pub last: f64,
    /// Times the grabbed mark snaps to (none with snapping off), and how
    /// near it must come to one, in seconds.
    pub targets: &'a [f64],
    pub reach: f64,
}

/// How far marks `moving` (the grabbed one first) move when the pointer
/// has gone `raw` seconds: onto a snap target within reach, then to whole
/// frames, and no further than keeps every one of them on the clip.
pub(super) fn dragged_by(raw: f64, moving: &[f64], room: Room) -> f64 {
    let Some(grabbed) = moving.first() else {
        return 0.0;
    };
    let fps = room.fps.max(1.0);
    let by = room
        .targets
        .iter()
        .map(|t| t - (grabbed + raw))
        .filter(|d| d.abs() <= room.reach)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        .map_or(raw, |d| raw + d);
    let frames = (by * fps).round();
    let lowest = moving.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = moving.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let least = ((room.first - lowest) * fps).round();
    let most = ((room.last - highest) * fps).round();
    // Never past either end (a mark already beyond one doesn't move further).
    frames.clamp(least.min(0.0), most.max(0.0)) / fps
}

/// The starts of the first and last frame of a clip from `start` to `end`.
pub(super) fn frames_of(start: f64, end: f64, fps: f64) -> (f64, f64) {
    let fps = fps.max(1.0);
    let first = (start * fps).round();
    let last = ((end * fps) - 1e-6).ceil() - 1.0;
    (first / fps, last.max(first) / fps)
}

/// Whether moving marks `moving` by `by` puts a keyframe of some number on
/// a frame where another mark has one of that number.
pub(super) fn clashes(marks: &[KeyframeMark], moving: &[f64], by: f64, fps: f64) -> bool {
    let moves = |m: &KeyframeMark| moving.iter().any(|at| same(*at, m.at, fps));
    marks.iter().filter(|m| moves(m)).any(|m| {
        marks
            .iter()
            .filter(|o| !moves(o) && same(o.at, m.at + by, fps))
            .any(|o| m.clashes_with(o))
    })
}

/// Where the mark for `at` (seconds on the timeline) goes on a clip drawn
/// in `clip`, `x_of` placing times across the timeline. A mark is drawn
/// whole inside its clip: one on its first or last frame is moved in from
/// the edge.
fn centre(clip: Rect, at: f64, x_of: &impl Fn(f64) -> f32) -> Pos2 {
    let x = x_of(at);
    let inset = RADIUS + 1.5;
    let x = if clip.width() > 2.0 * inset && clip.x_range().contains(x) {
        x.clamp(clip.left() + inset, clip.right() - inset)
    } else {
        x
    };
    pos2(x, clip.bottom() - ABOVE_BOTTOM)
}

/// How the marks of one clip are drawn.
pub(super) struct MarksLook<'a> {
    pub colour: Color32,
    /// Marks chosen (drawn with a light rim).
    pub chosen: &'a [f64],
    /// A drag going on: the marks moving and how far.
    pub drag: Option<&'a MarkDrag>,
    pub fps: f64,
}

/// Draws a mark for each of `marks` that falls inside the clip.
pub(super) fn draw(
    painter: &Painter,
    clip: Rect,
    marks: &[KeyframeMark],
    x_of: impl Fn(f64) -> f32,
    look: MarksLook,
) {
    if clip.height() < 2.0 * ABOVE_BOTTOM {
        return;
    }
    let is = |list: &[f64], at: f64| list.iter().any(|c| same(*c, at, look.fps));
    let moving = |at: f64| look.drag.is_some_and(|d| is(&d.moving, at));
    for mark in marks.iter().filter(|m| !moving(m.at)) {
        let c = centre(clip, mark.at, &x_of);
        if clip.x_range().contains(c.x) {
            keyframe::paint_mark(painter, c, RADIUS, look.colour);
            if is(look.chosen, mark.at) {
                chosen_rim(painter, c);
            }
        }
    }
    let Some(drag) = look.drag else { return };
    let colour = match drag.refused {
        true => crate::status::ERROR,
        false => look.colour,
    };
    for at in &drag.moving {
        // Where it was, hollow, and where it goes.
        let was = centre(clip, *at, &x_of);
        if clip.x_range().contains(was.x) {
            keyframe::paint(painter, was, RADIUS, keyframe::KeyState::Still, look.colour);
        }
        let c = centre(clip, at + drag.by, &x_of);
        if clip.x_range().contains(c.x) {
            keyframe::paint_mark(painter, c, RADIUS + 1.0, colour);
            chosen_rim(painter, c);
        }
    }
}

/// The light rim of a chosen (or moving) mark.
fn chosen_rim(painter: &Painter, c: Pos2) {
    let r = RADIUS + 2.0;
    painter.add(egui::Shape::closed_line(
        vec![
            pos2(c.x, c.y - r),
            pos2(c.x + r, c.y),
            pos2(c.x, c.y + r),
            pos2(c.x - r, c.y),
        ],
        Stroke::new(1.5, Color32::WHITE),
    ));
}

/// The mark under `pointer`, the nearest when two are close.
pub(super) fn hit(
    clip: Rect,
    marks: &[KeyframeMark],
    x_of: impl Fn(f64) -> f32,
    pointer: Pos2,
) -> Option<&KeyframeMark> {
    if clip.height() < 2.0 * ABOVE_BOTTOM {
        return None;
    }
    marks
        .iter()
        .map(|m| (m, centre(clip, m.at, &x_of)))
        .filter(|(_, c)| clip.x_range().contains(c.x))
        .filter(|(_, c)| (c.x - pointer.x).abs() <= REACH.0 && (c.y - pointer.y).abs() <= REACH.1)
        .min_by(|a, b| (a.1.x - pointer.x).abs().total_cmp(&(b.1.x - pointer.x).abs()))
        .map(|(m, _)| m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(times: &[f64]) -> Vec<KeyframeMark> {
        times.iter().map(|t| KeyframeMark::at(*t)).collect()
    }

    #[test]
    fn a_mark_is_taken_near_its_diamond_only() {
        let clip = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 100.0));
        let x_of = |t: f64| 100.0 + t as f32 * 10.0;
        let times = marks(&[1.0, 2.0, 20.0, 40.0]);
        let at = |p| hit(clip, &times, x_of, p).map(|m| m.at);
        assert_eq!(at(pos2(111.0, 94.0)), Some(1.0));
        assert_eq!(at(pos2(119.0, 94.0)), Some(2.0), "the nearer one");
        assert_eq!(at(pos2(110.0, 70.0)), None, "too high");
        assert_eq!(at(pos2(500.0, 94.0)), None, "past the clip");
        let thin = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 58.0));
        assert_eq!(
            hit(thin, &times, x_of, pos2(110.0, 52.0)).map(|m| m.at),
            None,
            "no room on a thin clip"
        );
    }

    #[test]
    fn a_mark_on_the_clips_first_frame_is_drawn_whole_inside_it() {
        let clip = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 100.0));
        let x_of = |t: f64| 100.0 + t as f32 * 10.0;
        let first = centre(clip, 0.0, &x_of);
        assert!(first.x - RADIUS > clip.left(), "{first:?}");
        let last = centre(clip, 30.0, &x_of);
        assert!(last.x + RADIUS < clip.right(), "{last:?}");
        assert_eq!(centre(clip, 10.0, &x_of).x, 200.0, "in the middle: at its frame");
        // And it is taken where it is drawn.
        let times = marks(&[0.0]);
        assert!(hit(clip, &times, x_of, first).is_some());
    }

    fn room(targets: &[f64]) -> Room<'_> {
        Room {
            fps: 10.0,
            first: 2.0,
            last: 9.9,
            targets,
            reach: 0.25,
        }
    }

    #[test]
    fn a_dragged_mark_goes_by_whole_frames_and_snaps_within_reach() {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(dragged_by(0.34, &[5.0], room(&[])), 0.3), "whole frames");
        assert!(near(dragged_by(0.36, &[5.0], room(&[])), 0.4));
        // The playhead at 6.5: within reach of 6.36, so it lands there.
        assert!(near(dragged_by(1.36, &[5.0], room(&[6.5])), 1.5));
        assert!(near(dragged_by(1.1, &[5.0], room(&[6.5])), 1.1), "out of reach");
        // The nearest of two targets wins.
        assert!(near(dragged_by(1.0, &[5.0], room(&[5.8, 6.1])), 1.1));
    }

    #[test]
    fn dragged_marks_stay_on_the_clip_together() {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            near(dragged_by(-10.0, &[5.0], room(&[])), -3.0),
            "to the first frame"
        );
        assert!(
            near(dragged_by(10.0, &[5.0], room(&[])), 4.9),
            "to the last frame"
        );
        // Two marks: the later one stops at the end, the earlier with it.
        assert!(near(dragged_by(10.0, &[3.0, 8.0], room(&[])), 1.9));
        assert!(near(dragged_by(-10.0, &[3.0, 8.0], room(&[])), -1.0));
        assert_eq!(dragged_by(1.0, &[], room(&[])), 0.0);
        let (first, last) = frames_of(2.0, 10.0, 10.0);
        assert!(near(first, 2.0) && near(last, 9.9));
        let (_, odd) = frames_of(2.0, 10.05, 10.0);
        assert!(near(odd, 10.0), "a clip ending inside a frame shows that frame");
    }

    #[test]
    fn marks_with_the_same_number_clash_and_others_join() {
        let marks = vec![
            KeyframeMark {
                at: 1.0,
                keyed: vec!["opacity"],
                ease: None,
            },
            KeyframeMark {
                at: 2.0,
                keyed: vec!["position", "opacity"],
                ease: None,
            },
            KeyframeMark {
                at: 3.0,
                keyed: vec!["size"],
                ease: None,
            },
        ];
        assert!(clashes(&marks, &[1.0], 1.0, 10.0));
        assert!(!clashes(&marks, &[1.0], 2.0, 10.0), "opacity joins size");
        assert!(!clashes(&marks, &[1.0], 0.5, 10.0), "an empty frame");
        assert!(
            !clashes(&marks, &[1.0, 2.0], 1.0, 10.0),
            "both move: 2 s is free by then"
        );
        assert!(marks[1].says().contains("position, opacity"));
    }

    #[test]
    fn a_box_chooses_the_marks_it_holds_and_adds_to_those_kept() {
        let all = marks(&[1.0, 2.0, 3.0, 4.0]);
        let fps = 10.0;
        let mut b = MarkBox {
            clip: 7,
            from: 3.5,
            to: 1.5,
            kept: Vec::new(),
        };
        assert_eq!(
            b.chosen(&all, fps),
            Chosen {
                clip: 7,
                at: vec![2.0, 3.0]
            }
        );
        b.to = 2.0;
        assert_eq!(b.chosen(&all, fps).at, [2.0, 3.0], "a mark on its edge is in");
        b.kept = vec![4.0, 3.0];
        assert_eq!(b.chosen(&all, fps).at, [2.0, 3.0, 4.0], "each once");
        let clip = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 100.0));
        assert!(in_strip(clip, pos2(200.0, 95.0)));
        assert!(in_strip(clip, pos2(200.0, 84.0)));
        assert!(!in_strip(clip, pos2(200.0, 70.0)), "higher up moves the clip");
        assert!(!in_strip(clip, pos2(450.0, 95.0)));
        let thin = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 58.0));
        assert!(!in_strip(thin, pos2(200.0, 55.0)), "no marks on a thin clip");
    }

    #[test]
    fn marks_are_chosen_alone_or_added_and_taken_out() {
        let mut chosen = Chosen::default();
        let fps = 10.0;
        chosen.click(7, 1.0, false, fps);
        chosen.click(7, 2.0, true, fps);
        let all = marks(&[1.0, 2.0, 3.0]);
        assert_eq!(chosen.of(7, &all, fps), [1.0, 2.0]);
        assert!(chosen.of(8, &all, fps).is_empty(), "another clip");
        chosen.click(7, 1.0, true, fps);
        assert_eq!(chosen.of(7, &all, fps), [2.0]);
        chosen.click(8, 3.0, true, fps);
        assert_eq!(chosen.of(8, &all, fps), [3.0], "on another clip: alone");
        chosen.click(8, 2.0, false, fps);
        assert_eq!(chosen.of(8, &all, fps), [2.0]);
    }
}
