//! A clip's fades, drawn on it and changed on it.
//!
//! Each fade is a line from a bottom corner up to the top where the fade
//! in ends, and from the top down to the other bottom corner over the
//! fade out, following the fade's own curve, with the part above the line
//! darkened (what is faded away). A picture fades on a straight line and
//! sound on a quarter sine; a curve bends either ([`level`], the formula
//! the engine uses: `project::fades::level`).
//!
//! On a clip that can be faded, a small square handle sits in each top
//! corner (at the end of the fade, when there is one): dragging it sideways
//! sets the fade's length. A fade wide enough has a round point halfway
//! along its line: dragging it up or down bends the curve, and a
//! double-click makes it plain again. Both show when the pointer is on the
//! clip or it is selected; letting go is one change
//! ([`super::TimelineEvent::ClipFade`]).

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// A clip's fades, for drawing: lengths in seconds on the timeline and
/// curves from -1 (comes in late) to 1 (comes in soon), 0 plain.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClipFades {
    pub fade_in: f64,
    pub fade_out: f64,
    pub in_curve: f32,
    pub out_curve: f32,
}

impl ClipFades {
    /// The length and curve of one end's fade.
    pub fn of(&self, end: FadeEnd) -> (f64, f32) {
        match end {
            FadeEnd::In => (self.fade_in, self.in_curve),
            FadeEnd::Out => (self.fade_out, self.out_curve),
        }
    }

    /// The same fades with one end's set to `length` and `curve`.
    pub fn with(self, end: FadeEnd, length: f64, curve: f32) -> ClipFades {
        match end {
            FadeEnd::In => ClipFades {
                fade_in: length,
                in_curve: curve,
                ..self
            },
            FadeEnd::Out => ClipFades {
                fade_out: length,
                out_curve: curve,
                ..self
            },
        }
    }
}

/// How a clip's sound is set, for drawing: its volume as a factor (the
/// waveform is drawn that much larger or smaller) and its fades.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipSound {
    pub gain: f32,
    pub fades: ClipFades,
}

impl Default for ClipSound {
    fn default() -> ClipSound {
        ClipSound {
            gain: 1.0,
            fades: ClipFades::default(),
        }
    }
}

/// Which end of a clip a fade is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeEnd {
    In,
    Out,
}

/// Points along the curve of a fade drawn across its width.
const STEPS: usize = 24;
/// A handle's size, and how far from it it can still be grabbed.
const HANDLE: f32 = 7.0;
const REACH: f32 = 5.0;
/// A fade narrower than this has no curve point (it would sit on the handle).
const CURVE_ROOM: f32 = 18.0;

/// How far up a fade is `share` of the way along it, bent by `curve`: the
/// plain fade (straight, or a quarter sine for sound) to the power
/// 4^(-curve). The same formula as `project::fades::level`.
pub fn level(share: f64, curve: f32, sine: bool) -> f64 {
    let share = share.clamp(0.0, 1.0);
    let plain = match sine {
        true => (share * std::f64::consts::FRAC_PI_2).sin(),
        false => share,
    };
    plain.powf(4f64.powf(-f64::from(curve.clamp(-1.0, 1.0))))
}

/// The curve that puts a fade's middle at `height` (0 to 1) up: the
/// inverse of [`level`] at half way, held a little inside the ends.
pub fn curve_for(height: f32, sine: bool) -> f32 {
    let height = f64::from(height.clamp(0.01, 0.99));
    let plain = level(0.5, 0.0, sine);
    // plain ^ (4 ^ -c) = height  →  c = -log4(ln height / ln plain)
    let curve = -(height.ln() / plain.ln()).log(4.0);
    (curve as f32).clamp(-1.0, 1.0)
}

/// Where fades are drawn on a clip drawn in `r`: a little inside its top
/// and bottom.
pub fn inside(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.left(), r.top() + 2.0), pos2(r.right(), r.bottom() - 2.0))
}

/// The line of a fade over `r` (see [`inside`]), `width` points wide, from
/// the bottom corner at `end` up to the top.
pub fn line(r: Rect, width: f32, end: FadeEnd, curve: f32, sine: bool) -> Vec<Pos2> {
    let width = width.clamp(0.0, r.width());
    (0..=STEPS)
        .map(|i| {
            let share = i as f32 / STEPS as f32;
            let up = level(f64::from(share), curve, sine) as f32;
            let y = r.bottom() - up * r.height();
            match end {
                FadeEnd::In => pos2(r.left() + share * width, y),
                FadeEnd::Out => pos2(r.right() - share * width, y),
            }
        })
        .collect()
}

/// A fade's width in points at `zoom` points a second, never wider than
/// the clip drawn in `r`.
fn width(r: Rect, seconds: f64, zoom: f32) -> f32 {
    ((seconds * f64::from(zoom)) as f32).clamp(0.0, r.width())
}

/// The centre of the handle of the fade at `end` on a clip drawn in `r`:
/// at the top, where the fade ends, kept inside the clip.
pub fn handle_at(r: Rect, fades: &ClipFades, end: FadeEnd, zoom: f32) -> Pos2 {
    let r = inside(r);
    let w = width(r, fades.of(end).0, zoom);
    let half = HANDLE / 2.0 + 1.0;
    let x = match end {
        FadeEnd::In => (r.left() + w).max(r.left() + half),
        FadeEnd::Out => (r.right() - w).min(r.right() - half),
    };
    pos2(x, r.top() + half)
}

/// The fade's curve point, halfway along its line, when it is wide enough
/// to have one.
pub fn curve_point(r: Rect, fades: &ClipFades, end: FadeEnd, zoom: f32, sine: bool) -> Option<Pos2> {
    let inner = inside(r);
    let (seconds, curve) = fades.of(end);
    let w = width(inner, seconds, zoom);
    if w < CURVE_ROOM {
        return None;
    }
    let up = level(0.5, curve, sine) as f32;
    let y = inner.bottom() - up * inner.height();
    let x = match end {
        FadeEnd::In => inner.left() + w / 2.0,
        FadeEnd::Out => inner.right() - w / 2.0,
    };
    Some(pos2(x, y))
}

/// What of a fade the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeGrip {
    /// The handle: sets its length.
    Length,
    /// The curve point: bends it.
    Curve,
}

/// The handle or curve point under `p` on a clip drawn in `r` that is
/// wide and tall enough to be faded by hand.
pub fn grab(r: Rect, fades: &ClipFades, zoom: f32, sine: bool, p: Pos2) -> Option<(FadeEnd, FadeGrip)> {
    if r.width() < HANDLE * 4.0 || r.height() < 14.0 {
        return None;
    }
    let near =
        |at: Pos2| (at.x - p.x).abs() <= HANDLE / 2.0 + REACH && (at.y - p.y).abs() <= HANDLE / 2.0 + REACH;
    for end in [FadeEnd::In, FadeEnd::Out] {
        if curve_point(r, fades, end, zoom, sine).is_some_and(near) {
            return Some((end, FadeGrip::Curve));
        }
    }
    [FadeEnd::In, FadeEnd::Out]
        .into_iter()
        .find(|end| near(handle_at(r, fades, *end, zoom)))
        .map(|end| (end, FadeGrip::Length))
}

/// Draws `fades` on a clip drawn in `r` at `zoom` points a second: the
/// lines with what they fade away darkened.
pub fn draw(p: &egui::Painter, r: Rect, fades: &ClipFades, sine: bool, zoom: f32) {
    let inner = inside(r);
    for end in [FadeEnd::In, FadeEnd::Out] {
        let (seconds, curve) = fades.of(end);
        let w = width(inner, seconds, zoom);
        if w >= 1.0 {
            let points = line(inner, w, end, curve, sine);
            // The faded part above the line, as a strip of quads up to the top.
            let mut mesh = egui::Mesh::default();
            let shade = Color32::from_black_alpha(90);
            for pair in points.windows(2) {
                let [a, b] = [pair[0], pair[1]];
                let base = mesh.vertices.len() as u32;
                for v in [a, b, pos2(b.x, inner.top()), pos2(a.x, inner.top())] {
                    mesh.colored_vertex(v, shade);
                }
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::line(
                points,
                Stroke::new(1.5, Color32::from_white_alpha(220)),
            ));
        }
    }
}

/// Draws the handles and curve points of `fades` on a clip drawn in `r`,
/// over everything else on it (its name too).
pub fn draw_handles(p: &egui::Painter, r: Rect, fades: &ClipFades, sine: bool, zoom: f32) {
    if r.width() < HANDLE * 4.0 || r.height() < 14.0 {
        return;
    }
    for end in [FadeEnd::In, FadeEnd::Out] {
        let at = handle_at(r, fades, end, zoom);
        let square = Rect::from_center_size(at, vec2(HANDLE, HANDLE));
        p.rect_filled(square, 1.0, Color32::from_white_alpha(235));
        p.rect_stroke(
            square,
            1.0,
            Stroke::new(1.0, Color32::from_black_alpha(160)),
            egui::StrokeKind::Outside,
        );
        if let Some(point) = curve_point(r, fades, end, zoom, sine) {
            p.circle_filled(point, 3.5, Color32::from_white_alpha(235));
            p.circle_stroke(point, 3.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
        }
    }
}

/// A fade being dragged: its handle (the length) or its curve point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FadeDrag {
    pub clip: u64,
    /// The sound's fades (else the picture's).
    pub sound: bool,
    pub end: FadeEnd,
    pub grip: FadeGrip,
    /// The fades when the drag began, and as letting go now would set them.
    pub from: ClipFades,
    pub now: ClipFades,
    /// The clip's length in seconds: a fade in and out together never longer.
    pub length: f64,
}

impl FadeDrag {
    /// The pointer at `p` on the clip drawn in `r` (where it is drawn now,
    /// with `start` its start in seconds) at `zoom` points a second.
    pub fn follow(&mut self, r: Rect, zoom: f32, p: Pos2) {
        let (seconds, curve) = self.now.of(self.end);
        let other = match self.end {
            FadeEnd::In => self.now.fade_out,
            FadeEnd::Out => self.now.fade_in,
        };
        match self.grip {
            FadeGrip::Length => {
                let points = match self.end {
                    FadeEnd::In => p.x - r.left(),
                    FadeEnd::Out => r.right() - p.x,
                };
                let wanted = f64::from(points.max(0.0)) / f64::from(zoom.max(1e-6));
                let length = wanted.clamp(0.0, (self.length - other).max(0.0));
                self.now = self.now.with(self.end, length, curve);
            }
            FadeGrip::Curve => {
                let inner = inside(r);
                let height = (inner.bottom() - p.y) / inner.height().max(1.0);
                self.now = self.now.with(self.end, seconds, curve_for(height, self.sound));
            }
        }
    }

    /// Whether letting go changes anything.
    pub fn changed(&self) -> bool {
        self.now != self.from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_in_rises_from_the_bottom_corner_and_a_fade_out_falls_to_the_other() {
        let r = Rect::from_min_max(pos2(100.0, 0.0), pos2(300.0, 40.0));
        let up = line(r, 50.0, FadeEnd::In, 0.0, true);
        assert_eq!(up.first(), Some(&pos2(100.0, 40.0)));
        assert_eq!(up.last(), Some(&pos2(150.0, 0.0)));
        let down = line(r, 50.0, FadeEnd::Out, 0.0, true);
        assert_eq!(down.first(), Some(&pos2(300.0, 40.0)));
        assert_eq!(down.last(), Some(&pos2(250.0, 0.0)));
        // Halfway along, sound's curve is past halfway up (3 dB down, not
        // 6); a picture's is straight.
        let middle = up[STEPS / 2];
        assert!((middle.x - 125.0).abs() < 0.01);
        assert!(middle.y < 20.0 - 7.0, "{}", middle.y);
        assert!((line(r, 50.0, FadeEnd::In, 0.0, false)[STEPS / 2].y - 20.0).abs() < 0.01);
        // Never wider than the clip.
        assert_eq!(
            line(r, 900.0, FadeEnd::In, 0.0, true).last().map(|p| p.x),
            Some(300.0)
        );
    }

    #[test]
    fn a_curve_bends_the_middle_and_the_middle_gives_the_curve_back() {
        assert!(level(0.5, 1.0, false) > level(0.5, 0.0, false));
        assert!(level(0.5, -1.0, false) < level(0.5, 0.0, false));
        for sine in [false, true] {
            for curve in [-0.8f32, -0.3, 0.0, 0.5, 0.9] {
                let up = level(0.5, curve, sine) as f32;
                assert!((curve_for(up, sine) - curve).abs() < 1e-3, "{sine} {curve}");
            }
        }
        // Dragged past the top or bottom: held to the ends of the range.
        assert_eq!(curve_for(2.0, false), 1.0);
        assert_eq!(curve_for(-1.0, true), -1.0);
    }

    #[test]
    fn handles_sit_in_the_top_corners_and_follow_the_fade() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 50.0));
        let none = ClipFades::default();
        // No fade: just inside each top corner, grabbed there.
        let left = handle_at(r, &none, FadeEnd::In, 100.0);
        assert!(left.x < 10.0 && left.y < 10.0, "{left:?}");
        assert_eq!(
            grab(r, &none, 100.0, false, left),
            Some((FadeEnd::In, FadeGrip::Length))
        );
        let right = handle_at(r, &none, FadeEnd::Out, 100.0);
        assert!(right.x > 390.0);
        assert_eq!(
            grab(r, &none, 100.0, false, pos2(200.0, 25.0)),
            None,
            "the middle moves the clip"
        );
        // A one-second fade in at 100 points a second: the handle at 100,
        // the curve point halfway along its line.
        let faded = ClipFades {
            fade_in: 1.0,
            ..ClipFades::default()
        };
        assert!((handle_at(r, &faded, FadeEnd::In, 100.0).x - 100.0).abs() < 0.01);
        let point = curve_point(r, &faded, FadeEnd::In, 100.0, false).unwrap();
        assert!(
            (point.x - 50.0).abs() < 0.01 && (point.y - 25.0).abs() < 0.01,
            "{point:?}"
        );
        assert_eq!(
            grab(r, &faded, 100.0, false, point),
            Some((FadeEnd::In, FadeGrip::Curve))
        );
        // Too narrow a fade has no curve point.
        assert_eq!(curve_point(r, &faded, FadeEnd::In, 10.0, false), None);
    }

    #[test]
    fn dragging_sets_the_length_within_the_clip_and_bends_the_curve() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 50.0));
        let mut drag = FadeDrag {
            clip: 1,
            sound: false,
            end: FadeEnd::Out,
            grip: FadeGrip::Length,
            from: ClipFades {
                fade_in: 1.5,
                ..ClipFades::default()
            },
            now: ClipFades {
                fade_in: 1.5,
                ..ClipFades::default()
            },
            length: 4.0,
        };
        assert!(!drag.changed());
        drag.follow(r, 100.0, pos2(300.0, 5.0));
        assert!((drag.now.fade_out - 1.0).abs() < 1e-9);
        // Never into the fade in: the clip is 4 s, the fade in 1.5 s.
        drag.follow(r, 100.0, pos2(-50.0, 5.0));
        assert!((drag.now.fade_out - 2.5).abs() < 1e-9);
        assert!(drag.changed());
        // The curve point dragged up: the fade comes in sooner.
        drag.grip = FadeGrip::Curve;
        drag.follow(r, 100.0, pos2(0.0, 10.0));
        assert!(
            drag.now.out_curve > 0.0 && drag.now.fade_out == 2.5,
            "{:?}",
            drag.now
        );
    }
}
