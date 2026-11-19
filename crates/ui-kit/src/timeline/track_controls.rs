//! The column of track names, the part that works: each header's name,
//! role mark and reading line, an audio track's level button, M, S and
//! volume fader, a video track's eye, the edge that resizes a track, and
//! the clicks on the column (its menu, a double-click to rename).

use super::{
    HEADER_WIDTH, Half, Layout, Rows, TimelineData, TimelineEvent, TimelineTrack, TimelineView, headers,
    level, loading, tracks_of,
};
use crate::Theme;
use crate::knob::KnobLook;
use egui::{Align2, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

/// A track's height being changed by dragging the edge of its name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HeightDrag {
    track: u64,
    /// The height and the pointer's y when the drag began.
    from_height: f32,
    from_y: f32,
}

impl TimelineView {
    /// A track's header: its name (or the field it is being typed in),
    /// its role's mark, the line about what is still being read, and its
    /// buttons. True while its name is being typed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_track_header(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        data: &TimelineData,
        layout: &Layout,
        header_painter: &egui::Painter,
        track: &TimelineTrack,
        row: Rect,
        area: Rect,
        audio: bool,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let left = layout.time_left();
        let mut renaming = false;
        let header =
            Rect::from_min_max(pos2(layout.header_x, row.top()), pos2(left, row.bottom())).intersect(area);
        match self.rename.as_mut().filter(|r| r.track == track.id) {
            Some(rename) => {
                renaming = true;
                match headers::name_field(ui, rename, header, row.center().y) {
                    headers::Typed::Going => {}
                    headers::Typed::Dropped => self.rename = None,
                    headers::Typed::Done(name) => {
                        self.rename = None;
                        if name != track.name {
                            events.push(TimelineEvent::RenameTrack {
                                track: track.id,
                                name,
                            });
                        }
                    }
                }
            }
            None => headers::draw_name(
                header_painter,
                theme,
                track,
                pos2(layout.header_x + 12.0, row.center().y),
            ),
        }
        // A track's role: a mark in its colour along the header's edge.
        if let Some((_, colour)) = track.role.and_then(|i| data.roles.get(i)) {
            let mark = Rect::from_min_max(
                pos2(layout.header_x + 3.0, row.top() + 4.0),
                pos2(layout.header_x + 6.0, row.bottom() - 4.0),
            );
            header_painter.rect_filled(mark, 1.5, *colour);
        }
        // The short line about reading stops before the header's
        // buttons; pointing at it says the whole of it.
        let first_button = headers::button_rect(header, if audio { -1.0 } else { 1.0 });
        let text_room = Rect::from_min_max(header.min, pos2(first_button.left() - 4.0, header.bottom()));
        let text_painter = header_painter.with_clip_rect(header_painter.clip_rect().intersect(text_room));
        if let Some(loading) = &track.loading
            && let Some(line) = loading::draw_in_header(
                &text_painter,
                theme,
                header,
                text_room.right() - header.left() - 12.0,
                loading,
            )
        {
            ui.interact(
                line.expand(3.0).intersect(header),
                ui.scope_id().with(("track loading", track.id)),
                Sense::hover(),
            )
            .on_hover_text(loading.explanation());
        }
        if audio {
            let index = data.audio.iter().position(|t| t.id == track.id).unwrap_or(0);
            // The knob is placed by the whole row; what of it is out of
            // the area's view is cut off, as the clips are.
            let whole =
                Rect::from_min_max(pos2(header.left(), row.top()), pos2(header.right(), row.bottom()));
            self.track_controls(
                ui,
                theme,
                track,
                [header, whole],
                super::audio_colour(index),
                events,
            );
        } else {
            headers::eye(ui, theme, track, header, self.compact, events);
        }
        renaming
    }

    /// The column of track names: right-click for the menu of tracks,
    /// double-click a name to change it.
    pub(super) fn names_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        rows: [&Rows; 2],
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        let left = layout.time_left();
        for (half, area, rows) in [
            (Half::Video, layout.video, rows[0]),
            (Half::Audio, layout.audio, rows[1]),
        ] {
            let names = Rect::from_min_max(area.min, pos2(left, area.bottom()));
            let r = ui.interact(names, id.with(("names", half)), Sense::click());
            let under = r
                .interact_pointer_pos()
                .or(r.hover_pos())
                .and_then(|p| rows.at(p.y))
                .and_then(|row| tracks_of(half, data).get(row));
            if r.secondary_clicked() {
                self.names_menu = Some((half, under.map(|t| t.id)));
            }
            if r.double_clicked()
                && let Some(track) = under
            {
                self.rename = Some(headers::Rename::of(track));
            }
            headers::menu(
                &r,
                self.names_menu.filter(|(h, _)| *h == half),
                data,
                &mut self.rename,
                &mut self.level_lines,
                events,
            );
        }
    }

    /// M (mute), S (solo) and a volume fader in an audio track's header.
    fn track_controls(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        [header, whole]: [Rect; 2],
        colour: egui::Color32,
        events: &mut Vec<TimelineEvent>,
    ) {
        if header.height() < headers::least_for_buttons(self.compact) {
            return;
        }
        let id = ui.scope_id().with(("track", track.id));
        let knob = self.knob_of(track.id).filter(|_| !self.compact).and_then(|look| {
            // The detailed knob under the buttons; the compact one beside
            // them. Too short for it: the compact one, or none.
            let fits = |look: KnobLook| match look {
                KnobLook::Detailed => whole.height() >= 28.0 + look.size().y,
                KnobLook::Compact => whole.height() >= look.size().y,
            };
            [look, KnobLook::Compact].into_iter().find(|l| fits(*l))
        });
        // The buttons sit left of a compact knob.
        let buttons = match knob {
            Some(KnobLook::Compact) => header.with_max_x(header.right() - KnobLook::Compact.size().x - 4.0),
            _ => header,
        };
        let button = |i: f32| headers::button_rect(buttons, i);
        if !self.compact {
            self.level_button(ui, theme, track.id, button(-1.0), id);
        }
        self.mute_and_solo(ui, theme, track, buttons, id, events);
        match knob {
            Some(look) => {
                // Drawn within what is in view of the header (a scope
                // would take room in the page's row, so the clip is set
                // and put back).
                let before = ui.clip_rect();
                ui.set_clip_rect(before.intersect(header));
                self.knob(ui, theme, track, whole, (look, colour), id, events);
                ui.set_clip_rect(before);
            }
            // The fader, in a header with room for it under the buttons.
            None if !self.compact && self.knob_of(track.id).is_none() => {
                self.fader(ui, theme, track, header, id, events)
            }
            None => {}
        }
    }

    /// The track's volume knob: beside the buttons (compact) or under them
    /// (detailed), on the right of the header. Turned, it is drawn and
    /// heard at once; let go, it is one change.
    #[allow(clippy::too_many_arguments)]
    fn knob(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        header: Rect,
        (look, colour): (KnobLook, egui::Color32),
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        let size = look.size();
        let min = match look {
            KnobLook::Compact => pos2(header.right() - size.x - 2.0, header.center().y - size.y / 2.0),
            KnobLook::Detailed => pos2(header.right() - size.x - 4.0, header.top() + 26.0),
        };
        let knob_id = id.with("knob");
        let tip = crate::knob::tip(&track.name, track.gain_db);
        let knob = crate::knob::Knob {
            look,
            at: min,
            id: knob_id,
            db: track.gain_db,
            colour,
            tip: &tip,
        };
        let out = crate::knob::volume_knob(ui, theme, knob);
        let area = Rect::from_min_size(min, size);
        if area.contains(
            ui.input(|i| i.pointer.hover_pos())
                .unwrap_or(Pos2::new(f32::NAN, f32::NAN)),
        ) {
            self.knob_under = Some((area, knob_id));
        }
        // Drawn (the waveforms follow) while it is being turned.
        if out.shown_db != track.gain_db {
            self.fader_drag = Some((track.id, out.shown_db));
        } else if self.fader_drag.is_some_and(|(t, _)| t == track.id) {
            self.fader_drag = None;
        }
        if let Some(gain_db) = out.done {
            events.push(TimelineEvent::TrackMix {
                track: track.id,
                muted: track.muted,
                solo: track.solo,
                gain_db,
            });
        }
    }

    /// The level button, left of M and S: the level line on the clips of
    /// track `track`.
    fn level_button(&mut self, ui: &mut Ui, theme: &Theme, track: u64, rect: Rect, id: egui::Id) {
        let on = self.level_line(track);
        let r = ui
            .interact(rect, id.with("level"), Sense::click())
            .on_hover_text(if on {
                "Level lines on: drag the line across a clip to change its volume, double-click it for 0 dB. Click to hide them"
            } else {
                "Level lines: show a line across each clip of this track, to drag its volume up or down"
            });
        let fill = match on {
            true => theme.accent,
            false if r.hovered() => theme.line_strong,
            false => theme.line,
        };
        ui.painter().rect_filled(rect, 3.0, fill);
        level::draw_icon(
            ui.painter(),
            rect.center(),
            if on { theme.bg } else { theme.text },
        );
        if r.clicked() {
            self.set_level_line(track, !on);
        }
    }

    /// M (mute) and S (solo), the two buttons at the right of an audio
    /// track's header.
    fn mute_and_solo(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        header: Rect,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        for (i, (label, on, tip)) in [
            ("M", track.muted, "Mute"),
            ("S", track.solo, "Solo: hear only soloed tracks"),
        ]
        .into_iter()
        .enumerate()
        {
            let rect = headers::button_rect(header, i as f32);
            let r = ui
                .interact(rect, id.with(label), Sense::click())
                .on_hover_text(tip);
            let fill = match (on, i) {
                (true, 0) => crate::status::WARNING,
                (true, _) => theme.accent,
                _ if r.hovered() => theme.line_strong,
                _ => theme.line,
            };
            let p = ui.painter();
            p.rect_filled(rect, 3.0, fill);
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.5),
                if on { theme.bg } else { theme.text },
            );
            if r.clicked() {
                let (muted, solo) = if i == 0 {
                    (!track.muted, track.solo)
                } else {
                    (track.muted, !track.solo)
                };
                events.push(TimelineEvent::TrackMix {
                    track: track.id,
                    muted,
                    solo,
                    gain_db: track.gain_db,
                });
            }
        }
    }

    /// The track's volume fader under its buttons, on the one volume
    /// scale ([`crate::volume`]: the bottom at the left, the top at the
    /// right); double-click resets to 0. Only in a header with room for it.
    fn fader(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        header: Rect,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        let bar = Rect::from_min_size(
            pos2(header.left() + 12.0, header.bottom() - 12.0),
            vec2(HEADER_WIDTH - 30.0, 5.0),
        );
        if bar.bottom() > header.bottom() || bar.top() < header.top() + 22.0 {
            return;
        }
        let db = match self.fader_drag {
            Some((t, db)) if t == track.id => db,
            _ => track.gain_db,
        };
        let to_x = |db: f32| fader_x(bar, db);
        let r = ui
            .interact(
                bar.expand2(vec2(4.0, 5.0)),
                id.with("volume"),
                Sense::click_and_drag(),
            )
            .on_hover_text(fader_tip(db));
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            self.fader_drag = Some((track.id, fader_db_at(bar, p.x)));
        }
        if r.drag_stopped()
            && let Some((_, v)) = self.fader_drag.take()
        {
            events.push(TimelineEvent::TrackMix {
                track: track.id,
                muted: track.muted,
                solo: track.solo,
                gain_db: v,
            });
        }
        if r.double_clicked() {
            events.push(TimelineEvent::TrackMix {
                track: track.id,
                muted: track.muted,
                solo: track.solo,
                gain_db: 0.0,
            });
        }
        let p = ui.painter();
        p.rect_filled(bar, 2.0, theme.line);
        p.rect_filled(
            Rect::from_min_max(bar.min, pos2(to_x(db), bar.bottom())),
            2.0,
            theme.text_secondary,
        );
        p.line_segment(
            [
                pos2(to_x(0.0), bar.top() - 2.0),
                pos2(to_x(0.0), bar.bottom() + 2.0),
            ],
            Stroke::new(1.0, theme.muted),
        );
        p.circle_filled(pos2(to_x(db), bar.center().y), 5.0, theme.text);
    }

    /// The edge of a track's name that resizes the track: the bottom edge of
    /// an audio track, the top edge of a video track (the side each area
    /// grows towards). Drag it, or double-click for the usual height.
    pub(super) fn track_edge(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        half: Half,
        track: &TimelineTrack,
        row: Rect,
        area: Rect,
    ) {
        let edge = match half {
            Half::Video => row.top(),
            Half::Audio => row.bottom(),
        };
        let handle = Rect::from_min_max(
            pos2(area.left(), edge - 3.0),
            pos2(area.left() + self.header_width(), edge + 3.0),
        )
        .intersect(area);
        // A drag goes on while the button is held, also when the edge has
        // been dragged out of the area and can't be pointed at any more.
        if !ui.input(|i| i.pointer.primary_down()) {
            self.height_drag = None;
        }
        // Compact tracks are all one height: nothing to drag.
        if self.compact {
            self.height_drag = None;
            return;
        }
        let r = handle.is_positive().then(|| {
            ui.interact(
                handle,
                ui.scope_id().with(("track height", track.id)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical)
        });
        if r.as_ref()
            .is_some_and(|r| r.drag_started_by(egui::PointerButton::Primary))
        {
            let from_y = ui.input(|i| i.pointer.press_origin()).map_or(edge, |p| p.y);
            self.height_drag = Some(HeightDrag {
                track: track.id,
                from_height: row.height(),
                from_y,
            });
        }
        let dragging = self.height_drag.filter(|d| d.track == track.id);
        if let Some(drag) = dragging
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            // Down makes an audio track taller; up makes a video track taller.
            let by = match half {
                Half::Video => drag.from_y - p.y,
                Half::Audio => p.y - drag.from_y,
            };
            let usual = self.usual_height(half);
            self.heights.set(track.id, usual, drag.from_height + by);
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if r.as_ref().is_some_and(|r| r.double_clicked()) {
            self.heights.reset(track.id);
        }
        // The edge lights up when pointed at; while dragged, across the whole timeline.
        let pointed_at = r.as_ref().is_some_and(|r| r.hovered());
        if dragging.is_some() || pointed_at {
            let (right, colour) = match dragging {
                Some(_) => (area.right(), theme.text_secondary),
                None => (handle.right(), theme.muted),
            };
            ui.painter_at(area).line_segment(
                [pos2(area.left(), edge), pos2(right, edge)],
                Stroke::new(2.0, colour),
            );
        }
        if dragging.is_none()
            && let Some(r) = r
        {
            r.on_hover_text(
                "Drag to make this track taller or shorter. Double-click: its usual height. Alt+wheel: every track here.",
            );
        }
    }
}

/// x on a track header's fader `bar` for `db` (the fader law of
/// [`crate::volume`], bottom at the left).
pub(crate) fn fader_x(bar: Rect, db: f32) -> f32 {
    bar.left() + crate::volume::share_of(db) * bar.width()
}

/// Where a track header's fader `bar` dragged to `x` lands.
pub(crate) fn fader_db_at(bar: Rect, x: f32) -> f32 {
    crate::volume::settled(crate::volume::db_at_share(
        (x - bar.left()) / bar.width().max(1.0),
    ))
}

/// What a track header's fader says when pointed at: "Volume −6.0 dB
/// (double-click: 0 dB)"; a track at the bottom of the range is heard.
pub(crate) fn fader_tip(db: f32) -> String {
    let words = crate::volume::words(db, crate::volume::Bottom::Heard);
    format!("Volume {words} (double-click: 0 dB)")
}
