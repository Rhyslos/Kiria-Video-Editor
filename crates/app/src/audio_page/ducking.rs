//! The Ducking tool: which tracks are the dialogue it listens to (every
//! track whose role is Dialogue), how far and how fast this track goes
//! down under them, and a live meter of how far it is down now.

use super::Touched;
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use project::TrackRole;
use project::track_sound::Ducking;
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The tool. `keys` are the dialogue tracks' names; `role` this track's.
pub fn ui(
    ui: &mut Ui,
    theme: &Theme,
    d: &mut Ducking,
    role: TrackRole,
    keys: &[&str],
    ducked_db: Option<f32>,
) -> Touched {
    let mut touched = Touched::default();
    let full = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(
        full.min,
        vec2((full.width() * 0.4).max(150.0), (full.height() - 26.0).max(80.0)),
    );
    draw_left(ui, theme, left, d, role, keys, ducked_db);
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
    let mut on = d.on;
    if ui2.checkbox(&mut on, "Duck under dialogue").changed() {
        d.on = on;
        touched.changed = true;
    }
    let mut depth = -f64::from(d.depth_mdb) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Depth",
        &mut depth,
        Number {
            range: (0.0, 60.0),
            per_point: 0.2,
            decimals: 0,
            suffix: " dB",
            usual: 12.0,
        },
    ));
    let mut threshold = f64::from(d.threshold_mdb) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Speech over",
        &mut threshold,
        Number {
            range: (-80.0, 0.0),
            per_point: 0.2,
            decimals: 0,
            suffix: " dB",
            usual: -40.0,
        },
    ));
    let mut down = f64::from(d.down_us) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Down in",
        &mut down,
        Number {
            range: (1.0, 2_000.0),
            per_point: 1.0,
            decimals: 0,
            suffix: " ms",
            usual: 40.0,
        },
    ));
    let mut hold = f64::from(d.hold_us) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Hold",
        &mut hold,
        Number {
            range: (0.0, 2_000.0),
            per_point: 2.0,
            decimals: 0,
            suffix: " ms",
            usual: 300.0,
        },
    ));
    let mut up = f64::from(d.up_us) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Back up in",
        &mut up,
        Number {
            range: (10.0, 5_000.0),
            per_point: 5.0,
            decimals: 0,
            suffix: " ms",
            usual: 400.0,
        },
    ));
    let before = *d;
    d.depth_mdb = -(depth * 1_000.0).round() as i32;
    d.threshold_mdb = (threshold * 1_000.0).round() as i32;
    d.down_us = (down * 1_000.0).round() as u32;
    d.hold_us = (hold * 1_000.0).round() as u32;
    d.up_us = (up * 1_000.0).round() as u32;
    if touched.changed && *d != before && !d.on {
        d.on = true;
    }
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// The dialogue it listens to, and how far down it is now against the depth.
fn draw_left(
    ui: &Ui,
    theme: &Theme,
    r: Rect,
    d: &Ducking,
    role: TrackRole,
    keys: &[&str],
    ducked_db: Option<f32>,
) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let mut y = r.top() + 8.0;
    p.text(
        pos2(r.left() + 8.0, y),
        Align2::LEFT_TOP,
        "Listens to",
        FontId::proportional(11.0),
        theme.muted,
    );
    y += 16.0;
    if role == TrackRole::Dialogue {
        p.text(
            pos2(r.left() + 8.0, y),
            Align2::LEFT_TOP,
            "This track is dialogue:\nothers duck under it.",
            FontId::proportional(11.5),
            theme.text_secondary,
        );
        y += 32.0;
    }
    for name in keys.iter().take(4) {
        p.circle_filled(pos2(r.left() + 12.0, y + 7.0), 3.5, ui_kit::theme::role::DIALOGUE);
        p.text(
            pos2(r.left() + 20.0, y),
            Align2::LEFT_TOP,
            *name,
            FontId::proportional(11.5),
            theme.text,
        );
        y += 16.0;
    }
    if keys.len() > 4 {
        p.text(
            pos2(r.left() + 8.0, y),
            Align2::LEFT_TOP,
            format!("and {} more", keys.len() - 4),
            FontId::proportional(11.0),
            theme.muted,
        );
    }
    if keys.is_empty() {
        p.text(
            pos2(r.left() + 8.0, y),
            Align2::LEFT_TOP,
            "No dialogue track yet",
            FontId::proportional(11.5),
            ui_kit::status::WARNING,
        );
    }
    // The duck meter: a bar down from the top, to the depth set.
    let meter = Rect::from_min_max(
        pos2(r.right() - 34.0, r.top() + 24.0),
        pos2(r.right() - 18.0, r.bottom() - 18.0),
    );
    p.text(
        meter.center_top() - vec2(0.0, 4.0),
        Align2::CENTER_BOTTOM,
        "Duck",
        FontId::proportional(9.5),
        theme.muted,
    );
    p.rect_filled(meter, 3.0, theme.panel);
    let depth = (-d.depth_mdb as f32 / 1_000.0).max(1.0);
    let y_of = |db: f32| meter.top() + (db / depth).clamp(0.0, 1.0) * meter.height();
    p.line_segment(
        [
            pos2(meter.left() - 3.0, meter.bottom()),
            pos2(meter.right() + 3.0, meter.bottom()),
        ],
        Stroke::new(1.0, theme.muted),
    );
    let down = ducked_db.map_or(0.0, |db| (-db).max(0.0));
    if down > 0.05 {
        p.rect_filled(
            Rect::from_min_max(meter.min, pos2(meter.right(), y_of(down))),
            2.0,
            ui_kit::theme::role::MUSIC,
        );
    }
    p.text(
        meter.center_bottom() + vec2(0.0, 3.0),
        Align2::CENTER_TOP,
        format!("−{down:.0}"),
        FontId::monospace(9.5),
        theme.text_secondary,
    );
    if ducked_db.is_some() {
        ui.ctx().request_repaint();
    }
}
