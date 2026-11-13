//! The EQ tool: the response curve of the six bands and the high pass,
//! drawn over the live analyser of the track (after its EQ), with a point
//! per band to drag (sideways for its frequency, up and down for its gain,
//! the wheel over it for how narrow it is) and the high pass's corner to
//! drag sideways. The High pass lives here rather than as a tab of its
//! own: it shapes the same curve.

use super::{Live, Touched, words};
use audio::dsp::bands;
use audio::dsp::filters::{Eq as EqFilter, HighPass as HighPassFilter};
use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use project::track_sound::{BandShape, TrackSound};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number};

/// The rate the curve is worked out at.
const RATE: f32 = 48_000.0;
/// The graph's gain scale, ±dB.
pub const RANGE_DB: f32 = 24.0;
/// The analyser's scale: this many dBFS at the bottom and the top.
const FLOOR_DBFS: f32 = -96.0;
const TOP_DBFS: f32 = -6.0;
/// Points across the curve.
const STEPS: usize = 160;
/// How near a band's point the pointer must be to take it.
const GRAB: f32 = 9.0;

/// x in `r` of `freq` (a musical scale from 20 Hz to 20 kHz).
pub fn x_of(r: Rect, freq: f32) -> f32 {
    let t = (freq.max(bands::LOWEST) / bands::LOWEST).ln() / (bands::HIGHEST / bands::LOWEST).ln();
    r.left() + t.clamp(0.0, 1.0) * r.width()
}

/// The frequency at `x` in `r`, to three figures.
pub fn freq_at(r: Rect, x: f32) -> u32 {
    let t = ((x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0);
    let f = bands::LOWEST * (bands::HIGHEST / bands::LOWEST).powf(t);
    let figures = 10f32.powf((f.log10().floor() - 2.0).max(0.0));
    ((f / figures).round() * figures).clamp(20.0, 20_000.0) as u32
}

/// y in `r` of a gain.
pub fn y_of(r: Rect, db: f32) -> f32 {
    r.center().y - (db / RANGE_DB).clamp(-1.0, 1.0) * r.height() / 2.0
}

/// The gain at `y` in `r`: tenths of a decibel, 0 within a quarter.
pub fn db_at(r: Rect, y: f32) -> f32 {
    let db = ((r.center().y - y) / (r.height() / 2.0).max(1.0) * RANGE_DB).clamp(-RANGE_DB, RANGE_DB);
    if db.abs() < 0.25 {
        0.0
    } else {
        (db * 10.0).round() / 10.0
    }
}

/// The whole curve (EQ when on, plus the high pass when on) at `freq`, dB.
#[cfg(test)]
pub fn response(sound: &TrackSound, freq: f32) -> f32 {
    curve_with(&settings_of(sound), freq)
}

/// The mixer's settings for a track that sounds as `sound`.
pub fn settings_of(sound: &TrackSound) -> audio::settings::TrackSettings {
    audio::settings::TrackSettings::from_project(&project::Track {
        id: project::TrackId(0),
        kind: project::TrackKind::Audio,
        name: String::new(),
        clips: Default::default(),
        muted: false,
        solo: false,
        locked: false,
        hidden: false,
        gain_db: 0.0,
        sound: sound.clone(),
    })
}

fn curve_with(s: &audio::settings::TrackSettings, freq: f32) -> f32 {
    let mut db = 0.0;
    if s.eq.on {
        db += EqFilter::new(RATE, s.eq.bands).response_db(freq);
    }
    if s.high_pass.on {
        db += HighPassFilter::new(RATE, s.high_pass.freq).response_db(freq);
    }
    db
}

/// The tool's state between frames: the band chosen and a drag.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EqTool {
    pub band: usize,
    dragging: Option<Grip>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Band(usize),
    HighPass,
}

fn band_point(r: Rect, band: &project::track_sound::EqBand) -> Pos2 {
    pos2(
        x_of(r, band.freq_hz as f32),
        y_of(r, band.gain_mdb as f32 / 1_000.0),
    )
}

fn high_pass_point(r: Rect, freq: u32) -> Pos2 {
    pos2(x_of(r, freq as f32), y_of(r, -RANGE_DB * 0.6))
}

impl EqTool {
    /// Draws the graph and the settings; changes `sound`.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        sound: &mut TrackSound,
        live: &Live,
        name: &str,
    ) -> (Touched, String) {
        let mut touched = Touched::default();
        let full = ui.available_rect_before_wrap();
        let graph_w = (full.width() * 0.5).max(180.0);
        let graph = Rect::from_min_size(full.min, vec2(graph_w, (full.height() - 26.0).max(80.0)));
        touched = touched.and(self.graph(ui, theme, graph, sound, live));
        let side = Rect::from_min_max(
            pos2(graph.right() + 14.0, full.top()),
            pos2(full.right(), graph.bottom()),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(side)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        touched = touched.and(self.settings(&mut child, theme, sound));
        ui.allocate_rect(
            Rect::from_min_max(full.min, pos2(full.right(), graph.bottom())),
            Sense::hover(),
        );
        (touched, words::eq(name, &sound.eq, &sound.high_pass))
    }

    fn graph(&mut self, ui: &mut Ui, theme: &Theme, r: Rect, sound: &mut TrackSound, live: &Live) -> Touched {
        let p = ui.painter_at(r);
        p.rect_filled(r, 4.0, theme.bg);
        // Grid: decades and every 12 dB.
        for f in [50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0] {
            let x = x_of(r, f);
            p.line_segment(
                [pos2(x, r.top()), pos2(x, r.bottom())],
                Stroke::new(1.0, theme.line),
            );
            p.text(
                pos2(x + 2.0, r.bottom() - 2.0),
                Align2::LEFT_BOTTOM,
                words::hz(f as u32).replace(" Hz", "").replace(" kHz", "k"),
                FontId::monospace(9.0),
                theme.faint,
            );
        }
        for db in [-12.0, 0.0, 12.0] {
            let y = y_of(r, db);
            let stroke = if db == 0.0 { theme.line_strong } else { theme.line };
            p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, stroke));
            p.text(
                pos2(r.left() + 3.0, y - 1.0),
                Align2::LEFT_BOTTOM,
                format!("{db:+.0}"),
                FontId::monospace(9.0),
                theme.faint,
            );
        }
        // The analyser behind the curve.
        if let Some(levels) = &live.spectrum {
            let y_level = |dbfs: f32| {
                let t = ((dbfs - FLOOR_DBFS) / (TOP_DBFS - FLOOR_DBFS)).clamp(0.0, 1.0);
                r.bottom() - t * r.height()
            };
            let mut shape = vec![pos2(r.left(), r.bottom())];
            for (i, level) in levels.iter().enumerate() {
                shape.push(pos2(x_of(r, bands::centre(i)), y_level(*level)));
            }
            shape.push(pos2(r.right(), r.bottom()));
            let fill = theme.accent.gamma_multiply(0.18);
            // Drawn as strips, one per band, so the shape needn't be convex.
            for w in shape.windows(2) {
                let (a, b) = (w[0], w[1]);
                p.add(Shape::convex_polygon(
                    vec![a, b, pos2(b.x, r.bottom()), pos2(a.x, r.bottom())],
                    fill,
                    Stroke::NONE,
                ));
            }
            p.add(Shape::line(
                shape,
                Stroke::new(1.0, theme.accent.gamma_multiply(0.5)),
            ));
            ui.ctx().request_repaint();
        }
        // The curve.
        let settings = settings_of(sound);
        let curve: Vec<Pos2> = (0..=STEPS)
            .map(|i| {
                let x = r.left() + r.width() * i as f32 / STEPS as f32;
                let f = bands::LOWEST * (bands::HIGHEST / bands::LOWEST).powf(i as f32 / STEPS as f32);
                pos2(x, y_of(r, curve_with(&settings, f)))
            })
            .collect();
        let on = sound.eq.on || sound.high_pass.on;
        let colour = if on { theme.text } else { theme.muted };
        p.add(Shape::line(
            curve,
            Stroke::new(if on { 2.0 } else { 1.2 }, colour),
        ));
        // Points: the high pass's corner, then each band.
        let hp = high_pass_point(r, sound.high_pass.freq_hz);
        let hp_colour = if sound.high_pass.on {
            ui_kit::status::WARNING
        } else {
            theme.faint
        };
        p.circle_stroke(hp, 5.0, Stroke::new(1.5, hp_colour));
        p.text(
            hp + vec2(0.0, 9.0),
            Align2::CENTER_TOP,
            "Low cut",
            FontId::proportional(9.5),
            hp_colour,
        );
        for (i, band) in sound.eq.bands.iter().enumerate() {
            let at = band_point(r, band);
            let chosen = i == self.band;
            let fill = if !(sound.eq.on && band.on) {
                theme.faint
            } else if chosen {
                theme.accent
            } else {
                theme.text_secondary
            };
            p.circle_filled(at, if chosen { 6.0 } else { 4.5 }, fill);
            p.text(
                at + vec2(0.0, -8.0),
                Align2::CENTER_BOTTOM,
                format!("{}", i + 1),
                FontId::proportional(9.5),
                fill,
            );
        }
        // Taking hold of them.
        let response = ui.interact(r, ui.scope_id().with("eq-graph"), Sense::click_and_drag());
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let nearest = pointer.and_then(|at| {
            let mut best: Option<(Grip, f32)> = None;
            for (i, band) in sound.eq.bands.iter().enumerate() {
                let d = band_point(r, band).distance(at);
                if d <= GRAB && best.is_none_or(|(_, b)| d < b) {
                    best = Some((Grip::Band(i), d));
                }
            }
            let d = hp.distance(at);
            if d <= GRAB && best.is_none_or(|(_, b)| d < b) {
                best = Some((Grip::HighPass, d));
            }
            best.map(|(g, _)| g)
        });
        let mut touched = Touched::default();
        if nearest.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.drag_started() {
            let from = ui.input(|i| i.pointer.press_origin());
            self.dragging = from.and_then(|at| {
                sound
                    .eq
                    .bands
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (Grip::Band(i), band_point(r, b).distance(at)))
                    .chain(std::iter::once((Grip::HighPass, hp.distance(at))))
                    .filter(|(_, d)| *d <= GRAB)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(g, _)| g)
            });
        }
        if let (Some(grip), Some(at)) = (self.dragging, response.interact_pointer_pos())
            && response.dragged()
        {
            match grip {
                Grip::Band(i) => {
                    self.band = i;
                    if let Some(band) = sound.eq.bands.get_mut(i) {
                        band.freq_hz = freq_at(r, at.x);
                        band.gain_mdb = (db_at(r, at.y) * 1_000.0).round() as i32;
                        band.on = true;
                    }
                    sound.eq.on = true;
                }
                Grip::HighPass => {
                    sound.high_pass.freq_hz = freq_at(r, at.x).clamp(20, 500);
                    sound.high_pass.on = true;
                }
            }
            touched = Touched {
                changed: true,
                busy: true,
            };
        }
        if response.drag_stopped() {
            self.dragging = None;
        }
        if response.clicked()
            && let Some(Grip::Band(i)) = nearest
        {
            self.band = i;
        }
        if response.double_clicked()
            && let Some(Grip::Band(i)) = nearest
            && let Some(band) = sound.eq.bands.get_mut(i)
        {
            band.gain_mdb = 0;
            touched.changed = true;
        }
        // The wheel over a peak: how narrow it is.
        if let Some(Grip::Band(i)) = nearest
            && let Some(band) = sound.eq.bands.get_mut(i)
            && band.shape == BandShape::Peak
        {
            let wheel = ui.input(|inp| inp.smooth_scroll_delta.y);
            if wheel.abs() > 0.0 {
                let q = (band.q_milli as f32 * (1.0 + wheel / 400.0)).clamp(100.0, 20_000.0);
                band.q_milli = q.round() as u32;
                touched = Touched {
                    changed: true,
                    busy: true,
                };
            }
        }
        response.on_hover_text(
            "Drag a numbered point: sideways for its frequency, up or down to lift or cut. The wheel over a point makes it narrower or wider; double-click puts it back to flat. Drag Low cut sideways to take out rumble below it. The shaded shape is the track's sound now.",
        );
        touched
    }

    fn settings(&mut self, ui: &mut Ui, theme: &Theme, sound: &mut TrackSound) -> Touched {
        let mut touched = Touched::default();
        let mut on = sound.eq.on;
        if ui.checkbox(&mut on, "EQ on").changed() {
            sound.eq.on = on;
            touched.changed = true;
        }
        // The band chosen.
        ui.horizontal(|ui| {
            for i in 0..sound.eq.bands.len() {
                if ui
                    .selectable_label(i == self.band, format!("{}", i + 1))
                    .clicked()
                {
                    self.band = i;
                }
            }
        });
        let band_i = self.band.min(sound.eq.bands.len() - 1);
        if let Some(band) = sound.eq.bands.get_mut(band_i) {
            let kind = match band.shape {
                BandShape::LowShelf => "Low shelf",
                BandShape::Peak => "Peak",
                BandShape::HighShelf => "High shelf",
            };
            let mut band_on = band.on;
            if ui
                .checkbox(&mut band_on, format!("Band {} · {kind}", band_i + 1))
                .changed()
            {
                band.on = band_on;
                touched.changed = true;
            }
            let mut freq = f64::from(band.freq_hz);
            touched = touched.and(inspector::slider(
                ui,
                theme,
                "Frequency",
                &mut freq,
                Number {
                    range: (20.0, 20_000.0),
                    per_point: 10.0,
                    decimals: 0,
                    suffix: " Hz",
                    usual: f64::from(project::track_sound::Eq::default().bands[band_i].freq_hz),
                },
            ));
            band.freq_hz = freq.round() as u32;
            let mut gain = f64::from(band.gain_mdb) / 1_000.0;
            touched = touched.and(inspector::slider(
                ui,
                theme,
                "Gain",
                &mut gain,
                Number {
                    range: (-24.0, 24.0),
                    per_point: 0.1,
                    decimals: 1,
                    suffix: " dB",
                    usual: 0.0,
                },
            ));
            band.gain_mdb = (gain * 1_000.0).round() as i32;
            if band.shape == BandShape::Peak {
                let mut q = f64::from(band.q_milli) / 1_000.0;
                touched = touched.and(inspector::slider(
                    ui,
                    theme,
                    "Width (Q)",
                    &mut q,
                    Number {
                        range: (0.1, 20.0),
                        per_point: 0.02,
                        decimals: 2,
                        suffix: "",
                        usual: 1.0,
                    },
                ));
                band.q_milli = (q * 1_000.0).round() as u32;
            }
            if touched.changed && band.gain_mdb != 0 {
                sound.eq.on = true;
            }
        }
        ui.add_space(4.0);
        let mut hp_on = sound.high_pass.on;
        if ui.checkbox(&mut hp_on, "Low cut (high pass)").changed() {
            sound.high_pass.on = hp_on;
            touched.changed = true;
        }
        let mut hp = f64::from(sound.high_pass.freq_hz);
        let hp_touched = inspector::slider(
            ui,
            theme,
            "Below",
            &mut hp,
            Number {
                range: (20.0, 500.0),
                per_point: 1.0,
                decimals: 0,
                suffix: " Hz",
                usual: 80.0,
            },
        );
        if hp_touched.changed {
            sound.high_pass.on = true;
        }
        sound.high_pass.freq_hz = hp.round() as u32;
        touched.and(hp_touched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_maps_frequency_and_gain_both_ways() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        for f in [20, 100, 250, 1_000, 3_000, 20_000] {
            let back = freq_at(r, x_of(r, f as f32));
            assert!(
                (back as f32 / f as f32 - 1.0).abs() < 0.02,
                "{f} came back {back}"
            );
        }
        assert_eq!(db_at(r, y_of(r, -6.0)), -6.0);
        assert_eq!(db_at(r, y_of(r, 0.1)), 0.0, "0 dB is easy to find");
        assert_eq!(freq_at(r, x_of(r, 1_234.0)), 1_230, "three figures");
    }

    #[test]
    fn the_curve_is_flat_until_a_band_or_the_high_pass_does_something() {
        let mut sound = TrackSound::default();
        assert_eq!(response(&sound, 1_000.0), 0.0);
        sound.eq.on = true;
        sound.eq.bands[2].gain_mdb = 6_000;
        assert!((response(&sound, 1_000.0) - 6.0).abs() < 0.2);
        sound.high_pass.on = true;
        assert!(response(&sound, 30.0) < -12.0, "rumble is cut");
    }
}
