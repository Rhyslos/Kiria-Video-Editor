//! A ramped clip's speed drawn on it: a thin line over the lower part of
//! the clip, high where it plays fast and low where it plays slowly, with
//! normal speed in the middle of the band and a faint line marking it.

use super::SpeedCurve;
use egui::{Color32, Pos2, Rect, Stroke, pos2};

/// How far the band reaches either side of normal speed, in doublings:
/// four times as fast at the top, a quarter at the bottom.
const DOUBLINGS: f32 = 2.0;

/// The band the line is drawn in: the lower part of a clip drawn in `r`,
/// below where its name is written.
pub fn band(r: Rect) -> Rect {
    let top = r.top() + (r.height() * 0.45).max(18.0);
    Rect::from_min_max(pos2(r.left() + 1.0, top), pos2(r.right() - 1.0, r.bottom() - 4.0))
}

/// Where a speed goes in `band`, up and down.
pub fn y_of(band: Rect, speed: f32) -> f32 {
    let doublings = speed.max(1e-3).log2().clamp(-DOUBLINGS, DOUBLINGS);
    band.center().y - doublings / DOUBLINGS * band.height() / 2.0
}

/// The curve's points in a clip drawn in `r`.
pub fn line(r: Rect, curve: &SpeedCurve) -> Vec<Pos2> {
    let band = band(r);
    curve
        .0
        .iter()
        .map(|(share, speed)| pos2(r.left() + share * r.width(), y_of(band, *speed)))
        .collect()
}

/// Draws `curve` on a clip drawn in `r`, in `colour`. Nothing in a clip too
/// short to hold it.
pub fn draw(p: &egui::Painter, r: Rect, curve: &SpeedCurve, colour: Color32) {
    let band = band(r);
    if band.height() < 8.0 || r.width() < 8.0 || curve.0.len() < 2 {
        return;
    }
    let normal = y_of(band, 1.0);
    p.line_segment(
        [pos2(band.left(), normal), pos2(band.right(), normal)],
        Stroke::new(1.0, Color32::from_white_alpha(40)),
    );
    let points = line(r, curve);
    // A dark edge under the line, so it reads over pictures and waveforms.
    p.add(egui::Shape::line(
        points.clone(),
        Stroke::new(3.0, Color32::from_black_alpha(140)),
    ));
    p.add(egui::Shape::line(points, Stroke::new(1.5, colour)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn faster_is_higher_and_normal_is_the_middle() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 80.0));
        let band = band(r);
        assert_eq!(y_of(band, 1.0), band.center().y);
        assert!(y_of(band, 4.0) <= band.top() + 0.01);
        assert!(y_of(band, 0.25) >= band.bottom() - 0.01);
        assert_eq!(y_of(band, 0.1), y_of(band, 0.25), "slower stays at the bottom");
        let curve = SpeedCurve(vec![(0.0, 1.0), (1.0, 0.5)].into());
        let points = line(r, &curve);
        assert_eq!(points[0].x, 0.0);
        assert_eq!(points[1].x, 200.0);
        assert!(points[1].y > points[0].y);
    }
}
