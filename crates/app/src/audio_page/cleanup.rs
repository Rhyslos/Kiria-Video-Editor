//! The Clean-up tool: learn the noise from the region (or the selected
//! clip) on a worker, see the profile it learned, and set how much of it
//! goes. The page turns a learned profile into one undo step that also
//! turns clean-up on.

use super::Touched;
use super::dynamics::draw_meter;
use audio::dsp::bands;
use egui::{Align2, FontId, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use project::track_sound::Cleanup;
use project::{Time, TrackId};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// What the noise would be learned from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LearnFrom {
    Region(Time, Time),
    /// The selected clip of this track.
    Clip(Time, Time),
    Nothing,
}

impl LearnFrom {
    pub fn span(self) -> Option<(Time, Time)> {
        match self {
            LearnFrom::Region(a, b) | LearnFrom::Clip(a, b) => Some((a, b)),
            LearnFrom::Nothing => None,
        }
    }

    fn button(self) -> &'static str {
        match self {
            LearnFrom::Region(..) => "Learn noise from the region",
            LearnFrom::Clip(..) => "Learn noise from the selected clip",
            LearnFrom::Nothing => "Learn noise",
        }
    }
}

/// What the tool asks the page.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Asks {
    pub touched: Touched,
    /// Learn the noise from this stretch.
    pub learn: Option<(Time, Time)>,
}

/// How many decibels the drawing spans, below its top.
const SPAN: f32 = 80.0;

/// The drawing's scale for levels `bands` (dBFS): from [`SPAN`] under its
/// top to the top, which is at least 10 dB over the loudest band, on a
/// whole 10 dB, and never over 0 dBFS. Returns (floor, top).
pub fn scale(bands: &[f32]) -> (f32, f32) {
    let loudest = bands.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let top = if loudest.is_finite() {
        ((loudest / 10.0).ceil() * 10.0 + 10.0).min(0.0)
    } else {
        -30.0
    };
    (top - SPAN, top)
}

/// The tool. `learning` is the track being learned, if any.
pub fn ui(
    ui: &mut Ui,
    theme: &Theme,
    c: &mut Cleanup,
    reducing_db: Option<f32>,
    from: LearnFrom,
    learning: Option<TrackId>,
) -> Asks {
    let mut asks = Asks::default();
    let full = ui.available_rect_before_wrap();
    let graph = Rect::from_min_size(
        full.min,
        vec2((full.width() * 0.5).max(160.0), (full.height() - 26.0).max(80.0)),
    );
    draw_profile(ui, theme, graph, c);
    let meter = Rect::from_min_size(
        pos2(graph.right() + 10.0, graph.top() + 14.0),
        vec2(14.0, graph.height() - 28.0),
    );
    draw_meter(ui, theme, meter, reducing_db.filter(|_| c.on));
    let side = Rect::from_min_max(
        pos2(meter.right() + 16.0, full.top()),
        pos2(full.right(), graph.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = c.on;
    if ui2.checkbox(&mut on, "Clean-up on").changed() {
        c.on = on;
        asks.touched.changed = true;
    }
    ui2.horizontal(|ui| {
        let busy = learning.is_some();
        let button = ui.add_enabled(from.span().is_some() && !busy, egui::Button::new(from.button()));
        let button = if from.span().is_none() {
            button.on_disabled_hover_text("Mark a region over a pause where only the noise is heard (or select a clip of this track), then learn it.")
        } else {
            button.on_hover_text("Up to 3 s is taken as noise alone; longer, its quietest quarter (the pauses) is.")
        };
        if button.clicked() {
            asks.learn = from.span();
        }
        if busy {
            ui.spinner();
        }
    });
    let mut amount = -f64::from(c.reduction_mdb) / 1_000.0;
    let t = inspector::slider(
        ui2,
        theme,
        "Amount",
        &mut amount,
        Number {
            range: (0.0, 60.0),
            per_point: 0.2,
            decimals: 0,
            suffix: " dB",
            usual: 18.0,
        },
    );
    asks.touched = asks.touched.and(t);
    let mut threshold = f64::from(c.threshold_mdb) / 1_000.0;
    let t = inspector::slider(
        ui2,
        theme,
        "Over noise",
        &mut threshold,
        Number {
            range: (0.0, 24.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 6.0,
        },
    );
    asks.touched = asks.touched.and(t);
    let mut release = f64::from(c.release_us) / 1_000.0;
    let t = inspector::slider(
        ui2,
        theme,
        "Release",
        &mut release,
        Number {
            range: (10.0, 2_000.0),
            per_point: 2.0,
            decimals: 0,
            suffix: " ms",
            usual: 120.0,
        },
    );
    asks.touched = asks.touched.and(t);
    c.reduction_mdb = -(amount * 1_000.0).round() as i32;
    c.threshold_mdb = (threshold * 1_000.0).round() as i32;
    c.release_us = (release * 1_000.0).round() as u32;
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), graph.bottom())),
        Sense::hover(),
    );
    asks
}

/// The learned noise across the frequencies, or a note that there is none.
fn draw_profile(ui: &Ui, theme: &Theme, r: Rect, c: &Cleanup) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let Some(profile) = c.profile.as_ref().filter(|p| p.levels_mdb.len() == bands::COUNT) else {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "No noise learned yet",
            FontId::proportional(12.0),
            theme.muted,
        );
        return;
    };
    // Stored per hertz on the clean-up's own scale; drawn as each band's
    // share of the noise, dBFS, so the bands add up to the noise's level.
    let learned =
        audio::dsp::cleanup::NoiseProfile(std::array::from_fn(|i| profile.levels_mdb[i] as f32 / 1_000.0));
    let levels = learned.band_dbfs();
    let (floor, top) = scale(&levels);
    let x = |i: usize| r.left() + (i as f32 + 0.5) / bands::COUNT as f32 * r.width();
    let y = |db: f32| {
        let t = ((db - floor) / (top - floor)).clamp(0.0, 1.0);
        r.bottom() - t * r.height()
    };
    let colour = if c.on {
        ui_kit::status::WARNING
    } else {
        theme.muted
    };
    let fill = colour.gamma_multiply(0.2);
    let points: Vec<_> = levels
        .iter()
        .enumerate()
        .map(|(i, l)| pos2(x(i), y(*l)))
        .collect();
    for w in points.windows(2) {
        p.add(Shape::convex_polygon(
            vec![w[0], w[1], pos2(w[1].x, r.bottom()), pos2(w[0].x, r.bottom())],
            fill,
            Stroke::NONE,
        ));
    }
    p.add(Shape::line(points, Stroke::new(1.5, colour)));
    for (f, label) in [(100.0, "100"), (1_000.0, "1k"), (10_000.0, "10k")] {
        let at = r.left() + bands::position(f) / bands::COUNT as f32 * r.width();
        p.text(
            pos2(at, r.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            label,
            FontId::monospace(9.0),
            theme.faint,
        );
    }
    p.text(
        r.left_top() + vec2(4.0, 3.0),
        Align2::LEFT_TOP,
        "Learned noise",
        FontId::proportional(9.5),
        theme.faint,
    );
    for (db, align, at) in [
        (top, Align2::RIGHT_TOP, r.right_top() + vec2(-4.0, 3.0)),
        (floor, Align2::RIGHT_BOTTOM, r.right_bottom() + vec2(-4.0, -14.0)),
    ] {
        p.text(
            at,
            align,
            format!("{db:.0} dB"),
            FontId::monospace(9.0),
            theme.faint,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tester's hiss: every band about -60 dBFS (stored about -27 dB
    /// per hertz on the clean-up's scale) is drawn with room above it,
    /// never pinned to the top.
    #[test]
    fn the_drawing_fits_the_learned_levels() {
        let learned = audio::dsp::cleanup::NoiseProfile([-27.5; bands::COUNT]);
        let levels = learned.band_dbfs();
        let (floor, top) = scale(&levels);
        for l in levels {
            assert!(l > floor && l < top - 5.0, "{l} in {floor}..{top}");
        }
        assert_eq!(scale(&[-3.0]).1, 0.0, "never over 0 dBFS");
        assert_eq!(scale(&[-41.0]), (-110.0, -30.0));
        assert_eq!(scale(&[]).1, -30.0, "nothing learned");
    }
}
