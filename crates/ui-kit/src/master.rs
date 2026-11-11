//! The master column beside the timeline: the meter of the whole mix,
//! which is also its volume fader, and the loudness under it. The same on
//! every page that shows it.

use crate::Theme;
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// The scale shared by the meter and the fader, in dB.
const TOP_DB: f32 = 12.0;
const BOTTOM_DB: f32 = -60.0;

/// What the meter shows: peaks of each channel (dB below full scale) and
/// the loudness over 0.4 s and 3 s (LUFS). Silence is `-inf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterLevels {
    pub peak_db: [f32; 2],
    pub momentary: f32,
    pub short_term: f32,
}

/// Kept by the page between frames.
#[derive(Debug, Clone, Default)]
pub struct MasterColumn {
    /// The fader while it is being dragged.
    drag: Option<f32>,
    /// Peaks drawn, falling slowly so they can be read.
    shown: [f32; 2],
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

impl MasterColumn {
    /// Draws the column in `rect`. `levels` is `None` when nothing plays.
    /// Returns the new volume when the fader is let go (or double-clicked
    /// back to 0 dB).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        levels: Option<MeterLevels>,
        gain_db: f32,
        tip: &str,
    ) -> Option<f32> {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, theme.panel);
        painter.text(
            pos2(rect.center().x, rect.top() + 10.0),
            Align2::CENTER_CENTER,
            "Master",
            FontId::proportional(11.0),
            theme.text_secondary,
        );
        let bar = Rect::from_min_max(
            pos2(rect.center().x - 13.0, rect.top() + 24.0),
            pos2(rect.center().x + 13.0, rect.bottom() - 46.0),
        );
        if bar.height() < 20.0 {
            return None;
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
        // The two channels, green up to -6 dB, yellow to 0, red above.
        let peaks = levels.map_or([f32::NEG_INFINITY; 2], |l| l.peak_db);
        for (ch, peak) in peaks.iter().enumerate() {
            // Falls about 20 dB a second at 60 frames a second.
            self.shown[ch] = if peak.is_finite() {
                peak.max(self.shown[ch] - 0.35)
            } else {
                self.shown[ch] - 0.35
            };
            self.shown[ch] = self.shown[ch].max(BOTTOM_DB);
            let lane = Rect::from_min_max(
                pos2(bar.left() + 2.0 + ch as f32 * 12.0, bar.top()),
                pos2(bar.left() + 12.0 + ch as f32 * 12.0, bar.bottom()),
            );
            painter.rect_filled(lane, 2.0, theme.bg);
            for (from, to, colour) in [
                (BOTTOM_DB, -6.0, Color32::from_rgb(0x5D, 0xBB, 0x7A)),
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
        }
        if levels.is_some() {
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
        let mut changed = None;
        if r.double_clicked() {
            self.drag = None;
            changed = Some(0.0);
        } else if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            self.drag = Some((db_at(bar, p.y) * 2.0).round() / 2.0);
        } else if r.drag_stopped() {
            changed = self.drag.take();
        }
        // Volume and loudness underneath.
        let mut text_y = bar.bottom() + 10.0;
        for (label, value, colour) in [
            ("Vol", format!("{shown_gain:+.1}"), theme.text),
            (
                "M",
                levels.map_or_else(|| "–".into(), |l| db_text(l.momentary)),
                theme.text_secondary,
            ),
            (
                "S",
                levels.map_or_else(|| "–".into(), |l| db_text(l.short_term)),
                theme.text_secondary,
            ),
        ] {
            painter.text(
                pos2(rect.left() + 8.0, text_y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(10.0),
                theme.muted,
            );
            painter.text(
                pos2(rect.right() - 8.0, text_y),
                Align2::RIGHT_CENTER,
                value,
                FontId::monospace(10.0),
                colour,
            );
            text_y += 12.0;
        }
        changed
    }
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
}
