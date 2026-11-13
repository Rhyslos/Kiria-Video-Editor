//! The Loudness tool: the loudness the track is kept near, how far it may
//! move it, and its live gain against those limits.

use super::Touched;
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use project::track_sound::Loudness;
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The tool. `gain_db` is what it is doing now (while playing),
/// `short_term` the track's loudness over 3 s.
pub fn ui(
    ui: &mut Ui,
    theme: &Theme,
    l: &mut Loudness,
    gain_db: Option<f32>,
    short_term: Option<f32>,
) -> Touched {
    let mut touched = Touched::default();
    let full = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(
        full.min,
        vec2((full.width() * 0.4).max(150.0), (full.height() - 26.0).max(80.0)),
    );
    draw_gain(ui, theme, left, l, gain_db, short_term);
    let side = Rect::from_min_max(
        pos2(left.right() + 16.0, full.top()),
        pos2(full.right(), left.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = l.on;
    if ui2.checkbox(&mut on, "Keep it near a loudness").changed() {
        l.on = on;
        touched.changed = true;
    }
    let mut target = f64::from(l.target_mlufs) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Aim (LUFS)",
        &mut target,
        Number {
            range: (-40.0, -5.0),
            per_point: 0.1,
            decimals: 1,
            suffix: "",
            usual: -18.0,
        },
    ));
    let mut boost = f64::from(l.max_boost_mdb) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Lift up to",
        &mut boost,
        Number {
            range: (0.0, 24.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 12.0,
        },
    ));
    let mut cut = f64::from(l.max_cut_mdb) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Lower up to",
        &mut cut,
        Number {
            range: (0.0, 24.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 12.0,
        },
    ));
    let before = *l;
    l.target_mlufs = (target * 1_000.0).round() as i32;
    l.max_boost_mdb = (boost * 1_000.0).round() as u32;
    l.max_cut_mdb = (cut * 1_000.0).round() as u32;
    if touched.changed && *l != before && !l.on {
        l.on = true;
    }
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// A centre-zero bar of the gain now between the most it may lower and
/// lift, and the track's loudness against the aim.
fn draw_gain(ui: &Ui, theme: &Theme, r: Rect, l: &Loudness, gain_db: Option<f32>, short_term: Option<f32>) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let bar = Rect::from_min_max(
        pos2(r.left() + 14.0, r.center().y - 7.0),
        pos2(r.right() - 14.0, r.center().y + 7.0),
    );
    let (cut, boost) = (l.max_cut_mdb as f32 / 1_000.0, l.max_boost_mdb as f32 / 1_000.0);
    let span = cut.max(boost).max(1.0);
    let x_of = |db: f32| bar.center().x + (db / span).clamp(-1.0, 1.0) * bar.width() / 2.0;
    p.rect_filled(bar, 3.0, theme.panel);
    p.rect_filled(
        Rect::from_min_max(pos2(x_of(-cut), bar.top()), pos2(x_of(boost), bar.bottom())),
        3.0,
        theme.raised,
    );
    p.line_segment(
        [
            pos2(bar.center().x, bar.top() - 4.0),
            pos2(bar.center().x, bar.bottom() + 4.0),
        ],
        Stroke::new(1.0, theme.muted),
    );
    if let Some(g) = gain_db.filter(|_| l.on) {
        let (a, b) = (x_of(0.0), x_of(g));
        p.rect_filled(
            Rect::from_min_max(
                pos2(a.min(b), bar.top() + 2.0),
                pos2(a.max(b), bar.bottom() - 2.0),
            ),
            2.0,
            theme.accent,
        );
        ui.ctx().request_repaint();
    }
    let said = match gain_db.filter(|_| l.on) {
        Some(g) => format!("{g:+.1} dB now"),
        None => "Gain now".to_owned(),
    };
    p.text(
        bar.center_top() - vec2(0.0, 8.0),
        Align2::CENTER_BOTTOM,
        said,
        FontId::proportional(11.5),
        theme.text,
    );
    p.text(
        bar.left_bottom() + vec2(0.0, 4.0),
        Align2::LEFT_TOP,
        format!("−{cut:.0}"),
        FontId::monospace(9.5),
        theme.faint,
    );
    p.text(
        bar.right_bottom() + vec2(0.0, 4.0),
        Align2::RIGHT_TOP,
        format!("+{boost:.0}"),
        FontId::monospace(9.5),
        theme.faint,
    );
    let now = short_term
        .filter(|v| v.is_finite())
        .map_or("–".to_owned(), |v| format!("{v:.1}"));
    p.text(
        r.center_bottom() - vec2(0.0, 10.0),
        Align2::CENTER_BOTTOM,
        format!(
            "Now {now} LUFS · aiming for {:.1}",
            l.target_mlufs as f32 / 1_000.0
        ),
        FontId::proportional(11.0),
        theme.text_secondary,
    );
}
