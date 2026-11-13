//! The Compressor tool: its gain curve (level in, level out, the
//! threshold dashed), a meter of how far it turns the sound down now,
//! the plain presets and every setting. The master's compressor in the
//! mixer uses the same presets ([`preset`], [`which_preset`]).

use super::Touched;
use audio::dsp::dynamics::Preset;
use egui::{Align2, FontId, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use project::track_sound::Compressor;
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The graph's scale: -60 dB to 0 in and out.
const BOTTOM: f32 = -60.0;
/// The meter's scale: 0 to this much turned down.
const METER_DB: f32 = 20.0;

/// A preset as the project keeps a compressor, on.
pub fn preset(p: Preset) -> Compressor {
    let s = p.settings();
    Compressor {
        on: true,
        threshold_mdb: (s.threshold_db * 1_000.0).round() as i32,
        ratio_milli: (s.ratio * 1_000.0).round() as u32,
        attack_us: (s.attack_ms * 1_000.0).round() as u32,
        release_us: (s.release_ms * 1_000.0).round() as u32,
        knee_mdb: (s.knee_db * 1_000.0).round() as u32,
        makeup_mdb: (s.makeup_db * 1_000.0).round() as i32,
    }
}

/// The preset a compressor is set to, if it is one.
pub fn which_preset(c: &Compressor) -> Option<Preset> {
    Preset::ALL.into_iter().find(|p| {
        let mut want = preset(*p);
        want.on = c.on;
        want == *c
    })
}

/// What a preset is for, in a few words.
pub fn preset_words(p: Preset) -> &'static str {
    match p {
        Preset::Gentle => "Gentle: takes the edge off the loudest moments",
        Preset::Voice => "Voice: keeps a voice even",
        Preset::Music => "Music: holds music or game sound together",
        Preset::Strong => "Strong: everything at about one level",
    }
}

/// The preset dropdown; returns whether one was picked (and sets it).
pub fn preset_menu(ui: &mut Ui, id: &str, c: &mut Compressor, width: f32) -> bool {
    let shown = which_preset(c).map_or("Your own settings", |p| p.name());
    let mut picked = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .show_ui(ui, |ui| {
            for p in Preset::ALL {
                if ui
                    .selectable_label(which_preset(c) == Some(p), preset_words(p))
                    .clicked()
                {
                    *c = preset(p);
                    picked = true;
                }
            }
        });
    picked
}

/// The level out for a level in (dB), before makeup.
fn curve(c: &Compressor, level: f32) -> f32 {
    let s = audio::dsp::dynamics::CompressorSettings {
        on: true,
        threshold_db: c.threshold_mdb as f32 / 1_000.0,
        ratio: c.ratio_milli as f32 / 1_000.0,
        attack_ms: 1.0,
        release_ms: 1.0,
        knee_db: c.knee_mdb as f32 / 1_000.0,
        makeup_db: 0.0,
    };
    level + s.curve(level)
}

/// The Compressor tool for a track: graph, meter and settings.
pub fn ui(ui: &mut Ui, theme: &Theme, c: &mut Compressor, reducing_db: Option<f32>) -> Touched {
    let full = ui.available_rect_before_wrap();
    let size = (full.height() - 26.0).min(full.width() * 0.42).max(80.0);
    let graph = Rect::from_min_size(full.min, vec2(size, size));
    draw_curve(ui, theme, graph, c);
    let meter = Rect::from_min_size(
        pos2(graph.right() + 10.0, graph.top() + 14.0),
        vec2(14.0, size - 30.0),
    );
    draw_meter(ui, theme, meter, reducing_db);
    let side = Rect::from_min_max(
        pos2(meter.right() + 16.0, full.top()),
        pos2(full.right(), graph.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let touched = settings(&mut child, theme, c, "track-compressor-preset");
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), graph.bottom())),
        Sense::hover(),
    );
    touched
}

fn draw_curve(ui: &Ui, theme: &Theme, r: Rect, c: &Compressor) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let to = |level_in: f32, level_out: f32| {
        let x = r.left() + (level_in - BOTTOM) / -BOTTOM * r.width();
        let y = r.bottom() - (level_out - BOTTOM) / -BOTTOM * r.height();
        pos2(x, y.clamp(r.top(), r.bottom()))
    };
    for db in [-48.0, -36.0, -24.0, -12.0] {
        p.line_segment([to(db, BOTTOM), to(db, 0.0)], Stroke::new(1.0, theme.line));
        p.line_segment([to(BOTTOM, db), to(0.0, db)], Stroke::new(1.0, theme.line));
    }
    p.add(Shape::dashed_line(
        &[to(BOTTOM, BOTTOM), to(0.0, 0.0)],
        Stroke::new(1.0, theme.line_strong),
        3.0,
        3.0,
    ));
    let threshold = c.threshold_mdb as f32 / 1_000.0;
    p.add(Shape::dashed_line(
        &[to(threshold, BOTTOM), to(threshold, 0.0)],
        Stroke::new(1.0, theme.muted),
        2.0,
        3.0,
    ));
    let points: Vec<_> = (0..=60)
        .map(|i| {
            let level = BOTTOM + i as f32;
            to(level, curve(c, level))
        })
        .collect();
    let colour = if c.on { theme.text } else { theme.muted };
    p.add(Shape::line(
        points,
        Stroke::new(if c.on { 2.0 } else { 1.2 }, colour),
    ));
    p.text(
        r.left_bottom() + vec2(4.0, -3.0),
        Align2::LEFT_BOTTOM,
        "in →",
        FontId::proportional(9.5),
        theme.faint,
    );
    p.text(
        r.left_top() + vec2(4.0, 3.0),
        Align2::LEFT_TOP,
        "out",
        FontId::proportional(9.5),
        theme.faint,
    );
}

/// A meter of how far the sound is turned down now ("GR").
pub fn draw_meter(ui: &Ui, theme: &Theme, r: Rect, reducing_db: Option<f32>) {
    let p = ui.painter_at(r.expand2(vec2(10.0, 14.0)));
    p.text(
        r.center_top() - vec2(0.0, 2.0),
        Align2::CENTER_BOTTOM,
        "GR",
        FontId::proportional(9.5),
        theme.muted,
    );
    p.rect_filled(r, 3.0, theme.bg);
    let down = reducing_db.map_or(0.0, |d| (-d).clamp(0.0, METER_DB));
    if down > 0.05 {
        let bottom = r.top() + down / METER_DB * r.height();
        p.rect_filled(
            Rect::from_min_max(r.min, pos2(r.right(), bottom)),
            2.0,
            ui_kit::status::WARNING,
        );
    }
    p.text(
        r.center_bottom() + vec2(0.0, 3.0),
        Align2::CENTER_TOP,
        if down > 0.05 {
            format!("−{down:.1}")
        } else {
            "0".into()
        },
        FontId::monospace(9.5),
        theme.text_secondary,
    );
    if reducing_db.is_some() {
        ui.ctx().request_repaint();
    }
}

/// On, the preset and every number.
pub fn settings(ui: &mut Ui, theme: &Theme, c: &mut Compressor, id: &str) -> Touched {
    let mut touched = Touched::default();
    ui.horizontal(|ui| {
        let mut on = c.on;
        if ui.checkbox(&mut on, "On").changed() {
            c.on = on;
            touched.changed = true;
        }
        if preset_menu(ui, id, c, ui.available_width() - 8.0) {
            touched.changed = true;
        }
    });
    let mut row = |ui: &mut Ui, label: &str, value: &mut f64, how: Number| {
        let t = inspector::slider(ui, theme, label, value, how);
        touched = touched.and(t);
    };
    let mut threshold = f64::from(c.threshold_mdb) / 1_000.0;
    row(
        ui,
        "Threshold",
        &mut threshold,
        Number {
            range: (-60.0, 0.0),
            per_point: 0.2,
            decimals: 1,
            suffix: " dB",
            usual: -24.0,
        },
    );
    let mut ratio = f64::from(c.ratio_milli) / 1_000.0;
    row(
        ui,
        "Ratio",
        &mut ratio,
        Number {
            range: (1.0, 20.0),
            per_point: 0.05,
            decimals: 1,
            suffix: " : 1",
            usual: 3.0,
        },
    );
    let mut attack = f64::from(c.attack_us) / 1_000.0;
    row(
        ui,
        "Attack",
        &mut attack,
        Number {
            range: (0.1, 200.0),
            per_point: 0.2,
            decimals: 1,
            suffix: " ms",
            usual: 5.0,
        },
    );
    let mut release = f64::from(c.release_us) / 1_000.0;
    row(
        ui,
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
    let mut knee = f64::from(c.knee_mdb) / 1_000.0;
    row(
        ui,
        "Knee",
        &mut knee,
        Number {
            range: (0.0, 24.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 6.0,
        },
    );
    let mut makeup = f64::from(c.makeup_mdb) / 1_000.0;
    row(
        ui,
        "Makeup",
        &mut makeup,
        Number {
            range: (0.0, 24.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 4.0,
        },
    );
    let before = *c;
    c.threshold_mdb = (threshold * 1_000.0).round() as i32;
    c.ratio_milli = (ratio * 1_000.0).round() as u32;
    c.attack_us = (attack * 1_000.0).round() as u32;
    c.release_us = (release * 1_000.0).round() as u32;
    c.knee_mdb = (knee * 1_000.0).round() as u32;
    c.makeup_mdb = (makeup * 1_000.0).round() as i32;
    // A number moved on a compressor that was off turns it on.
    if touched.changed && *c != before && !c.on {
        c.on = true;
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_and_are_told_apart() {
        for p in Preset::ALL {
            let c = preset(p);
            assert!(c.on);
            assert_eq!(which_preset(&c), Some(p));
            let mut off = c;
            off.on = false;
            assert_eq!(which_preset(&off), Some(p), "the same preset, turned off");
        }
        let mut own = preset(Preset::Voice);
        own.ratio_milli += 500;
        assert_eq!(which_preset(&own), None);
        // The project's default is the Voice preset's numbers, off.
        assert_eq!(which_preset(&Compressor::default()), Some(Preset::Voice));
    }

    #[test]
    fn the_curve_bends_at_the_threshold() {
        let c = preset(Preset::Voice);
        assert!((curve(&c, -40.0) + 40.0).abs() < 1e-3, "under: as it is");
        // 12 dB over at 3:1 comes out 4 dB over.
        assert!((curve(&c, -12.0) + 20.0).abs() < 0.05, "{}", curve(&c, -12.0));
    }
}
