//! The Distortion, Echo, Reverb and Pitch tools: each a picture of what it does
//! on the left and its settings on the right, the same layout as the
//! Loudness and Ducking tools. Moving a setting turns the effect on.

use super::Touched;
use egui::{Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use project::track_sound::{Distortion, DistortionStyle, Echo, Pitch, Reverb};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The left picture and the right column of a tool.
fn split(ui: &mut Ui) -> (Rect, Rect, Rect) {
    let full = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(
        full.min,
        vec2((full.width() * 0.4).max(150.0), (full.height() - 26.0).max(80.0)),
    );
    let side = Rect::from_min_max(
        pos2(left.right() + 16.0, full.top()),
        pos2(full.right(), left.bottom()),
    );
    (full, left, side)
}

fn percent(usual: f64) -> Number {
    Number {
        range: (0.0, 100.0),
        per_point: 0.5,
        decimals: 0,
        suffix: " %",
        usual,
    }
}

/// The Distortion tool.
pub fn distortion(ui: &mut Ui, theme: &Theme, d: &mut Distortion) -> Touched {
    let mut touched = Touched::default();
    let (full, left, side) = split(ui);
    draw_curve(ui, theme, left, d);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = d.on;
    if ui2.checkbox(&mut on, "Distort it").changed() {
        d.on = on;
        touched.changed = true;
    }
    let mut style = d.style;
    let styles = [
        (DistortionStyle::Drive, "Drive"),
        (DistortionStyle::Radio, "Radio"),
        (DistortionStyle::Phone, "Phone"),
    ];
    if ui_kit::settings::segmented(ui2, theme, &mut style, &styles).changed() {
        d.style = style;
        d.on = true;
        touched.changed = true;
    }
    ui2.add_space(6.0);
    let mut drive = f64::from(d.drive_mdb) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Drive",
        &mut drive,
        Number {
            range: (0.0, 30.0),
            per_point: 0.1,
            decimals: 1,
            suffix: " dB",
            usual: 12.0,
        },
    ));
    let mut mix = f64::from(d.mix_milli) / 10.0;
    touched = touched.and(inspector::slider(ui2, theme, "Mix", &mut mix, percent(100.0)));
    let before = *d;
    d.drive_mdb = (drive * 1_000.0).round() as u32;
    d.mix_milli = (mix * 10.0).round() as u32;
    turn_on_when_moved(&mut touched, before != *d, &mut d.on);
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// The Echo tool.
pub fn echo(ui: &mut Ui, theme: &Theme, e: &mut Echo) -> Touched {
    let mut touched = Touched::default();
    let (full, left, side) = split(ui);
    draw_repeats(ui, theme, left, e);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = e.on;
    if ui2.checkbox(&mut on, "Echo").changed() {
        e.on = on;
        touched.changed = true;
    }
    let mut time = f64::from(e.time_us) / 1_000.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Time",
        &mut time,
        Number {
            range: (20.0, 1_500.0),
            per_point: 2.0,
            decimals: 0,
            suffix: " ms",
            usual: 350.0,
        },
    ));
    let mut feedback = f64::from(e.feedback_milli) / 10.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Repeats",
        &mut feedback,
        Number {
            range: (0.0, 90.0),
            ..percent(35.0)
        },
    ));
    let mut mix = f64::from(e.mix_milli) / 10.0;
    touched = touched.and(inspector::slider(ui2, theme, "Level", &mut mix, percent(35.0)));
    let mut ping_pong = e.ping_pong;
    if ui2.checkbox(&mut ping_pong, "Left and right in turn").changed() {
        e.ping_pong = ping_pong;
        e.on = true;
        touched.changed = true;
    }
    let before = *e;
    e.time_us = (time * 1_000.0).round() as u32;
    e.feedback_milli = (feedback * 10.0).round() as u32;
    e.mix_milli = (mix * 10.0).round() as u32;
    turn_on_when_moved(&mut touched, before != *e, &mut e.on);
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// The Reverb tool.
pub fn reverb(ui: &mut Ui, theme: &Theme, r: &mut Reverb) -> Touched {
    let mut touched = Touched::default();
    let (full, left, side) = split(ui);
    draw_room(ui, theme, left, r);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = r.on;
    if ui2.checkbox(&mut on, "Put it in a room").changed() {
        r.on = on;
        touched.changed = true;
    }
    let mut size = f64::from(r.size_milli) / 10.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Room size",
        &mut size,
        percent(50.0),
    ));
    let mut damping = f64::from(r.damping_milli) / 10.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Damping",
        &mut damping,
        percent(50.0),
    ));
    let mut mix = f64::from(r.mix_milli) / 10.0;
    touched = touched.and(inspector::slider(ui2, theme, "Level", &mut mix, percent(25.0)));
    let before = *r;
    r.size_milli = (size * 10.0).round() as u32;
    r.damping_milli = (damping * 10.0).round() as u32;
    r.mix_milli = (mix * 10.0).round() as u32;
    turn_on_when_moved(&mut touched, before != *r, &mut r.on);
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// The Pitch tool.
pub fn pitch(ui: &mut Ui, theme: &Theme, p: &mut Pitch) -> Touched {
    let mut touched = Touched::default();
    let (full, left, side) = split(ui);
    draw_pitch(ui, theme, left, p);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(side)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui2 = &mut child;
    let mut on = p.on;
    if ui2.checkbox(&mut on, "Shift the pitch").changed() {
        p.on = on;
        touched.changed = true;
    }
    let mut semitones = f64::from(p.cents) / 100.0;
    touched = touched.and(inspector::slider(
        ui2,
        theme,
        "Semitones",
        &mut semitones,
        Number {
            range: (-12.0, 12.0),
            per_point: 0.05,
            decimals: 1,
            suffix: "",
            usual: 0.0,
        },
    ));
    // Steps a voice is often moved by: a deep voice, a high one.
    ui2.horizontal(|ui| {
        for (label, cents) in [
            ("-1 octave", -1_200),
            ("-5", -500),
            ("0", 0),
            ("+5", 500),
            ("+1 octave", 1_200),
        ] {
            if ui.small_button(label).clicked() {
                semitones = f64::from(cents) / 100.0;
                touched.changed = true;
            }
        }
    });
    let mut mix = f64::from(p.mix_milli) / 10.0;
    touched = touched.and(inspector::slider(ui2, theme, "Mix", &mut mix, percent(100.0)));
    let before = *p;
    p.cents = (semitones * 100.0).round() as i32;
    p.mix_milli = (mix * 10.0).round() as u32;
    turn_on_when_moved(&mut touched, before != *p, &mut p.on);
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), left.bottom())),
        Sense::hover(),
    );
    touched
}

/// A setting moved while the effect was off turns it on, as the other
/// tools do.
fn turn_on_when_moved(touched: &mut Touched, moved: bool, on: &mut bool) {
    if touched.changed && moved && !*on {
        *on = true;
    }
}

/// Distortion: how a level going in comes out, the soft clip's curve.
fn draw_curve(ui: &Ui, theme: &Theme, r: Rect, d: &Distortion) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let box_ = r.shrink(14.0);
    p.line_segment(
        [box_.left_bottom(), box_.right_top()],
        Stroke::new(1.0, theme.line),
    );
    let gain = 10f32.powf(d.drive_mdb as f32 / 20_000.0);
    let mix = d.mix_milli as f32 / 1_000.0;
    let points: Vec<_> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0 * 2.0 - 1.0;
            let wet = (x * gain).tanh() / gain.tanh().max(1e-6);
            let y = x + (wet - x) * mix;
            pos2(
                box_.center().x + x * box_.width() / 2.0,
                box_.center().y - y.clamp(-1.0, 1.0) * box_.height() / 2.0,
            )
        })
        .collect();
    let colour = if d.on { theme.accent } else { theme.muted };
    p.add(Shape::line(points, Stroke::new(2.0, colour)));
}

/// Echo: the sound and its repeats, each shorter by the feedback.
fn draw_repeats(ui: &Ui, theme: &Theme, r: Rect, e: &Echo) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let base = r.bottom() - 14.0;
    let tall = r.height() - 28.0;
    let colour = if e.on { theme.accent } else { theme.muted };
    // Two seconds across.
    let per_second = (r.width() - 28.0) / 2.0;
    let gap = e.time_us as f32 / 1_000_000.0 * per_second;
    let mut level = 1.0;
    let mut x = r.left() + 14.0;
    let mut first = true;
    while x < r.right() - 14.0 && level > 0.02 {
        let height = tall * level * if first { 1.0 } else { e.mix_milli as f32 / 1_000.0 };
        let bar = Rect::from_min_max(pos2(x, base - height), pos2(x + 4.0, base));
        p.rect_filled(bar, 1.0, if first { theme.text } else { colour });
        if !first {
            level *= e.feedback_milli as f32 / 1_000.0;
        }
        first = false;
        x += gap.max(4.0);
    }
}

/// Pitch: a wave as it came, faint, and the same wave shifted, its
/// cycles closer together higher and further apart lower.
fn draw_pitch(ui: &Ui, theme: &Theme, r: Rect, p: &Pitch) {
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 4.0, theme.bg);
    let inner = r.shrink(14.0);
    let wave = |cycles: f32| -> Vec<egui::Pos2> {
        (0..=128)
            .map(|i| {
                let t = i as f32 / 128.0;
                let y = (t * cycles * std::f32::consts::TAU).sin();
                pos2(
                    inner.left() + t * inner.width(),
                    inner.center().y - y * inner.height() * 0.4,
                )
            })
            .collect()
    };
    painter.add(Shape::line(wave(3.0), Stroke::new(1.0, theme.line)));
    let shifted = if p.on {
        2f32.powf(p.cents as f32 / 1_200.0)
    } else {
        1.0
    };
    let colour = if p.on { theme.accent } else { theme.muted };
    painter.add(Shape::line(wave(3.0 * shifted), Stroke::new(2.0, colour)));
}

/// Reverb: the room's sound dying away, longer for a bigger room, its
/// colour duller with damping.
fn draw_room(ui: &Ui, theme: &Theme, r: Rect, rv: &Reverb) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    let base = r.bottom() - 14.0;
    let tall = (r.height() - 28.0) * (0.3 + 0.7 * rv.mix_milli as f32 / 1_000.0);
    let feedback = 0.7 + 0.28 * rv.size_milli as f32 / 1_000.0;
    // How long it takes to die away 60 dB: about 30 ms round each loop,
    // each round `feedback` of the last.
    let seconds = (0.03 * 6.9 / -feedback.ln()).clamp(0.2, 6.0);
    let colour = if rv.on { theme.accent } else { theme.muted };
    let width = r.width() - 28.0;
    let points: Vec<_> = (0..=64)
        .map(|i| {
            let t = i as f32 / 64.0;
            // Three seconds across.
            let y = (-6.9 * t * 3.0 / seconds).exp();
            pos2(r.left() + 14.0 + t * width, base - tall * y)
        })
        .collect();
    let shade = colour.gamma_multiply(0.25 + 0.5 * (1.0 - rv.damping_milli as f32 / 1_000.0));
    for pair in points.windows(2) {
        let under = Rect::from_min_max(pos2(pair[0].x, pair[0].y.min(pair[1].y)), pos2(pair[1].x, base));
        p.rect_filled(under, 0.0, shade);
    }
    p.add(Shape::line(points, Stroke::new(2.0, colour)));
}
