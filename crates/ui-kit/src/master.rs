//! The master column beside the timeline: the meter of the whole mix,
//! which is also its volume fader, and the loudness under it. The same on
//! every page that shows it.
//!
//! The meter has three lanes on one decibel scale: the left and right
//! channels' sample peaks (green, yellow from -6 dB, red over 0 dB) with a
//! line at each channel's true peak (the level between samples too; red
//! over [`TRUE_PEAK_CEILING`]), and the short-term loudness (LUFS) with a
//! line at the loudness aimed for ([`LoudnessTarget`], chosen in the
//! column's right-click menu). Under it: the volume, the momentary (M),
//! short-term (S) and integrated (I) loudness and the highest true peak
//! (TP). I and TP count from when playback started; a click on them starts
//! them again, and they stay readable after playback stops.

use crate::Theme;
use egui::{Align2, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

/// The scale shared by the meter and the fader, in dB.
const TOP_DB: f32 = 12.0;
const BOTTOM_DB: f32 = -60.0;
/// How far drawn levels fall each frame (about 20 dB a second at 60
/// frames a second), so they can be read.
const FALL_DB: f32 = 0.35;
/// The highest true peak that is safe for files played or encoded again.
pub const TRUE_PEAK_CEILING: f32 = -1.0;
/// Within this many LU of the target, the integrated loudness is on it.
const ON_TARGET_LU: f32 = 1.0;

/// The loudness a mix is aimed at: a line on the meter, and the colour of
/// the integrated loudness. Kept in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoudnessTarget {
    Off,
    /// -14 LUFS: video sites and streaming.
    #[default]
    Online,
    /// -16 LUFS: podcasts and spoken word on phones.
    Spoken,
    /// -23 LUFS: European broadcast.
    Broadcast,
    /// -24 LUFS: North American broadcast.
    BroadcastNorthAmerica,
}

impl LoudnessTarget {
    pub const ALL: [LoudnessTarget; 5] = [
        LoudnessTarget::Online,
        LoudnessTarget::Spoken,
        LoudnessTarget::Broadcast,
        LoudnessTarget::BroadcastNorthAmerica,
        LoudnessTarget::Off,
    ];

    /// The loudness aimed at, LUFS; `None` for no target.
    pub fn lufs(self) -> Option<f32> {
        match self {
            LoudnessTarget::Off => None,
            LoudnessTarget::Online => Some(-14.0),
            LoudnessTarget::Spoken => Some(-16.0),
            LoudnessTarget::Broadcast => Some(-23.0),
            LoudnessTarget::BroadcastNorthAmerica => Some(-24.0),
        }
    }

    /// As the menu lists it.
    pub fn label(self) -> &'static str {
        match self {
            LoudnessTarget::Off => "No target",
            LoudnessTarget::Online => "-14 LUFS: online video",
            LoudnessTarget::Spoken => "-16 LUFS: podcasts, spoken word",
            LoudnessTarget::Broadcast => "-23 LUFS: broadcast (Europe)",
            LoudnessTarget::BroadcastNorthAmerica => "-24 LUFS: broadcast (North America)",
        }
    }
}

/// How an integrated loudness stands against a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgainstTarget {
    /// No target, or nothing measured yet.
    Unknown,
    Quieter,
    OnIt,
    Louder,
}

/// Where `integrated` (LUFS) stands against `target`.
pub fn against(integrated: f32, target: LoudnessTarget) -> AgainstTarget {
    match target.lufs() {
        Some(aim) if integrated.is_finite() => {
            if integrated > aim + ON_TARGET_LU {
                AgainstTarget::Louder
            } else if integrated < aim - ON_TARGET_LU {
                AgainstTarget::Quieter
            } else {
                AgainstTarget::OnIt
            }
        }
        _ => AgainstTarget::Unknown,
    }
}

/// What the meter shows, as the engine measured it. Silence is `-inf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterLevels {
    /// Each channel's highest sample in the last block, dBFS.
    pub peak_db: [f32; 2],
    /// Each channel's highest true peak in the last block, dBTP.
    pub true_peak_db: [f32; 2],
    /// The highest true peak since playback (or the last reset) began, dBTP.
    pub true_peak_max: f32,
    /// Loudness over 0.4 s, 3 s, and integrated since playback (or the
    /// last reset) began, LUFS.
    pub momentary: f32,
    pub short_term: f32,
    pub integrated: f32,
}

/// What the column shows of the mix this frame.
#[derive(Debug, Clone, Copy)]
pub struct Mix<'a> {
    /// `None` when nothing plays.
    pub levels: Option<MeterLevels>,
    /// The mix's volume, dB.
    pub gain_db: f32,
    pub target: LoudnessTarget,
    /// What the meter says when pointed at.
    pub tip: &'a str,
}

/// What a frame of the column asks of the page.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MasterResponse {
    /// The new volume, when the fader is let go (or double-clicked back to 0 dB).
    pub gain_db: Option<f32>,
    /// Start the integrated loudness and the highest true peak again.
    pub reset: bool,
    /// Another loudness target was chosen.
    pub target: Option<LoudnessTarget>,
}

/// Kept by the page between frames.
#[derive(Debug, Clone)]
pub struct MasterColumn {
    /// The fader while it is being dragged.
    drag: Option<f32>,
    /// Peaks, true peaks and short-term loudness drawn, falling slowly.
    shown: [f32; 2],
    shown_true: [f32; 2],
    shown_loudness: f32,
    /// The integrated loudness and highest true peak last measured, still
    /// shown after playback stops.
    held: (f32, f32),
}

impl Default for MasterColumn {
    fn default() -> Self {
        MasterColumn {
            drag: None,
            shown: [BOTTOM_DB; 2],
            shown_true: [BOTTOM_DB; 2],
            shown_loudness: BOTTOM_DB,
            held: (f32::NEG_INFINITY, f32::NEG_INFINITY),
        }
    }
}

/// y on the scale for `db` within `bar`.
fn y_of(bar: Rect, db: f32) -> f32 {
    let t = ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    bar.bottom() - t * bar.height()
}

fn db_at(bar: Rect, y: f32) -> f32 {
    let t = ((bar.bottom() - y) / bar.height().max(1.0)).clamp(0.0, 1.0);
    BOTTOM_DB + t * (TOP_DB - BOTTOM_DB)
}

/// Loudness or a level as text: "-14.2", or "–" for silence.
pub fn db_text(db: f32) -> String {
    if db.is_finite() {
        format!("{db:.1}")
    } else {
        "–".into()
    }
}

/// `now` drawn: up at once, down slowly from what was drawn.
fn falling(drawn: f32, now: f32) -> f32 {
    let fallen = drawn - FALL_DB;
    if now.is_finite() { now.max(fallen) } else { fallen }.max(BOTTOM_DB)
}

impl MasterColumn {
    /// Draws the column in `rect`. `levels` is `None` when nothing plays.
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, mix: Mix) -> MasterResponse {
        let Mix {
            levels,
            gain_db,
            target,
            tip,
        } = mix;
        let mut response = MasterResponse::default();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, theme.panel);
        painter.text(
            pos2(rect.center().x, rect.top() + 10.0),
            Align2::CENTER_CENTER,
            "Master",
            FontId::proportional(11.0),
            theme.text_secondary,
        );
        // The target in the right-click menu, anywhere on the column.
        let column = ui.interact(rect, ui.scope_id().with("master-column"), Sense::click());
        target_menu(&column, target, &mut response);
        let bar = Rect::from_min_max(
            pos2(rect.center().x - 15.0, rect.top() + 24.0),
            pos2(rect.center().x + 17.0, rect.bottom() - 70.0),
        );
        if bar.height() < 20.0 {
            return response;
        }
        if let Some(l) = levels {
            self.held = (l.integrated, l.true_peak_max);
        }
        // Ticks every few dB, labelled on the left.
        for db in [12.0, 0.0, -6.0, -12.0, -24.0, -48.0] {
            let y = y_of(bar, db);
            painter.line_segment(
                [pos2(bar.left() - 3.0, y), pos2(bar.right() + 3.0, y)],
                Stroke::new(1.0, theme.line),
            );
            painter.text(
                pos2(bar.left() - 5.0, y),
                Align2::RIGHT_CENTER,
                format!("{db:.0}"),
                FontId::monospace(8.5),
                theme.faint,
            );
        }
        let lane = |i: f32| {
            Rect::from_min_max(
                pos2(bar.left() + 2.0 + i * 10.0, bar.top()),
                pos2(bar.left() + 10.0 + i * 10.0, bar.bottom()),
            )
        };
        // The two channels, green up to -6 dB, yellow to 0, red above,
        // with a line at the true peak.
        let peaks = levels.map_or([f32::NEG_INFINITY; 2], |l| l.peak_db);
        let true_peaks = levels.map_or([f32::NEG_INFINITY; 2], |l| l.true_peak_db);
        for ch in 0..2 {
            self.shown[ch] = falling(self.shown[ch], peaks[ch]);
            self.shown_true[ch] = falling(self.shown_true[ch], true_peaks[ch]);
            let lane = lane(ch as f32);
            painter.rect_filled(lane, 2.0, theme.bg);
            for (from, to, colour) in [
                (BOTTOM_DB, -6.0, crate::status::GOOD),
                (-6.0, 0.0, crate::status::WARNING),
                (0.0, TOP_DB, crate::status::ERROR),
            ] {
                let top = self.shown[ch].min(to);
                if top > from {
                    painter.rect_filled(
                        Rect::from_min_max(
                            pos2(lane.left(), y_of(bar, top)),
                            pos2(lane.right(), y_of(bar, from)),
                        ),
                        1.0,
                        colour,
                    );
                }
            }
            if self.shown_true[ch] > BOTTOM_DB {
                let y = y_of(bar, self.shown_true[ch]);
                let colour = match self.shown_true[ch] > TRUE_PEAK_CEILING {
                    true => crate::status::ERROR,
                    false => theme.text,
                };
                painter.line_segment(
                    [pos2(lane.left() - 1.0, y), pos2(lane.right() + 1.0, y)],
                    Stroke::new(2.0, colour),
                );
            }
        }
        // The short-term loudness, and the target across it.
        let loudness_lane = lane(2.0).translate(vec2(2.0, 0.0));
        painter.rect_filled(loudness_lane, 2.0, theme.bg);
        self.shown_loudness = falling(
            self.shown_loudness,
            levels.map_or(f32::NEG_INFINITY, |l| l.short_term),
        );
        if self.shown_loudness > BOTTOM_DB {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(loudness_lane.left(), y_of(bar, self.shown_loudness)),
                    loudness_lane.right_bottom(),
                ),
                1.0,
                theme.accent.gamma_multiply(0.8),
            );
        }
        if let Some(aim) = target.lufs() {
            let y = y_of(bar, aim);
            painter.line_segment(
                [
                    pos2(loudness_lane.left() - 2.0, y),
                    pos2(loudness_lane.right() + 2.0, y),
                ],
                Stroke::new(1.5, crate::status::GOOD),
            );
        }
        // Keep drawing while playing, and until the bars have fallen.
        let falling_still = self.shown.iter().chain(&self.shown_true).any(|s| *s > BOTTOM_DB)
            || self.shown_loudness > BOTTOM_DB;
        if levels.is_some() || falling_still {
            ui.ctx().request_repaint();
        }
        // The fader handle rides on the same scale.
        let shown_gain = self.drag.unwrap_or(gain_db);
        let y = y_of(bar, shown_gain);
        let handle = Rect::from_center_size(pos2(bar.center().x, y), vec2(bar.width() + 14.0, 8.0));
        painter.rect_filled(handle, 3.0, theme.text);
        painter.line_segment(
            [pos2(handle.left() + 3.0, y), pos2(handle.right() - 3.0, y)],
            Stroke::new(1.0, theme.bg),
        );
        let id = ui.scope_id().with("master-fader");
        let r = ui
            .interact(bar.expand2(vec2(8.0, 4.0)), id, Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeVertical)
            .on_hover_text(tip);
        target_menu(&r, target, &mut response);
        if r.double_clicked() {
            self.drag = None;
            response.gain_db = Some(0.0);
        } else if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            self.drag = Some((db_at(bar, p.y) * 2.0).round() / 2.0);
        } else if r.drag_stopped() {
            response.gain_db = self.drag.take();
        }
        // Volume and loudness underneath.
        let (integrated, true_peak) = self.held;
        let now = |value: fn(&MeterLevels) -> f32| levels.map_or_else(|| "–".into(), |l| db_text(value(&l)));
        let integrated_colour = match against(integrated, target) {
            AgainstTarget::OnIt => crate::status::GOOD,
            AgainstTarget::Louder => crate::status::WARNING,
            AgainstTarget::Quieter | AgainstTarget::Unknown => theme.text_secondary,
        };
        let true_peak_colour = match true_peak > TRUE_PEAK_CEILING {
            true => crate::status::ERROR,
            false => theme.text_secondary,
        };
        let rows = [
            ("Vol", format!("{shown_gain:+.1}"), theme.text),
            ("M", now(|l| l.momentary), theme.text_secondary),
            ("S", now(|l| l.short_term), theme.text_secondary),
            ("I", db_text(integrated), integrated_colour),
            ("TP", db_text(true_peak), true_peak_colour),
        ];
        let first_y = bar.bottom() + 10.0;
        for (i, (label, value, colour)) in rows.into_iter().enumerate() {
            let y = first_y + i as f32 * 12.0;
            painter.text(
                pos2(rect.left() + 8.0, y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(10.0),
                theme.muted,
            );
            painter.text(
                pos2(rect.right() - 8.0, y),
                Align2::RIGHT_CENTER,
                value,
                FontId::monospace(10.0),
                colour,
            );
        }
        // I and TP: a click starts them again.
        let held_rows = Rect::from_min_max(
            pos2(rect.left() + 4.0, first_y + 3.0 * 12.0 - 6.0),
            pos2(rect.right() - 4.0, first_y + 4.0 * 12.0 + 6.0),
        );
        let held = ui
            .interact(held_rows, ui.scope_id().with("master-held"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(held_tip(target));
        target_menu(&held, target, &mut response);
        if held.clicked() {
            self.held = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            response.reset = true;
        }
        response
    }
}

/// The right-click menu of the column (anywhere on it): the loudness to aim for.
fn target_menu(on: &egui::Response, target: LoudnessTarget, response: &mut MasterResponse) {
    on.context_menu(|ui| {
        ui.label("Loudness to aim for");
        for choice in LoudnessTarget::ALL {
            if ui.radio(choice == target, choice.label()).clicked() {
                response.target = Some(choice);
                ui.close();
            }
        }
    });
}

/// What the I and TP rows say when pointed at.
fn held_tip(target: LoudnessTarget) -> String {
    let aim = match target.lufs() {
        Some(aim) => {
            format!(" Green within {ON_TARGET_LU:.0} LU of the target ({aim:.0} LUFS), yellow louder.")
        }
        None => String::new(),
    };
    format!(
        "I: the integrated loudness since playback started (loop the region to measure just that stretch).{aim}\nTP: the highest true peak since then, the level between samples too; red over {TRUE_PEAK_CEILING:.0} dBTP.\nClick to start both again. Right-click the column for the loudness to aim for."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_round_trips_and_text() {
        let bar = Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 720.0));
        assert_eq!(y_of(bar, TOP_DB), 0.0);
        assert_eq!(y_of(bar, BOTTOM_DB), 720.0);
        assert!((db_at(bar, y_of(bar, -6.0)) + 6.0).abs() < 1e-4);
        assert_eq!(db_text(-14.25), "-14.2");
        assert_eq!(db_text(f32::NEG_INFINITY), "–");
    }

    #[test]
    fn the_integrated_loudness_is_told_against_the_target() {
        let online = LoudnessTarget::Online;
        assert_eq!(online.lufs(), Some(-14.0));
        assert_eq!(against(-14.6, online), AgainstTarget::OnIt);
        assert_eq!(against(-12.5, online), AgainstTarget::Louder);
        assert_eq!(against(-18.0, online), AgainstTarget::Quieter);
        assert_eq!(against(f32::NEG_INFINITY, online), AgainstTarget::Unknown);
        assert_eq!(against(-14.0, LoudnessTarget::Off), AgainstTarget::Unknown);
        assert_eq!(LoudnessTarget::default(), LoudnessTarget::Online);
        for (i, a) in LoudnessTarget::ALL.iter().enumerate() {
            for b in &LoudnessTarget::ALL[i + 1..] {
                assert_ne!(a.label(), b.label());
            }
        }
    }

    #[test]
    fn levels_rise_at_once_and_fall_slowly() {
        assert_eq!(falling(-30.0, -10.0), -10.0);
        assert!((falling(-10.0, -30.0) - (-10.0 - FALL_DB)).abs() < 1e-6);
        assert!((falling(-10.0, f32::NEG_INFINITY) - (-10.0 - FALL_DB)).abs() < 1e-6);
        assert_eq!(falling(BOTTOM_DB, f32::NEG_INFINITY), BOTTOM_DB);
    }

    /// Draws the column once in a 80 × 600 rectangle with `events`.
    fn frame(
        column: &mut MasterColumn,
        events: Vec<egui::Event>,
        levels: Option<MeterLevels>,
    ) -> MasterResponse {
        let ctx = egui::Context::default();
        let mut got = MasterResponse::default();
        for events in [Vec::new(), events] {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 700.0))),
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(80.0, 600.0));
                let mix = Mix {
                    levels,
                    gain_db: 0.0,
                    target: LoudnessTarget::Online,
                    tip: "tip",
                };
                got = column.show(ui, &Theme::ASH, rect, mix);
            });
            output.drop_without_applying_deltas();
        }
        got
    }

    #[test]
    fn the_held_loudness_stays_after_playing_and_a_click_starts_it_again() {
        let mut column = MasterColumn::default();
        let levels = MeterLevels {
            peak_db: [-6.0; 2],
            true_peak_db: [-0.5; 2],
            true_peak_max: -0.5,
            momentary: -13.0,
            short_term: -14.0,
            integrated: -14.2,
        };
        frame(&mut column, Vec::new(), Some(levels));
        frame(&mut column, Vec::new(), None);
        assert_eq!(column.held, (-14.2, -0.5), "still shown once stopped");
        // The I row: 10 + 600 - 70 + 10 + 3 rows of 12, in from the left.
        let at = pos2(40.0, 10.0 + 600.0 - 70.0 + 10.0 + 36.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let response = frame(
            &mut column,
            vec![egui::Event::PointerMoved(at), click(true), click(false)],
            None,
        );
        assert!(response.reset);
        assert_eq!(column.held.0, f32::NEG_INFINITY);
    }
}
