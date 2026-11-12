//! A clip's fades drawn on it: from the bottom corner up to the top where
//! the fade in ends, and from the top down to the other bottom corner over
//! the fade out, following the fade's own curve, with the part above the
//! line darkened (the sound that is faded away).

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};

/// How a clip's sound is set, for drawing: its volume as a factor (the
/// waveform is drawn that much larger or smaller) and its fades in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipSound {
    pub gain: f32,
    pub fade_in: f64,
    pub fade_out: f64,
}

impl Default for ClipSound {
    fn default() -> ClipSound {
        ClipSound {
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        }
    }
}

/// Points along the curve of a fade drawn across `width` pixels.
const STEPS: usize = 16;

/// The line of a fade in over `r`, from its bottom left corner up to its
/// top `width` pixels in; mirrored for a fade out (from the top `width`
/// pixels before the right edge down to the bottom right corner).
pub fn line(r: Rect, width: f32, fade_out: bool) -> Vec<Pos2> {
    let width = width.clamp(0.0, r.width());
    (0..=STEPS)
        .map(|i| {
            let share = i as f32 / STEPS as f32;
            // The same quarter sine the mixer uses: how far up the sound is.
            let level = (share * std::f32::consts::FRAC_PI_2).sin();
            let y = r.bottom() - level * r.height();
            if fade_out {
                pos2(r.right() - share * width, y)
            } else {
                pos2(r.left() + share * width, y)
            }
        })
        .collect()
}

/// Draws `sound`'s fades on a clip drawn in `r` at `zoom` pixels a second.
pub fn draw(p: &egui::Painter, r: Rect, sound: &ClipSound, zoom: f32) {
    let inside = Rect::from_min_max(pos2(r.left(), r.top() + 2.0), pos2(r.right(), r.bottom() - 2.0));
    for (seconds, fade_out) in [(sound.fade_in, false), (sound.fade_out, true)] {
        let width = (seconds * f64::from(zoom)) as f32;
        if width < 1.0 {
            continue;
        }
        let points = line(inside, width, fade_out);
        // The faded part above the line, as a strip of quads up to the top.
        let mut mesh = egui::Mesh::default();
        let shade = Color32::from_black_alpha(90);
        for pair in points.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            let base = mesh.vertices.len() as u32;
            for v in [a, b, pos2(b.x, inside.top()), pos2(a.x, inside.top())] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_in_rises_from_the_bottom_corner_and_a_fade_out_falls_to_the_other() {
        let r = Rect::from_min_max(pos2(100.0, 0.0), pos2(300.0, 40.0));
        let up = line(r, 50.0, false);
        assert_eq!(up.first(), Some(&pos2(100.0, 40.0)));
        assert_eq!(up.last(), Some(&pos2(150.0, 0.0)));
        let down = line(r, 50.0, true);
        assert_eq!(down.first(), Some(&pos2(300.0, 40.0)));
        assert_eq!(down.last(), Some(&pos2(250.0, 0.0)));
        // Halfway along, the curve is past halfway up (3 dB down, not 6).
        let middle = up[STEPS / 2];
        assert!((middle.x - 125.0).abs() < 0.01);
        assert!(middle.y < 20.0 - 7.0, "{}", middle.y);
        // Never wider than the clip.
        assert_eq!(line(r, 900.0, false).last().map(|p| p.x), Some(300.0));
    }
}
