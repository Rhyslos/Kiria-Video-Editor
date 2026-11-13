//! The mixer: one vertical strip per audio track, side by side, scrolling
//! sideways. Each strip is a level meter that is also the track's fader
//! (on the master column's scale, [`crate::master`]: drag it, double-click
//! for 0 dB), a box with the exact volume (click it and type), the
//! effects that are on as tags (a click opens that effect), mute and solo,
//! and its role as a colour along its top (right-click to choose).
//!
//! Only the strips in view are drawn and take input, so dozens of tracks
//! cost what the few on screen cost. The page keeps a [`Mixer`] between
//! frames; it knows nothing of projects and hands back [`StripEvent`]s.

use crate::Theme;
use crate::master::{BOTTOM_DB, TOP_DB, db_at, falling, y_of};
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;

/// How wide a strip is.
pub const STRIP_WIDTH: f32 = 84.0;
/// The scroll bar under the strips.
const SCROLLBAR: f32 = 10.0;

/// What a strip's meter shows, as the mixer measured it (silence is `-inf`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripLevels {
    /// Each channel's highest sample since the last frame, dBFS.
    pub peak_db: [f32; 2],
    /// The loudness over 3 s, LUFS.
    pub short_term: f32,
}

/// An effect's tag on a strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tag<'a> {
    pub name: &'a str,
    /// What it is doing now, when pointed at.
    pub tip: &'a str,
}

/// One track's strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip<'a> {
    pub id: u64,
    /// Its short name ("A3") and its name.
    pub short: &'a str,
    pub name: &'a str,
    /// Its role, an index into the roles handed to [`Mixer::show`].
    pub role: Option<usize>,
    pub gain_db: f32,
    pub muted: bool,
    pub solo: bool,
    /// The track the page's tools are working on: drawn lit.
    pub selected: bool,
    /// `None` while nothing plays.
    pub levels: Option<StripLevels>,
    /// The effects that are on.
    pub tags: &'a [Tag<'a>],
}

/// Something done on a strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StripEvent {
    /// The fader moved (`done` once it is let go, typed or double-clicked:
    /// then it is one undo step; before, it is heard as it moves).
    Gain {
        db: f32,
        done: bool,
    },
    Mute(bool),
    Solo(bool),
    /// A tag was clicked: open that effect (an index into the strip's tags).
    Tag(usize),
    /// The strip was clicked: work on this track.
    Select,
    /// A role was chosen in its right-click menu.
    Role(usize),
}

/// What the mixer keeps of a strip between frames.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct StripState {
    /// Peaks drawn, falling slowly.
    shown: [f32; 2],
}

/// The mixer row: where it is scrolled, the levels drawn, a fader being
/// dragged and a volume being typed.
#[derive(Debug, Default)]
pub struct Mixer {
    /// How far the strips are scrolled, in points.
    scroll: f32,
    states: HashMap<u64, StripState>,
    drag: Option<(u64, f32)>,
    typing: Option<(u64, String)>,
    scrollbar_grab: Option<(f32, f32)>,
}

/// The strips in view, `scroll` points along a row `width` wide of
/// `count` strips: the first and one past the last.
pub fn in_view(scroll: f32, width: f32, count: usize) -> (usize, usize) {
    let first = (scroll / STRIP_WIDTH).floor().max(0.0) as usize;
    let last = ((scroll + width) / STRIP_WIDTH).ceil().max(0.0) as usize;
    (first.min(count), last.min(count))
}

/// How far a row of `count` strips `width` wide can scroll.
pub fn most_scroll(width: f32, count: usize) -> f32 {
    (count as f32 * STRIP_WIDTH - width).max(0.0)
}

/// A typed volume: "-6", "−6.5 dB", "+3", "0", "off" (silent, the bottom).
pub fn read_db(text: &str) -> Option<f32> {
    let text = text.trim().to_lowercase().replace('−', "-");
    if text == "off" {
        return Some(BOTTOM_DB);
    }
    let number = text.trim_end_matches("db").trim();
    let db: f32 = number.parse().ok()?;
    db.is_finite().then(|| db.clamp(BOTTOM_DB, TOP_DB))
}

/// A volume as a strip's box writes it: "−8.0 dB", "0.0 dB", "+1.5 dB", "Off".
pub fn db_words(db: f32) -> String {
    crate::timeline::level::words(f64::from(db))
}

/// Where a fader lands at `db` as it is dragged: half decibels, 0 dB
/// within one.
fn settled(db: f32) -> f32 {
    if db.abs() < 1.0 {
        0.0
    } else {
        (db * 2.0).round() / 2.0
    }
}

impl Mixer {
    /// A fader being dragged: its track and volume now, to be heard.
    pub fn gain_being_set(&self) -> Option<(u64, f32)> {
        self.drag
    }

    /// Scrolls so the strip at `index` is in a row `width` wide.
    pub fn show_strip(&mut self, index: usize, width: f32) {
        let left = index as f32 * STRIP_WIDTH;
        if left < self.scroll {
            self.scroll = left;
        } else if left + STRIP_WIDTH > self.scroll + width {
            self.scroll = left + STRIP_WIDTH - width;
        }
    }

    /// Draws `strips` in `rect`, scrolled sideways, and returns what was
    /// done to which track. `roles` are the roles a track can have, with
    /// their colours.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        strips: &[Strip],
        roles: &[(&str, Color32)],
    ) -> Vec<(u64, StripEvent)> {
        let mut events = Vec::new();
        let row = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - SCROLLBAR));
        let most = most_scroll(row.width(), strips.len());
        // The wheel scrolls sideways over the strips (either way it turns).
        if ui.rect_contains_pointer(row) {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            let by = if delta.x.abs() > delta.y.abs() {
                delta.x
            } else {
                delta.y
            };
            self.scroll -= by;
        }
        self.scroll = self.scroll.clamp(0.0, most);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.panel);
        let (first, last) = in_view(self.scroll, row.width(), strips.len());
        let row_painter = ui.painter_at(row);
        let mut seen = Vec::with_capacity(last - first);
        for (i, strip) in strips.iter().enumerate().take(last).skip(first) {
            let x = row.left() + i as f32 * STRIP_WIDTH - self.scroll;
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(STRIP_WIDTH, row.height()));
            seen.push(strip.id);
            for event in self.strip(ui, theme, &row_painter, r, row, strip, roles) {
                events.push((strip.id, event));
            }
        }
        // Levels of strips out of view are forgotten.
        self.states.retain(|id, _| seen.contains(id));
        self.scrollbar(ui, theme, rect, row, strips.len());
        if strips.iter().any(|s| s.levels.is_some())
            || self
                .states
                .values()
                .any(|s| s.shown.iter().any(|v| *v > BOTTOM_DB))
        {
            ui.ctx().request_repaint();
        }
        events
    }

    /// The bar under the strips: drag its thumb, or click beside it.
    fn scrollbar(&mut self, ui: &mut Ui, theme: &Theme, rect: Rect, row: Rect, count: usize) {
        let bar = Rect::from_min_max(pos2(rect.left(), row.bottom()), rect.max);
        let total = count as f32 * STRIP_WIDTH;
        if total <= row.width() {
            return;
        }
        let share = row.width() / total;
        let thumb_w = (bar.width() * share).max(24.0);
        let travel = (bar.width() - thumb_w).max(1.0);
        let most = most_scroll(row.width(), count);
        let thumb_x = bar.left() + self.scroll / most.max(1.0) * travel;
        let thumb = Rect::from_min_size(pos2(thumb_x, bar.top() + 2.0), vec2(thumb_w, bar.height() - 4.0));
        let r = ui.interact(
            bar,
            ui.scope_id().with("mixer-scrollbar"),
            Sense::click_and_drag(),
        );
        if r.drag_started()
            && let Some(p) = r.interact_pointer_pos()
        {
            if !thumb.contains(p) {
                self.scroll = ((p.x - bar.left() - thumb_w / 2.0) / travel * most).clamp(0.0, most);
            }
            self.scrollbar_grab = Some((p.x, self.scroll));
        }
        if let (Some((from_x, from_scroll)), Some(p)) = (self.scrollbar_grab, r.interact_pointer_pos()) {
            self.scroll = (from_scroll + (p.x - from_x) / travel * most).clamp(0.0, most);
        }
        if r.drag_stopped() {
            self.scrollbar_grab = None;
        }
        let p = ui.painter_at(bar);
        p.rect_filled(bar, 0.0, theme.bg);
        p.rect_filled(thumb, 3.0, theme.line_strong);
    }

    /// One strip in `r`, drawn inside `row`.
    #[allow(clippy::too_many_arguments)]
    fn strip(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        p: &egui::Painter,
        r: Rect,
        row: Rect,
        strip: &Strip,
        roles: &[(&str, Color32)],
    ) -> Vec<StripEvent> {
        let mut events = Vec::new();
        let id = ui.scope_id().with(("mixer-strip", strip.id));
        let inner = r.shrink2(vec2(3.0, 0.0));
        // The strip itself: a click works on this track, a right-click
        // chooses its role.
        let whole = ui.interact(inner.intersect(row), id, Sense::click());
        if whole.clicked() {
            events.push(StripEvent::Select);
        }
        whole.context_menu(|ui| {
            ui.label(egui::RichText::new(strip.name).strong());
            ui.label("Role");
            for (i, (name, colour)) in roles.iter().enumerate() {
                let text = egui::RichText::new(format!("● {name}")).color(*colour);
                if ui.radio(strip.role == Some(i), text).clicked() {
                    events.push(StripEvent::Role(i));
                    ui.close();
                }
            }
        });
        let fill = if strip.selected { theme.raised } else { theme.panel };
        p.rect_filled(inner, 4.0, fill);
        if strip.selected {
            p.rect_stroke(
                inner,
                4.0,
                Stroke::new(1.0, theme.text_secondary),
                egui::StrokeKind::Inside,
            );
        }
        let role = strip.role.and_then(|i| roles.get(i));
        let colour = role.map_or(theme.line_strong, |(_, c)| *c);
        p.rect_filled(
            Rect::from_min_size(inner.min + vec2(6.0, 4.0), vec2(inner.width() - 12.0, 3.0)),
            1.5,
            colour,
        );
        // Its names.
        let names = Rect::from_min_size(inner.min + vec2(6.0, 10.0), vec2(inner.width() - 12.0, 15.0));
        let short = p.layout_no_wrap(strip.short.to_owned(), crate::fonts::mono(11.0), theme.text);
        let short_w = short.size().x;
        p.galley(
            pos2(names.left(), names.center().y - short.size().y / 2.0),
            short,
            theme.text,
        );
        let name_room = names.width() - short_w - 4.0;
        let name = p.layout(
            strip.name.to_owned(),
            FontId::proportional(11.0),
            theme.text_secondary,
            f32::INFINITY,
        );
        let name = if name.size().x > name_room {
            let mut job = egui::text::LayoutJob::simple_singleline(
                strip.name.to_owned(),
                FontId::proportional(11.0),
                theme.text_secondary,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(name_room.max(8.0));
            ui.fonts_mut(|f| f.layout_job(job))
        } else {
            name
        };
        p.galley(
            pos2(
                names.left() + short_w + 4.0,
                names.center().y - name.size().y / 2.0,
            ),
            name,
            theme.text_secondary,
        );
        // Effect tags, two to a row.
        let tags_top = names.bottom() + 4.0;
        let tag_w = (inner.width() - 12.0 - 4.0) / 2.0;
        for (i, tag) in strip.tags.iter().enumerate().take(4) {
            let (col, line) = ((i % 2) as f32, (i / 2) as f32);
            let t = Rect::from_min_size(
                pos2(inner.left() + 6.0 + col * (tag_w + 4.0), tags_top + line * 18.0),
                vec2(tag_w, 15.0),
            );
            let tr = ui
                .interact(t.intersect(row), id.with(("tag", i)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(tag.tip);
            if tr.clicked() {
                events.push(StripEvent::Tag(i));
            }
            p.rect_filled(t, 3.0, if tr.hovered() { theme.active } else { theme.bg });
            p.text(
                t.center(),
                Align2::CENTER_CENTER,
                tag.name,
                FontId::proportional(10.0),
                theme.text,
            );
        }
        // The meter, which is also the fader.
        let meter_top = tags_top + 2.0 * 18.0 + 6.0;
        let bottom_rows = 52.0;
        let meter = Rect::from_min_max(
            pos2(inner.center().x - 6.0, meter_top),
            pos2(inner.center().x + 14.0, inner.bottom() - bottom_rows),
        );
        let state = self.states.entry(strip.id).or_insert(StripState {
            shown: [BOTTOM_DB; 2],
        });
        let peaks = strip.levels.map_or([f32::NEG_INFINITY; 2], |l| l.peak_db);
        for (ch, peak) in peaks.into_iter().enumerate() {
            state.shown[ch] = falling(state.shown[ch], peak);
        }
        let shown = state.shown;
        if meter.height() >= 24.0 {
            for db in [0.0, -12.0, -48.0] {
                let y = y_of(meter, db);
                p.text(
                    pos2(meter.left() - 4.0, y),
                    Align2::RIGHT_CENTER,
                    format!("{db:.0}"),
                    FontId::monospace(8.5),
                    theme.faint,
                );
                p.line_segment(
                    [pos2(meter.left() - 2.0, y), pos2(meter.right() + 2.0, y)],
                    Stroke::new(1.0, theme.line),
                );
            }
            for (ch, level) in shown.iter().enumerate() {
                let lane = Rect::from_min_max(
                    pos2(meter.left() + ch as f32 * 10.0, meter.top()),
                    pos2(meter.left() + ch as f32 * 10.0 + 8.0, meter.bottom()),
                );
                p.rect_filled(lane, 2.0, theme.bg);
                for (from, to, c) in [
                    (BOTTOM_DB, -6.0, crate::status::GOOD),
                    (-6.0, 0.0, crate::status::WARNING),
                    (0.0, TOP_DB, crate::status::ERROR),
                ] {
                    let top = level.min(to);
                    if top > from {
                        p.rect_filled(
                            Rect::from_min_max(
                                pos2(lane.left(), y_of(meter, top)),
                                pos2(lane.right(), y_of(meter, from)),
                            ),
                            1.0,
                            c,
                        );
                    }
                }
            }
            let gain = match self.drag {
                Some((t, db)) if t == strip.id => db,
                _ => strip.gain_db,
            };
            let y = y_of(meter, gain);
            let handle = Rect::from_center_size(pos2(meter.center().x, y), vec2(meter.width() + 14.0, 8.0));
            p.rect_filled(handle, 3.0, theme.text);
            p.line_segment(
                [pos2(handle.left() + 3.0, y), pos2(handle.right() - 3.0, y)],
                Stroke::new(1.0, theme.bg),
            );
            let fr = ui
                .interact(
                    meter.expand2(vec2(8.0, 4.0)).intersect(row),
                    id.with("fader"),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeVertical)
                .on_hover_text(format!(
                    "{}: drag for its volume, double-click for 0 dB. The bars are its left and right peaks.",
                    strip.name
                ));
            if fr.double_clicked() {
                self.drag = None;
                events.push(StripEvent::Gain { db: 0.0, done: true });
            } else if fr.dragged()
                && let Some(at) = fr.interact_pointer_pos()
            {
                let db = settled(db_at(meter, at.y));
                self.drag = Some((strip.id, db));
                events.push(StripEvent::Gain { db, done: false });
            } else if fr.drag_stopped()
                && let Some((_, db)) = self.drag.take()
            {
                events.push(StripEvent::Gain { db, done: true });
            }
        }
        // The exact volume: click to type.
        let gain = match self.drag {
            Some((t, db)) if t == strip.id => db,
            _ => strip.gain_db,
        };
        let db_box = Rect::from_min_size(
            pos2(inner.left() + 6.0, inner.bottom() - bottom_rows + 6.0),
            vec2(inner.width() - 12.0, 18.0),
        );
        match self.typing.as_mut().filter(|(t, _)| *t == strip.id) {
            Some((_, text)) => {
                let edit = ui.put(
                    db_box,
                    egui::TextEdit::singleline(text)
                        .font(crate::fonts::mono(11.0))
                        .align(Align2::CENTER_CENTER),
                );
                edit.request_focus();
                let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if entered || edit.lost_focus() {
                    if !escaped && let Some(db) = read_db(text) {
                        events.push(StripEvent::Gain { db, done: true });
                    }
                    self.typing = None;
                }
            }
            None => {
                let br = ui
                    .interact(db_box.intersect(row), id.with("db"), Sense::click())
                    .on_hover_cursor(CursorIcon::Text)
                    .on_hover_text("Click to type the volume");
                p.rect_stroke(
                    db_box,
                    3.0,
                    Stroke::new(
                        1.0,
                        if strip.selected {
                            theme.text_secondary
                        } else {
                            theme.line_strong
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                p.text(
                    db_box.center(),
                    Align2::CENTER_CENTER,
                    db_words(gain),
                    crate::fonts::mono(11.0),
                    theme.text,
                );
                if br.clicked() {
                    self.typing = Some((strip.id, format!("{gain:.1}")));
                }
            }
        }
        // Mute and solo.
        let buttons_top = db_box.bottom() + 5.0;
        let half = (inner.width() - 12.0 - 4.0) / 2.0;
        for (i, (label, on, colour)) in [
            ("M", strip.muted, crate::status::WARNING),
            ("S", strip.solo, crate::status::GOOD),
        ]
        .into_iter()
        .enumerate()
        {
            let b = Rect::from_min_size(
                pos2(inner.left() + 6.0 + i as f32 * (half + 4.0), buttons_top),
                vec2(half, 18.0),
            );
            let what = if i == 0 { "Mute" } else { "Solo" };
            let br = ui
                .interact(b.intersect(row), id.with(label), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!("{what} {}", strip.name));
            if br.clicked() {
                events.push(if i == 0 {
                    StripEvent::Mute(!on)
                } else {
                    StripEvent::Solo(!on)
                });
            }
            p.rect_filled(b, 3.0, if on { colour } else { theme.bg });
            p.text(
                b.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(11.0),
                if on {
                    theme.accent_ink
                } else {
                    theme.text_secondary
                },
            );
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_strips_in_view_are_drawn() {
        assert_eq!(in_view(0.0, 400.0, 30), (0, 5));
        assert_eq!(in_view(STRIP_WIDTH * 10.5, 400.0, 30), (10, 16));
        assert_eq!(in_view(0.0, 4_000.0, 30), (0, 30));
        assert_eq!(most_scroll(400.0, 30), 30.0 * STRIP_WIDTH - 400.0);
        assert_eq!(most_scroll(4_000.0, 3), 0.0);
    }

    #[test]
    fn typed_volumes_are_read_in_range() {
        assert_eq!(read_db("-6"), Some(-6.0));
        assert_eq!(read_db("−6.5 dB"), Some(-6.5));
        assert_eq!(read_db("+3"), Some(3.0));
        assert_eq!(read_db("40"), Some(TOP_DB));
        assert_eq!(read_db("Off"), Some(BOTTOM_DB));
        assert_eq!(read_db("loud"), None);
        assert_eq!(db_words(-8.0), "−8.0 dB");
        assert_eq!(settled(0.7), 0.0);
        assert_eq!(settled(-6.3), -6.5);
    }

    /// Draws `strips` once in a 400 × 400 row with `events`.
    fn frame(
        mixer: &mut Mixer,
        strips: &[Strip],
        events: Vec<egui::Event>,
        ctx: &egui::Context,
    ) -> Vec<(u64, StripEvent)> {
        let mut got = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 400.0))),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 400.0));
            got = mixer.show(
                ui,
                &Theme::ASH,
                rect,
                strips,
                &[("Dialogue", crate::theme::role::DIALOGUE)],
            );
        });
        output.drop_without_applying_deltas();
        got
    }

    #[test]
    fn mute_and_the_fader_answer_on_the_strip_clicked() {
        let names: Vec<String> = (0..30).map(|i| format!("A{}", i + 1)).collect();
        let strips: Vec<Strip> = names
            .iter()
            .enumerate()
            .map(|(i, n)| Strip {
                id: i as u64,
                short: n,
                name: "Track",
                role: Some(0),
                gain_db: -6.0,
                muted: false,
                solo: false,
                selected: i == 0,
                levels: None,
                tags: &[],
            })
            .collect();
        let ctx = egui::Context::default();
        let mut mixer = Mixer::default();
        frame(&mut mixer, &strips, Vec::new(), &ctx);
        assert_eq!(mixer.states.len(), 5, "only the strips in view keep levels");
        // The second strip's M: 84 + 3 + 6, near the bottom (390 - 52 + 6 + 18 + 5).
        let at = pos2(STRIP_WIDTH + 12.0, 400.0 - SCROLLBAR - 52.0 + 30.0 + 5.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut mixer, &strips, vec![egui::Event::PointerMoved(at)], &ctx);
        let got = frame(&mut mixer, &strips, vec![press(true), press(false)], &ctx);
        assert_eq!(got, [(1, StripEvent::Mute(true))]);
    }
}
