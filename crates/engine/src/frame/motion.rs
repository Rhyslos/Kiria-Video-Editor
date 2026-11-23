//! Motion blur: how a picture moved while the shutter was open, worked out
//! from its passes and its place in the frame at two moments.
//!
//! A clip's picture moves in two ways: its motion effects move it inside
//! its own frame (`render::Effect::Move`, each a map from a place in what
//! it makes to the place it takes from), and its look places it in the
//! timeline's frame (`Area`). A Motion blur pass is handed the map from a
//! place in the picture it is given to where what is there was when the
//! shutter opened; the shader averages the picture along the way.

use project::{Clip, Time};
use render::Area;

/// A map of places in a picture (0-1 across and down):
/// `(x, y)` to `(a x + b y + e, c x + d y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    m: [f64; 6],
}

impl Affine {
    pub const SAME: Affine = Affine {
        m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };

    #[cfg(test)]
    pub fn apply(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.m;
        (a * x + b * y + e, c * x + d * y + f)
    }

    /// `self` after `first`: a place is taken by `first`, then by `self`.
    pub fn after(&self, first: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.m;
        let [p, q, r, s, t, u] = first.m;
        Affine {
            m: [
                a * p + b * r,
                a * q + b * s,
                c * p + d * r,
                c * q + d * s,
                a * t + b * u + e,
                c * t + d * u + f,
            ],
        }
    }

    /// The map back; none when it squashes everything onto a line.
    pub fn inverse(&self) -> Option<Affine> {
        let [a, b, c, d, e, f] = self.m;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Affine {
            m: [ia, ib, ic, id, -(ia * e + ib * f), -(ic * e + id * f)],
        })
    }

    /// Where a Move pass takes each place in what it makes from, in a
    /// picture `aspect` times as wide as tall (as the shader does it).
    pub fn of_move(scale: f32, fixed: (f32, f32), turn: f32, shift: (f32, f32), aspect: f64) -> Affine {
        let scale = f64::from(scale.max(0.01));
        let (s, c) = f64::from(turn).sin_cos();
        // In pixels the turn keeps the picture's shape: [[c, s], [-s, c]]
        // between the picture's own sides.
        let (a, b, cc, d) = (c / scale, s / aspect / scale, -s * aspect / scale, c / scale);
        let (fx, fy) = (f64::from(fixed.0), f64::from(fixed.1));
        let (sx, sy) = (f64::from(shift.0), f64::from(shift.1));
        Affine {
            m: [a, b, cc, d, fx - a * fx - b * fy + sx, fy - cc * fx - d * fy + sy],
        }
    }

    /// From the picture's places (0-1) to the frame's pixels.
    pub fn of_area(area: &Area) -> Affine {
        let f = |v: f32| f64::from(v);
        Affine {
            m: [
                f(area.across.0),
                f(area.down.0),
                f(area.across.1),
                f(area.down.1),
                f(area.origin.0),
                f(area.origin.1),
            ],
        }
    }

    /// `share` (0-1) of the way from leaving every place where it is to this.
    pub fn part(&self, share: f64) -> Affine {
        let same = Affine::SAME.m;
        let mut m = [0.0; 6];
        for (i, v) in m.iter_mut().enumerate() {
            *v = same[i] + (self.m[i] - same[i]) * share;
        }
        Affine { m }
    }

    pub fn numbers(&self) -> [f32; 6] {
        self.m.map(|v| v as f32)
    }
}

/// Fills in the back maps of `passes`' Motion passes: `passes` are the
/// clip's at `at`; `passes_at` makes them for another moment (the same
/// passes in the same order, the clip's settings then), and `area_at` gives
/// where the picture is in the frame at a moment.
pub fn blur(
    clip: &Clip,
    at: Time,
    area_at: &dyn Fn(Time) -> Area,
    passes: &mut [render::Effect],
    passes_at: fn(&Clip, Time) -> Vec<render::Effect>,
) {
    let source = clip.source_at(at);
    let shutters: Vec<f64> = clip
        .effects
        .applied()
        .filter_map(|effect| match effect.kind_at(source) {
            project::EffectKind::Motion(m) => Some(m.open_seconds()),
            _ => None,
        })
        .collect();
    let longest = shutters.iter().copied().fold(0.0, f64::max);
    if longest <= 0.0 {
        return;
    }
    let opened = opened_at(clip, at, longest);
    if opened == at {
        return;
    }
    let earlier = passes_at(clip, opened);
    if earlier.len() != passes.len() {
        return;
    }
    let (now_area, then_area) = (area_at(at), area_at(opened));
    let (w, h) = now_area.size();
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let aspect = f64::from(w) / f64::from(h);
    let placed = Affine::of_area(&now_area);
    let Some(unplaced_then) = Affine::of_area(&then_area).inverse() else {
        return;
    };
    // Where a place of the placed picture was in it when the shutter
    // opened, as far as its look goes.
    let by_look = unplaced_then.after(&placed);
    // The motion effects so far, now and then.
    let (mut moved_now, mut moved_then) = (Affine::SAME, Affine::SAME);
    let mut shutter = shutters.iter();
    for (now, then) in passes.iter_mut().zip(&earlier) {
        match (*now, *then) {
            (
                render::Effect::Move {
                    scale,
                    fixed,
                    turn,
                    shift,
                },
                render::Effect::Move {
                    scale: scale_then,
                    fixed: fixed_then,
                    turn: turn_then,
                    shift: shift_then,
                },
            ) => {
                // A later move's map is taken first.
                moved_now = moved_now.after(&Affine::of_move(scale, fixed, turn, shift, aspect));
                moved_then = moved_then.after(&Affine::of_move(
                    scale_then, fixed_then, turn_then, shift_then, aspect,
                ));
            }
            (render::Effect::Motion { .. }, render::Effect::Motion { .. }) => {
                let open = shutter.next().copied().unwrap_or(0.0);
                let Some(unmoved_now) = moved_now.inverse() else {
                    continue;
                };
                // Back through the moves now to the file's picture, out
                // through the moves then, and through the look.
                let back = unmoved_now.after(&moved_then).after(&by_look);
                *now = render::Effect::Motion {
                    back: back.part(open / longest).numbers(),
                };
            }
            _ => {}
        }
    }
}

/// When the shutter opened for the frame at `at`: `seconds` before it, not
/// before the clip starts.
fn opened_at(clip: &Clip, at: Time, seconds: f64) -> Time {
    // In flicks, the finest step the timeline counts in.
    const FLICKS: u64 = 705_600_000;
    let back = Time::from_ratio((seconds * FLICKS as f64).round() as i64, FLICKS).unwrap_or(Time::ZERO);
    (at - back).max(clip.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn a_move_takes_places_as_the_shader_does() {
        // Shifted a tenth across: each place takes from a tenth to its right.
        let shifted = Affine::of_move(1.0, (0.5, 0.5), 0.0, (0.1, 0.0), 16.0 / 9.0);
        assert!(near(shifted.apply((0.5, 0.5)), (0.6, 0.5)));
        // Twice the size about a point: that point stays, others take from
        // halfway towards it.
        let enlarged = Affine::of_move(2.0, (0.2, 0.8), 0.0, (0.0, 0.0), 1.0);
        assert!(near(enlarged.apply((0.2, 0.8)), (0.2, 0.8)));
        assert!(near(enlarged.apply((0.6, 0.8)), (0.4, 0.8)));
        // A quarter turn in a wide picture keeps its shape: a step across
        // of a height becomes a step down of a height.
        let turned = Affine::of_move(1.0, (0.5, 0.5), std::f32::consts::FRAC_PI_2, (0.0, 0.0), 2.0);
        let (x, y) = turned.apply((0.75, 0.5));
        assert!((x - 0.5).abs() < 1e-6 && (y - 0.0).abs() < 1e-6, "{x}, {y}");
    }

    #[test]
    fn a_map_and_its_inverse_leave_every_place_where_it_is() {
        let m = Affine::of_move(1.7, (0.3, 0.6), 0.4, (0.05, -0.1), 16.0 / 9.0);
        let back = m.inverse().unwrap();
        for p in [(0.0, 0.0), (1.0, 0.3), (0.25, 0.9)] {
            assert!(near(back.after(&m).apply(p), p));
            assert!(near(m.apply(back.apply(p)), p));
        }
        assert!(Affine { m: [0.0; 6] }.inverse().is_none());
        assert_eq!(m.part(0.0), Affine::SAME);
        assert!(near(m.part(1.0).apply((0.2, 0.2)), m.apply((0.2, 0.2))));
    }

    #[test]
    fn a_picture_moved_in_the_frame_was_back_where_it_came_from() {
        // 100 pixels wide, moved 10 pixels right while the shutter was open.
        let area = |x: f32| Area {
            origin: (x, 0.0),
            across: (100.0, 0.0),
            down: (0.0, 50.0),
        };
        let back = Affine::of_area(&area(0.0))
            .inverse()
            .unwrap()
            .after(&Affine::of_area(&area(10.0)));
        assert!(near(back.apply((0.5, 0.5)), (0.6, 0.5)));
    }
}
