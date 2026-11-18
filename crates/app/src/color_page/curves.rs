//! The Curves tool: one curve shown at a time (Master, Red, Green or Blue)
//! in a square, the others faint behind it. Click to add a point, drag a
//! point to move it, double-click one to take it away; the ends move up
//! and down only. Reset curve straightens the one shown.

use project::grade::ONE;
use project::{Curve, Curves};
use ui_kit::Theme;

/// Which curve is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Master,
    Red,
    Green,
    Blue,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Master, Channel::Red, Channel::Green, Channel::Blue];

    fn name(self) -> &'static str {
        match self {
            Channel::Master => "Master",
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
        }
    }

    fn colour(self, theme: &Theme) -> egui::Color32 {
        match self {
            Channel::Master => theme.text,
            Channel::Red => egui::Color32::from_rgb(235, 90, 90),
            Channel::Green => egui::Color32::from_rgb(90, 200, 110),
            Channel::Blue => egui::Color32::from_rgb(90, 140, 240),
        }
    }

    fn of(self, curves: &mut Curves) -> &mut Curve {
        match self {
            Channel::Master => &mut curves.master,
            Channel::Red => &mut curves.red,
            Channel::Green => &mut curves.green,
            Channel::Blue => &mut curves.blue,
        }
    }
}

/// How near (points on screen) a click must be to take a point.
const NEAR: f32 = 9.0;

/// The points of `curve` with its ends made when missing, as shares of one,
/// in order along (a curve from a file or a paste may not be).
fn points(curve: &Curve) -> Vec<(f64, f64)> {
    let one = f64::from(ONE);
    let mut points: Vec<(f64, f64)> = curve
        .0
        .iter()
        .map(|(x, y)| (f64::from(*x) / one, f64::from(*y) / one))
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    if points.first().is_none_or(|(x, _)| *x > 0.0) {
        points.insert(0, (0.0, 0.0));
    }
    if points.last().is_none_or(|(x, _)| *x < 1.0) {
        points.push((1.0, 1.0));
    }
    points
}

/// `points` kept as a curve, straight ones left out.
fn kept(points: &[(f64, f64)]) -> Curve {
    let one = f64::from(ONE);
    let curve = Curve(
        points
            .iter()
            .map(|(x, y)| ((x * one).round() as i32, (y * one).round() as i32))
            .collect(),
    );
    if curve.is_plain() { Curve::default() } else { curve }
}

/// Adds a point at (`x`, `y`) between the ones around it; `None` when one
/// is already there or the curve is full. Says where it went.
pub fn add(curve: &Curve, x: f64, y: f64) -> Option<(Curve, usize)> {
    let mut p = points(curve);
    if p.len() >= Curve::MOST_POINTS || p.iter().any(|(px, _)| (px - x).abs() < 0.02) {
        return None;
    }
    let at = p.iter().position(|(px, _)| *px > x).unwrap_or(p.len());
    p.insert(at, (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    Some((kept(&p), at))
}

/// Moves point `index` to (`x`, `y`): between its neighbours, the ends
/// only up and down. Neighbours too close to keep it a hundredth from
/// each (as a file or a paste may hold them) leave it halfway between.
pub fn moved(curve: &Curve, index: usize, x: f64, y: f64) -> Curve {
    let mut p = points(curve);
    let last = p.len() - 1;
    if index > last {
        return curve.clone();
    }
    let x = match index {
        0 => 0.0,
        i if i == last => 1.0,
        i => {
            let (before, after) = (p[i - 1].0, p[i + 1].0);
            let (lo, hi) = (before + 0.01, after - 0.01);
            if lo <= hi {
                x.clamp(lo, hi)
            } else {
                (before + after) / 2.0
            }
        }
    };
    p[index] = (x, y.clamp(0.0, 1.0));
    kept(&p)
}

/// Takes point `index` away (never an end).
pub fn removed(curve: &Curve, index: usize) -> Curve {
    let mut p = points(curve);
    if index == 0 || index + 1 >= p.len() {
        return curve.clone();
    }
    p.remove(index);
    kept(&p)
}

/// Draws the tool; says whether a point is still held.
pub fn ui(ui: &mut egui::Ui, theme: &Theme, curves: &mut Curves, channel: &mut Channel) -> bool {
    ui.horizontal(|ui| {
        for c in Channel::ALL {
            ui.selectable_value(channel, c, c.name());
        }
        ui.separator();
        if ui.button("Reset curve").clicked() {
            *channel.of(curves) = Curve::default();
        }
        ui.label(
            egui::RichText::new("Click to add a point, drag to move, double-click to take away")
                .color(theme.muted)
                .size(12.0),
        );
    });
    let side = (ui.available_height() - 6.0).clamp(80.0, 300.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(6.0));
    painter.rect_filled(rect, 4.0, theme.bg);
    for i in 1..4 {
        let k = i as f32 / 4.0;
        let stroke = egui::Stroke::new(1.0, theme.line);
        painter.line_segment(
            [
                rect.lerp_inside(egui::vec2(k, 0.0)),
                rect.lerp_inside(egui::vec2(k, 1.0)),
            ],
            stroke,
        );
        painter.line_segment(
            [
                rect.lerp_inside(egui::vec2(0.0, k)),
                rect.lerp_inside(egui::vec2(1.0, k)),
            ],
            stroke,
        );
    }
    let to_screen = |(x, y): (f64, f64)| rect.lerp_inside(egui::vec2(x as f32, 1.0 - y as f32));
    let from_screen = |p: egui::Pos2| {
        let v = (p - rect.min) / rect.size();
        (
            f64::from(v.x).clamp(0.0, 1.0),
            f64::from(1.0 - v.y).clamp(0.0, 1.0),
        )
    };
    let line = |curve: &Curve, colour: egui::Color32, width: f32| {
        let samples: Vec<egui::Pos2> = (0..=64)
            .map(|i| {
                let x = f64::from(i) / 64.0;
                to_screen((x, curve.at(x)))
            })
            .collect();
        painter.add(egui::Shape::line(samples, egui::Stroke::new(width, colour)));
    };
    for c in Channel::ALL.into_iter().filter(|c| c != channel) {
        line(
            c.of(&mut curves.clone()),
            c.colour(theme).gamma_multiply(0.35),
            1.0,
        );
    }
    let colour = channel.colour(theme);
    let curve = channel.of(curves);
    line(curve, colour, 2.0);
    // Pointer: take, move, add or take away a point.
    let held_id = response.id.with("held point");
    let mut held: Option<usize> = ui.data(|d| d.get_temp(held_id));
    let near = |curve: &Curve, at: egui::Pos2| {
        points(curve)
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_screen(*p).distance(at)))
            .filter(|(_, d)| *d <= NEAR)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    if let Some(at) = response.interact_pointer_pos() {
        let (x, y) = from_screen(at);
        if response.double_clicked() {
            if let Some(i) = near(curve, at) {
                *curve = removed(curve, i);
            }
            held = None;
        } else if response.drag_started() || (response.clicked() && held.is_none()) {
            held = near(curve, at).or_else(|| {
                add(curve, x, y).map(|(added, i)| {
                    *curve = added;
                    i
                })
            });
        }
        if response.dragged()
            && let Some(i) = held
        {
            *curve = moved(curve, i, x, y);
        }
    }
    if !response.dragged() && !response.drag_started() {
        held = None;
    }
    ui.data_mut(|d| match held {
        Some(i) => {
            d.insert_temp(held_id, i);
        }
        None => {
            d.remove::<usize>(held_id);
        }
    });
    for (i, p) in points(curve).into_iter().enumerate() {
        let lit = held == Some(i);
        painter.circle_filled(to_screen(p), if lit { 5.0 } else { 4.0 }, colour);
        painter.circle_stroke(to_screen(p), 4.0, egui::Stroke::new(1.0, theme.bg));
    }
    response.dragged()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_added_moved_and_taken_away_ends_staying_put() {
        let straight = Curve::default();
        let (bent, i) = add(&straight, 0.5, 0.7).expect("added");
        assert_eq!(i, 1, "between the two ends");
        assert!((bent.at(0.5) - 0.7).abs() < 1e-6);
        assert!(add(&bent, 0.505, 0.2).is_none(), "one is there already");
        // An end moves only up and down.
        let lifted = moved(&bent, 0, 0.3, 0.1);
        assert!((lifted.at(0.0) - 0.1).abs() < 1e-6);
        // A middle point stays between its neighbours.
        let pushed = moved(&bent, 1, 2.0, 0.7);
        assert!(pushed.0.iter().all(|(x, _)| *x <= ONE));
        assert_eq!(
            removed(&bent, 1),
            Curve::default(),
            "straight again: kept as none"
        );
        assert_eq!(removed(&bent, 0), bent, "never an end");
    }

    /// Every point of `curve` moved every way, as a drag could: none panics.
    fn drag_every_point(curve: &Curve) -> Vec<Curve> {
        let mut out = Vec::new();
        for i in 0..points(curve).len() + 1 {
            for (x, y) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (-3.0, 7.0), (0.3, 0.9)] {
                out.push(moved(curve, i, x, y));
            }
            out.push(removed(curve, i));
        }
        out.extend(add(curve, 0.42, 0.6).map(|(c, _)| c));
        out
    }

    #[test]
    fn odd_curves_from_a_file_or_a_paste_never_panic_when_dragged() {
        let odd = [
            // Closer than 0.02: no room for a hundredth either side.
            Curve(vec![(500_000, 400_000), (505_000, 600_000), (510_000, 700_000)]),
            // Two at the same place.
            Curve(vec![(300_000, 200_000), (300_000, 800_000)]),
            // Out of order.
            Curve(vec![(800_000, 100_000), (200_000, 900_000), (500_000, 500_000)]),
            // At the extremes, and past them.
            Curve(vec![(0, ONE), (ONE, 0)]),
            Curve(vec![(i32::MIN, i32::MIN), (i32::MAX, i32::MAX)]),
            Curve(vec![(i32::MAX, i32::MIN), (i32::MIN, i32::MAX), (0, 0)]),
            Curve(vec![(i32::MAX, i32::MAX); 20]),
        ];
        for (n, curve) in odd.iter().enumerate() {
            for dragged in drag_every_point(curve) {
                // Points from 0 to 1, however close or muddled, are kept in
                // order when changed; past the ends it is enough not to panic.
                assert!(
                    n > 3 || dragged == *curve || dragged.0.windows(2).all(|w| w[0].0 <= w[1].0),
                    "{curve:?} became {dragged:?}"
                );
            }
        }
        // A middle point with no room stays between its neighbours.
        let tight = &odd[0];
        let held = moved(tight, 2, 0.9, 0.5);
        assert_eq!(held.0[2], (505_000, 500_000), "halfway between, not past one");
        // Out of order: the page shows and moves them in order along.
        let sorted = moved(&odd[2], 2, 0.5, 0.5);
        assert_eq!(
            sorted.0.iter().map(|(x, _)| *x).collect::<Vec<_>>(),
            vec![0, 200_000, 500_000, 800_000, ONE]
        );
    }
}
