//! The selected audio track's column beside the master column: the same
//! tall meter that is also the fader, for the track the page is working
//! on, so its volume can be set finely. Its name in its colour at the top;
//! the two channels' peaks and the loudness over 3 s on the master's scale;
//! the volume, and M and S, underneath. Built from the master column's
//! parts, so both read the same (working rule 1). The same column serves
//! tracks pinned beside the master, with an × to unpin them.

use crate::Theme;
use crate::master::{db_at, db_text, falling, volume_text, y_of};
use crate::theme::status;
use crate::volume::{self, BOTTOM_DB, TOP_DB};
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// What the column shows of the track this frame.
#[derive(Debug, Clone, Copy)]
pub struct TrackStrip<'a> {
    pub id: u64,
    pub name: &'a str,
    pub colour: Color32,
    pub gain_db: f32,
    pub muted: bool,
    pub solo: bool,
    /// Its channels' peaks (dBFS) and loudness over 3 s (LUFS); `None`
    /// while nothing plays.
    pub levels: Option<([f32; 2], f32)>,
    /// A pinned track's column: an × at its top right unpins it.
    pub closable: bool,
}

/// What a frame of the column asks of the page.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackColumnResponse {
    /// The track's new volume, when the fader is let go (or double-clicked).
    pub gain_db: Option<f32>,
    /// M or S clicked: mute and solo as they are to be.
    pub mute_solo: Option<(bool, bool)>,
    /// The volume while the fader is dragged, to show and hear.
    pub dragging: Option<f32>,
    /// The × was clicked: unpin it.
    pub close: bool,
    /// S was Ctrl-clicked: solo this track alone.
    pub solo_only: bool,
}

/// Kept by the page between frames.
#[derive(Debug, Clone)]
pub struct TrackColumn {
    drag: Option<(u64, f32)>,
    shown: [f32; 2],
    shown_loudness: f32,
}

impl Default for TrackColumn {
    fn default() -> Self {
        TrackColumn {
            drag: None,
            shown: [BOTTOM_DB; 2],
            shown_loudness: BOTTOM_DB,
        }
    }
}

impl TrackColumn {
    /// Draws the column for `strip` in `rect`.
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, strip: TrackStrip) -> TrackColumnResponse {
        let mut response = TrackColumnResponse::default();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, theme.panel);
        painter.rect_filled(
            Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(rect.width() - 12.0, 3.0)),
            1.5,
            strip.colour,
        );
        if strip.closable {
            let x = Rect::from_center_size(pos2(rect.right() - 10.0, rect.top() + 12.0), vec2(14.0, 14.0));
            let r = ui
                .interact(
                    x,
                    ui.scope_id().with(("track-column-close", strip.id)),
                    Sense::click(),
                )
                .on_hover_text(format!("Unpin {} from beside the master", strip.name));
            let colour = if r.hovered() { theme.text } else { theme.muted };
            for (a, b) in [
                (vec2(-3.5, -3.5), vec2(3.5, 3.5)),
                (vec2(-3.5, 3.5), vec2(3.5, -3.5)),
            ] {
                painter.line_segment([x.center() + a, x.center() + b], Stroke::new(1.4, colour));
            }
            response.close = r.clicked();
        }
        // The name, short of the × when there is one.
        let room = rect.shrink2(vec2(4.0, 0.0)).with_max_x(match strip.closable {
            true => rect.right() - 18.0,
            false => rect.right() - 4.0,
        });
        painter.with_clip_rect(room.intersect(rect)).text(
            pos2(room.center().x, rect.top() + 12.0),
            Align2::CENTER_CENTER,
            strip.name,
            FontId::proportional(11.0),
            theme.text,
        );
        let bar = Rect::from_min_max(
            pos2(rect.center().x - 15.0, rect.top() + 26.0),
            pos2(rect.center().x + 17.0, rect.bottom() - 58.0),
        );
        if bar.height() < 20.0 {
            return response;
        }
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
        // Another track chosen: its levels start from silence.
        if self.drag.is_some_and(|(t, _)| t != strip.id) {
            self.drag = None;
        }
        let peaks = strip.levels.map_or([f32::NEG_INFINITY; 2], |(p, _)| p);
        for (ch, peak) in peaks.iter().enumerate() {
            self.shown[ch] = falling(self.shown[ch], *peak);
            let lane = lane(ch as f32);
            painter.rect_filled(lane, 2.0, theme.bg);
            for (from, to, colour) in [
                (BOTTOM_DB, -6.0, strip.colour),
                (-6.0, 0.0, status::WARNING),
                (0.0, TOP_DB, status::ERROR),
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
        let loudness_lane = lane(2.0).translate(vec2(2.0, 0.0));
        painter.rect_filled(loudness_lane, 2.0, theme.bg);
        self.shown_loudness = falling(
            self.shown_loudness,
            strip.levels.map_or(f32::NEG_INFINITY, |(_, l)| l),
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
        if strip.levels.is_some()
            || self.shown.iter().any(|s| *s > BOTTOM_DB)
            || self.shown_loudness > BOTTOM_DB
        {
            ui.ctx().request_repaint();
        }
        // The fader handle on the same scale.
        let shown_gain = self.drag.map_or(strip.gain_db, |(_, d)| d);
        let y = y_of(bar, shown_gain);
        let handle = Rect::from_center_size(pos2(bar.center().x, y), vec2(bar.width() + 14.0, 8.0));
        painter.rect_filled(handle, 3.0, theme.text);
        painter.line_segment(
            [pos2(handle.left() + 3.0, y), pos2(handle.right() - 3.0, y)],
            Stroke::new(1.0, theme.bg),
        );
        let r = ui
            .interact(
                bar.expand2(vec2(8.0, 4.0)),
                ui.scope_id().with(("track-column-fader", strip.id)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical)
            .on_hover_text(format!(
                "{}: the selected audio track's volume. Drag (Shift: finer), double-click for 0 dB. The lanes are its two channels and its loudness over 3 s.",
                strip.name
            ));
        let shift = ui.input(|i| i.modifiers.shift);
        if r.double_clicked() {
            self.drag = None;
            response.gain_db = Some(0.0);
        } else if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            // Shift: a fifth as fast from where the drag is, for fine steps.
            let at = match (shift, self.drag) {
                (true, Some((_, d))) => d + (db_at(bar, p.y) - db_at(bar, p.y - r.drag_delta().y)) * 0.2,
                _ => db_at(bar, p.y),
            };
            let settled = match shift {
                true => volume::settled_fine(at),
                false => volume::settled(at),
            };
            self.drag = Some((strip.id, settled));
            response.dragging = Some(settled);
        } else if r.drag_stopped() {
            response.gain_db = self.drag.take().map(|(_, d)| d);
        }
        // The volume, and M and S, underneath.
        let first_y = bar.bottom() + 12.0;
        painter.text(
            pos2(rect.left() + 8.0, first_y),
            Align2::LEFT_CENTER,
            "Vol",
            FontId::proportional(10.0),
            theme.muted,
        );
        painter.text(
            pos2(rect.right() - 8.0, first_y),
            Align2::RIGHT_CENTER,
            volume_text(shown_gain),
            FontId::monospace(10.0),
            theme.text,
        );
        let loudness = strip.levels.map_or_else(|| "–".to_owned(), |(_, l)| db_text(l));
        painter.text(
            pos2(rect.left() + 8.0, first_y + 12.0),
            Align2::LEFT_CENTER,
            "S",
            FontId::proportional(10.0),
            theme.muted,
        );
        painter.text(
            pos2(rect.right() - 8.0, first_y + 12.0),
            Align2::RIGHT_CENTER,
            loudness,
            FontId::monospace(10.0),
            theme.text_secondary,
        );
        let buttons_y = first_y + 24.0;
        let width = (rect.width() - 22.0) / 2.0;
        for (i, (label, on, tip)) in [
            ("M", strip.muted, "Mute this track"),
            (
                "S",
                strip.solo,
                "Solo: hear only soloed tracks. Ctrl+click: this track alone",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let b = Rect::from_min_size(
                pos2(rect.left() + 8.0 + i as f32 * (width + 6.0), buttons_y),
                vec2(width, 16.0),
            );
            let r = ui
                .interact(
                    b,
                    ui.scope_id().with(("track-column", label, strip.id)),
                    Sense::click(),
                )
                .on_hover_text(tip);
            let fill = match (on, i) {
                (true, 0) => status::WARNING,
                (true, _) => theme.accent,
                _ if r.hovered() => theme.line_strong,
                _ => theme.line,
            };
            painter.rect_filled(b, 3.0, fill);
            painter.text(
                b.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.5),
                if on { theme.bg } else { theme.text },
            );
            if r.clicked() && i == 1 && ui.input(|i| i.modifiers.command) {
                response.solo_only = true;
            } else if r.clicked() {
                response.mute_solo = Some(match i {
                    0 => (!strip.muted, strip.solo),
                    _ => (strip.muted, !strip.solo),
                });
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(column: &mut TrackColumn, ctx: &egui::Context, events: Vec<egui::Event>) -> TrackColumnResponse {
        let mut out = TrackColumnResponse::default();
        let input = egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(200.0, 500.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let strip = TrackStrip {
                id: 7,
                name: "Mic",
                colour: Color32::LIGHT_BLUE,
                gain_db: -6.0,
                muted: false,
                solo: false,
                levels: None,
                closable: true,
            };
            out = column.show(
                ui,
                &Theme::CARBON,
                Rect::from_min_size(egui::Pos2::ZERO, vec2(84.0, 400.0)),
                strip,
            );
        });
        output.drop_without_applying_deltas();
        out
    }

    #[test]
    fn the_fader_is_one_change_when_let_go_and_m_mutes() {
        let ctx = egui::Context::default();
        let mut column = TrackColumn::default();
        let bar = Rect::from_min_max(pos2(42.0 - 15.0, 26.0), pos2(42.0 + 17.0, 400.0 - 58.0));
        let at = |db: f32| pos2(bar.center().x, y_of(bar, db));
        let button = |p, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut column, &ctx, vec![egui::Event::PointerMoved(at(-6.0))]);
        frame(&mut column, &ctx, vec![button(at(-6.0), true)]);
        frame(&mut column, &ctx, vec![egui::Event::PointerMoved(at(-3.0))]);
        let moving = frame(&mut column, &ctx, vec![egui::Event::PointerMoved(at(0.5))]);
        assert_eq!(moving.dragging, Some(0.0), "0 dB is easy to find");
        let done = frame(&mut column, &ctx, vec![button(at(0.5), false)]);
        assert_eq!(done.gain_db, Some(0.0));
        // M under the readings.
        let m = pos2(8.0 + 10.0, bar.bottom() + 36.0 + 8.0);
        frame(&mut column, &ctx, vec![egui::Event::PointerMoved(m)]);
        frame(&mut column, &ctx, vec![button(m, true)]);
        let clicked = frame(&mut column, &ctx, vec![button(m, false)]);
        assert_eq!(clicked.mute_solo, Some((true, false)));
        // The × of a pinned column.
        let x = pos2(84.0 - 10.0, 12.0);
        frame(&mut column, &ctx, vec![egui::Event::PointerMoved(x)]);
        frame(&mut column, &ctx, vec![button(x, true)]);
        let closed = frame(&mut column, &ctx, vec![button(x, false)]);
        assert!(closed.close);
    }
}
