//! Drawing the clips of the timeline's tracks: each clip with its
//! pictures, waveform, fades, speed, level line, volume and pan lines,
//! marks and name; clips too thin to tell apart merged into one shape;
//! transitions, a selected gap and a track's own line over its row; a clip
//! dragged past the last track in the lit strip of a new one; and where
//! something dragged in from a list would land.

use super::{
    COMPACT_TRACK, ClipPictures, ClipShows, ClipWaveform, Half, Layout, LineOwner, NEW_TRACK_STRIP, Rows,
    TimelineClip, TimelineData, TimelineTrack, TimelineView, Tool,
    clip_input::{ClipDrag, DragKind, Landing},
    empty_end, fades, find_clip, gaps, keyframes, level, loading, slip, speed, transitions, wave_look,
};
use crate::Theme;
use egui::{Align2, Color32, FontId, Rect, Stroke, Ui, pos2, vec2};

/// How a waveform is drawn: its track's colour, dimmed when the track is
/// muted, and how much louder or quieter than recorded (the clip's volume
/// times its track's).
#[derive(Debug, Clone, Copy)]
struct WaveLook {
    colour: Color32,
    muted: bool,
    gain: f32,
}

/// Colours of audio tracks, one per track in turn.
pub(super) const AUDIO_COLOURS: [Color32; 6] = [
    Color32::from_rgb(0x3E, 0x8E, 0x7E),
    Color32::from_rgb(0x5B, 0x7F, 0xC4),
    Color32::from_rgb(0x9A, 0x6C, 0xC4),
    Color32::from_rgb(0xC4, 0x7A, 0x4F),
    Color32::from_rgb(0x6F, 0x9E, 0x4C),
    Color32::from_rgb(0xB8, 0x5C, 0x7A),
];

/// How the clips of one track are drawn.
#[derive(Debug, Clone, Copy)]
pub(super) struct TrackLook {
    /// The track's colour: the audio track's own, or the one of video clips.
    pub colour: Color32,
    pub audio: bool,
    pub muted: bool,
    /// The track's volume as a gain (following its fader while it moves).
    pub gain: f32,
    /// The clips show their level line.
    pub level_line: bool,
    /// Pictures, waveforms and lines are drawn (not on compact tracks).
    pub full: bool,
    /// Left of the time area: where the track names end.
    pub left: f32,
}

/// One track's row in its area, as it is drawn.
pub(super) struct TrackRow<'d> {
    pub track: &'d TimelineTrack,
    /// Every track of its area (a clip dragged from another one is drawn here).
    pub tracks: &'d [TimelineTrack],
    pub row: Rect,
    /// The row right of the track names, to the area's right edge.
    pub band: Rect,
    pub area: Rect,
}

impl TimelineView {
    /// How sound is drawn in a clip of a track of `colour`, as the settings say.
    fn wave_look(&self, colour: Color32, muted: bool) -> wave_look::WaveLook {
        wave_look::WaveLook {
            style: self.wave_style,
            colouring: self.wave_colouring,
            side: self.wave_side,
            colour,
            muted,
        }
    }

    /// The clips of one track in its row, and what lies over them: the
    /// transitions, a selected gap, the track's own line, dimming when it
    /// is hidden. `elsewhere`: a clip being dragged to another track.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_track_clips(
        &self,
        ui: &Ui,
        theme: &Theme,
        data: &TimelineData,
        clip_painter: &egui::Painter,
        at: &TrackRow,
        look: TrackLook,
        elsewhere: Option<(ClipDrag, Landing)>,
        visible: (f64, f64),
    ) {
        let TrackRow {
            track,
            tracks,
            row,
            band,
            area,
        } = *at;
        let row = &row;
        let TrackLook {
            colour,
            audio,
            full,
            left,
            ..
        } = look;
        let inset = super::track_views::clip_inset(self.compact);
        // Clips a few pixels wide can't be told apart: neighbours
        // merge into one shape, so zooming far out stays cheap.
        let mut merged: Option<Rect> = None;
        let leaving = elsewhere
            .filter(|(d, _)| d.track == track.id)
            .map(|(d, _)| d.clip);
        let arriving = elsewhere
            .filter(|(_, over)| *over == Landing::Track(track.id))
            .and_then(|(d, _)| find_clip(tracks, d.clip));
        let shown = track.clips.iter().filter(|c| Some(c.id) != leaving);
        for clip in shown.chain(arriving) {
            let (start, duration) = match &self.clip_drag {
                Some(d) if d.moves(clip) => d.shown(clip),
                _ => (clip.start, clip.duration),
            };
            // Being slipped: its pictures, waveform and marks move under it.
            let slipping = self
                .clip_drag
                .filter(|d| d.kind == DragKind::Slip && d.moves(clip));
            let slipped = slipping.map(|d| slip::shifted(clip, d.delta));
            let clip = slipped.as_ref().unwrap_or(clip);
            if start + duration < visible.0 || start > visible.1 {
                continue;
            }
            let r = self.clip_rect(left, start, duration, row.top() + inset, row.bottom() - inset);
            if r.width() < 4.0 && !clip.selected && !clip.missing {
                merged = match merged {
                    Some(m) if r.left() - m.right() < 2.0 => Some(m.union(r)),
                    Some(m) => {
                        clip_painter.rect_filled(m, 1.0, tiny_fill(theme, colour, audio));
                        Some(r)
                    }
                    None => Some(r),
                };
                continue;
            }
            if let Some(m) = merged.take() {
                clip_painter.rect_filled(m, 1.0, tiny_fill(theme, colour, audio));
            }
            self.draw_clip(
                ui,
                theme,
                data.fps,
                clip_painter,
                look,
                clip,
                r,
                start,
                duration,
                slipping,
            );
        }
        if let Some(m) = merged {
            clip_painter.rect_filled(m, 1.0, tiny_fill(theme, colour, audio));
        }
        // Picture transitions, and crossfades between sound clips.
        self.draw_transitions(theme, clip_painter, track, *row, left);
        if let Some(gap) = self.gap.filter(|g| g.track == track.id) {
            let (x0, x1) = (self.x_of(left, gap.start), self.x_of(left, gap.end));
            gaps::draw(clip_painter, *row, x0, x1, theme.accent);
        }
        if audio && full {
            let row_band = Rect::from_min_max(
                pos2(left, row.top() + inset),
                pos2(area.right(), row.bottom() - inset),
            );
            self.draw_line(
                ui,
                clip_painter,
                LineOwner::Track(track.id),
                track.line.as_deref(),
                row_band,
                |t| self.x_of(left, t),
            );
        }
        if track.hidden {
            // Not in the picture: its clips are dimmed.
            clip_painter.rect_filled(band.shrink2(vec2(0.0, 1.0)), 0.0, theme.bg.gamma_multiply(0.6));
        }
    }

    /// One clip drawn whole in `r`, as it is shown from `start` for
    /// `duration` (moved, trimmed or slipped while it is dragged).
    #[allow(clippy::too_many_arguments)]
    fn draw_clip(
        &self,
        ui: &Ui,
        theme: &Theme,
        fps: f64,
        clip_painter: &egui::Painter,
        look: TrackLook,
        clip: &TimelineClip,
        r: Rect,
        start: f64,
        duration: f64,
        slipping: Option<ClipDrag>,
    ) {
        let TrackLook {
            colour,
            audio,
            muted,
            gain: track_gain,
            level_line,
            full,
            left,
        } = look;
        match audio {
            true => wave_look::draw_body(clip_painter, r, self.wave_look(colour, muted)),
            false => draw_clip_body(clip_painter, theme, r, colour, audio),
        }
        // With both (the rough cut view), pictures fill the top
        // and the sound the bottom of the clip.
        let (pictures, waveform) = match full {
            true => (clip.pictures.as_ref(), clip.waveform.as_ref()),
            false => (None, None),
        };
        let (pic_r, wave_r) = parts(r, pictures.is_some() && waveform.is_some());
        if let Some(pictures) = pictures {
            self.draw_pictures(clip_painter, pic_r, clip, start, pictures);
        }
        // The clip's volume: the one its level line is being dragged to.
        let dragged_db = self.level_drag.filter(|d| d.clip == clip.id).map(|d| d.db);
        let clip_gain = dragged_db.map_or(clip.sound.gain, level::gain_of_db);
        if let Some(w) = waveform {
            let wave_colour = if audio { colour } else { AUDIO_COLOURS[0] };
            let look = WaveLook {
                colour: wave_colour,
                muted,
                gain: clip_gain * track_gain,
            };
            self.draw_waveform(clip_painter, wave_r, clip, start, w, look);
        }
        // The fades of the kind its track has (a sound's on audio tracks, a
        // picture's on video tracks), as they are being dragged.
        let dragged = self.fade_drag.filter(|d| d.clip == clip.id);
        let (fade_r, own_fades) = match audio {
            true => (wave_r, dragged.map_or(clip.sound.fades, |d| d.now)),
            false => (pic_r, dragged.map_or(clip.fades, |d| d.now)),
        };
        if full {
            fades::draw(clip_painter, fade_r, &own_fades, audio, self.zoom);
            if !audio && waveform.is_some() && pictures.is_some() {
                // The rough cut view: its sound's fades under its pictures.
                fades::draw(clip_painter, wave_r, &clip.sound.fades, true, self.zoom);
            }
        }
        if full && let Some(curve) = &clip.speed_curve {
            speed::draw(clip_painter, r, curve, theme.accent);
        }
        if level_line {
            let db = dragged_db.unwrap_or_else(|| level::db_of_gain(clip.sound.gain));
            let hovered = self.level_drag.is_none()
                && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                    r.contains(p) && clip_painter.clip_rect().contains(p) && level::grabs(r, db, p.y)
                });
            let label_x = dragged_db
                .and_then(|_| ui.input(|i| i.pointer.hover_pos()))
                .map(|p| p.x);
            level::draw(clip_painter, r, db, hovered || dragged_db.is_some(), label_x);
        }
        if audio && full {
            let shift = start - clip.start;
            self.draw_line(
                ui,
                clip_painter,
                LineOwner::Clip(clip.id),
                clip.line.as_deref(),
                r,
                |t| self.x_of(left, t + shift),
            );
        }
        if let Some(until) = clip.plays_until.filter(|u| *u < start + duration) {
            empty_end::draw(clip_painter, theme, r, self.x_of(left, until));
        }
        draw_clip_marks(
            clip_painter,
            theme,
            r,
            clip,
            pictures.is_some() || waveform.is_some(),
        );
        if let (Some(d), Some(room)) = (slipping.filter(|d| d.clip == clip.id), clip.slip_room) {
            slip::draw_room(clip_painter, theme, r, room, d.delta);
        }
        if let Some(loading) = &clip.loading {
            loading::draw_on_clip(clip_painter, theme, r, loading);
        }
        if self.clip_drag.is_some_and(|d| d.refused && d.moves(clip)) {
            draw_refused(clip_painter, r);
        }
        // Fade handles show on the clip under the pointer and selected
        // ones, over its name.
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let handles = full
            && self.clip_drag.is_none()
            && matches!(self.tool, Tool::Select | Tool::Ripple)
            && (clip.selected
                || dragged.is_some()
                || pointer.is_some_and(|p| r.contains(p) && clip_painter.clip_rect().contains(p)));
        if handles {
            fades::draw_handles(clip_painter, fade_r, &own_fades, audio, self.zoom);
        }
        if full && !clip.keyframes.is_empty() {
            // On top of everything else in the clip, where it is
            // shown (also while it is dragged).
            let shift = start - clip.start;
            let x_of = |t: f64| self.x_of(left, t + shift);
            let chosen = self.chosen_marks.of(clip.id, &clip.keyframes, fps);
            let look = keyframes::MarksLook {
                colour: theme.accent,
                chosen: &chosen,
                drag: self.mark_drag.as_ref().filter(|d| d.clip == clip.id),
                fps,
            };
            // A box being dragged goes under the marks it chooses.
            if let Some(chosen_box) = self.mark_box.as_ref().filter(|b| b.clip == clip.id) {
                keyframes::draw_box(clip_painter, r, chosen_box, x_of, theme.accent);
            }
            keyframes::draw(clip_painter, r, &clip.keyframes, x_of, look);
        }
    }

    /// The transitions across the cuts of a track's `row`: a video track's
    /// picture transitions or an audio track's crossfades.
    fn draw_transitions(
        &self,
        theme: &Theme,
        clip_painter: &egui::Painter,
        track: &TimelineTrack,
        row: Rect,
        left: f32,
    ) {
        let row = &row;
        for clip in &track.clips {
            if let Some(length) = clip.transition_in {
                let (x0, x1) = (
                    self.x_of(left, clip.start - length / 2.0),
                    self.x_of(left, clip.start + length / 2.0),
                );
                if !transitions::in_view(x0, x1, clip_painter.clip_rect()) {
                    continue;
                }
                transitions::draw(
                    clip_painter,
                    *row,
                    x0,
                    x1,
                    theme.text,
                    theme.panel.gamma_multiply(0.85),
                );
            }
        }
    }

    /// A clip dragged past the last track of an area: a lit strip where
    /// the new track would be, with the clip in it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_new_track(
        &self,
        ui: &Ui,
        theme: &Theme,
        layout: &Layout,
        (clip_painter, header_painter): (&egui::Painter, &egui::Painter),
        half: Half,
        area: Rect,
        rows: &Rows,
        drag: ClipDrag,
        clip: &TimelineClip,
    ) {
        let (left, audio) = (layout.time_left(), half == Half::Audio);
        let inset = super::track_views::clip_inset(self.compact);
        let usual = match self.compact {
            true => COMPACT_TRACK,
            false => self.usual_height(half),
        };
        let strip = new_track_strip(area, rows, usual, half);
        let mark = ui.visuals().selection.stroke.color;
        let lit = Rect::from_min_max(pos2(left, strip.top()), strip.max);
        clip_painter.rect_filled(lit, 0.0, mark.gamma_multiply(0.3));
        let edge = match half {
            Half::Video => strip.top(),
            Half::Audio => strip.bottom(),
        };
        ui.painter_at(area).line_segment(
            [pos2(area.left(), edge), pos2(area.right(), edge)],
            Stroke::new(2.0, mark),
        );
        let header = Rect::from_min_max(strip.min, pos2(left, strip.bottom()));
        header_painter.rect_filled(header, 0.0, theme.panel);
        header_painter.rect_filled(header, 0.0, mark.gamma_multiply(0.3));
        header_painter.text(
            pos2(layout.header_x + super::headers::NAME_LEFT, strip.center().y),
            Align2::LEFT_CENTER,
            "New track",
            FontId::proportional(11.5),
            theme.text,
        );
        let (start, duration) = drag.shown(clip);
        let r = self.clip_rect(left, start, duration, strip.top() + inset, strip.bottom() - inset);
        let colour = if audio {
            AUDIO_COLOURS[0]
        } else {
            theme.line_strong
        };
        draw_clip_body(clip_painter, theme, r, colour, audio);
        draw_clip_marks(clip_painter, theme, r, clip, false);
    }

    /// Where something dragged in from a list would land: the cut a
    /// transition would go onto, the clip an effect would, the place and
    /// video track of a file.
    pub(super) fn draw_drop_marks(&self, ui: &Ui, layout: &Layout, video_rows: &Rows) {
        let left = layout.time_left();
        // A transition dragged over a cut: the cut it would go onto.
        if let Some((row, x)) = self.transition_over {
            let tracks = Rect::from_min_max(pos2(left, layout.video.top()), layout.audio.max);
            let mark = ui.visuals().selection.stroke.color;
            transitions::draw_landing(&ui.painter_at(tracks), row, x, mark);
        }
        // An effect dragged over a clip: the clip it would go onto.
        if let Some(clip) = self.effect_over {
            let tracks = Rect::from_min_max(pos2(left, layout.video.top()), layout.audio.max);
            let mark = ui.visuals().selection.stroke.color;
            ui.painter_at(tracks)
                .rect_stroke(clip, 4.0, Stroke::new(2.0, mark), egui::StrokeKind::Inside);
        }
        // A file dragged over the timeline: where it would land.
        if let Some((at, row)) = self.file_over {
            let tracks = Rect::from_min_max(pos2(left, layout.video.top()), layout.audio.max);
            let mark = ui.visuals().selection.stroke.color;
            let x = self.x_of(left, at);
            let p = ui.painter_at(tracks);
            if let Some(row) = row.and_then(|row| video_rows.rects().get(row)) {
                let band = Rect::from_min_max(pos2(left, row.top()), row.max).intersect(layout.video);
                p.rect_filled(band, 0.0, mark.gamma_multiply(0.15));
            }
            p.line_segment(
                [pos2(x, tracks.top()), pos2(x, tracks.bottom())],
                Stroke::new(2.0, mark),
            );
        }
    }

    /// Pictures side by side across the clip, each the one nearest its
    /// place in the file. Only the visible ones are drawn.
    fn draw_pictures(
        &self,
        p: &egui::Painter,
        r: Rect,
        clip: &TimelineClip,
        shown_start: f64,
        pics: &ClipPictures,
    ) {
        let area = Rect::from_min_max(
            pos2(r.left() + 1.0, r.top() + 3.0),
            pos2(r.right() - 1.0, r.bottom() - 1.0),
        );
        let visible = p.clip_rect().intersect(area);
        if visible.width() < 1.0 || pics.textures.is_empty() || pics.every <= 0.0 {
            return;
        }
        let tile = (area.height() * pics.aspect).max(8.0);
        let left = r.left() - ((shown_start - clip.start) * f64::from(self.zoom)) as f32;
        let painter = p.with_clip_rect(visible);
        let first = ((visible.left() - area.left()) / tile).floor().max(0.0);
        let mut x = area.left() + first * tile;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        while x < visible.right() {
            let t = pics.file_at(f64::from(x + tile / 2.0 - left) / f64::from(self.zoom));
            let i = ((t / pics.every).round().max(0.0) as usize).min(pics.textures.len() - 1);
            let rect = Rect::from_min_size(pos2(x, area.top()), vec2(tile, area.height()));
            painter.image(pics.textures[i], rect, uv, Color32::from_white_alpha(230));
            x += tile;
        }
    }

    /// The clip's loudness as one bar per pixel column, centred in the
    /// clip, as loud as `look` says (never taller than the clip).
    fn draw_waveform(
        &self,
        p: &egui::Painter,
        r: Rect,
        clip: &TimelineClip,
        shown_start: f64,
        w: &ClipWaveform,
        look: WaveLook,
    ) {
        let WaveLook { colour, muted, gain } = look;
        // The clip's whole height (its name sits on a dark pill over the
        // sound), clear of the edge and, with the loudness map, its coloured edge.
        let inset = if self.wave_colouring == wave_look::WaveColouring::Loudness {
            4.0
        } else {
            1.0
        };
        let room = Rect::from_min_max(
            pos2(r.left() + inset, r.top() + 3.0),
            pos2(r.right() - 1.0, r.bottom() - 3.0),
        );
        let clip_area = p.clip_rect().intersect(room);
        if clip_area.width() < 1.0 || clip_area.height() < 3.0 || w.peaks.is_empty() {
            return;
        }
        let left = r.left() - ((shown_start - clip.start) * f64::from(self.zoom)) as f32;
        let drawn = self.wave_look(colour, muted);
        let scale = match self.wave_fitted {
            true => fitted_scale(p.ctx(), w),
            false => wave_look::WaveScale::straight(),
        };
        let levels = crate::waveform::levels_for(p.ctx(), w.id, &w.peaks);
        let sides = w
            .sides
            .as_ref()
            .filter(|(up, down)| up.len() == w.peaks.len() && down.len() == w.peaks.len());
        let side_levels = sides.map(|(up, down)| {
            (
                crate::waveform::levels_for(p.ctx(), w.id.with("up"), up),
                crate::waveform::levels_for(p.ctx(), w.id.with("down"), down),
            )
        });
        let mut columns = Vec::with_capacity(clip_area.width() as usize + 1);
        let mut x = clip_area.left().floor();
        while x < clip_area.right() {
            let t = w.file_at(f64::from(x - left) / f64::from(self.zoom));
            let t_next = w.file_at(f64::from(x + 1.0 - left) / f64::from(self.zoom));
            let a = (t * f64::from(w.per_second)).max(0.0) as usize;
            let b = ((t_next * f64::from(w.per_second)).ceil() as usize)
                .max(a + 1)
                .min(w.peaks.len());
            if a >= w.peaks.len() {
                break;
            }
            let peak = crate::waveform::loudest(&w.peaks, levels.as_deref(), a, b) * gain;
            let (up, down) = match (sides, &side_levels) {
                (Some((up, down)), Some((up_levels, down_levels))) => (
                    crate::waveform::loudest(up, up_levels.as_deref(), a, b) * gain,
                    crate::waveform::loudest(down, down_levels.as_deref(), a, b) * gain,
                ),
                _ => (peak, peak),
            };
            columns.push(wave_look::Column {
                x,
                peak,
                body: wave_look::body(&w.peaks, a, b) * gain,
                up,
                down,
            });
            x += 1.0;
        }
        // Laid out on the clip's whole room even where only part shows, so
        // the scale is the same however far it is scrolled.
        let area = Rect::from_min_max(
            pos2(clip_area.left(), room.top()),
            pos2(clip_area.right(), room.bottom()),
        );
        wave_look::draw(&p.with_clip_rect(clip_area), area, &columns, drawn, scale);
    }
}

/// Where the lit strip for a new track goes: past the last row of `rows`
/// (above the top video track, under the bottom audio track), as tall as a
/// track usually is where `area` has that much room, and over the edge of
/// the area where it has none.
pub(super) fn new_track_strip(area: Rect, rows: &Rows, usual: f32, half: Half) -> Rect {
    // Never taller than a track usually is (compact tracks are shorter).
    let least = NEW_TRACK_STRIP.min(usual);
    let (top, bottom) = match (half, rows.rects().last()) {
        (Half::Video, last) => {
            let edge = last.map_or(area.bottom(), |row| row.top());
            let top = (edge - usual).max(area.top());
            if edge - top >= least {
                (top, edge)
            } else {
                (area.top(), area.top() + least)
            }
        }
        (Half::Audio, last) => {
            let edge = last.map_or(area.top(), |row| row.bottom());
            let bottom = (edge + usual).min(area.bottom());
            if bottom - edge >= least {
                (edge, bottom)
            } else {
                (area.bottom() - least, area.bottom())
            }
        }
    };
    Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), bottom))
}

/// Marks a dragged clip that could not be put where it is now.
fn draw_refused(p: &egui::Painter, r: Rect) {
    p.rect_filled(r, 3.0, crate::status::ERROR.gamma_multiply(0.35));
    p.rect_stroke(
        r,
        3.0,
        Stroke::new(1.5, crate::status::ERROR),
        egui::StrokeKind::Inside,
    );
}

/// The scale fitted to a file's sound, worked out once per file and stream
/// (and again while its peaks are still being read).
fn fitted_scale(ctx: &egui::Context, w: &ClipWaveform) -> wave_look::WaveScale {
    type Fits = std::collections::HashMap<egui::Id, (usize, wave_look::WaveScale)>;
    let key = egui::Id::unique("waveform fitted scales");
    let kept = ctx.data(|d| {
        d.get_temp::<std::sync::Arc<Fits>>(key)
            .and_then(|fits| fits.get(&w.id).copied())
    });
    match kept {
        Some((len, scale)) if len == w.peaks.len() => scale,
        _ => {
            let scale = wave_look::WaveScale::fit(&w.peaks);
            ctx.data_mut(|d| {
                let fits = d.get_temp_mut_or_default::<std::sync::Arc<Fits>>(key);
                std::sync::Arc::make_mut(fits).insert(w.id, (w.peaks.len(), scale));
            });
            scale
        }
    }
}

fn tiny_fill(theme: &Theme, colour: Color32, audio: bool) -> Color32 {
    if audio {
        colour.gamma_multiply(0.55)
    } else {
        theme.line_strong
    }
}

fn draw_clip_body(p: &egui::Painter, theme: &Theme, r: Rect, colour: Color32, audio: bool) {
    let fill = if audio {
        colour.gamma_multiply(0.55)
    } else {
        theme.raised
    };
    p.rect_filled(r, 4.0, fill);
    if !audio {
        // Video clips share one fill; the top edge carries the label colour.
        p.rect_filled(
            Rect::from_min_max(r.min, pos2(r.right(), r.top() + 3.0)),
            2.0,
            colour,
        );
    }
}

/// What goes over a clip's contents: missing and selected outlines, and its
/// name (on a dark pill over `pictures`). In a short clip (compact tracks)
/// the name is centred from top to bottom.
fn draw_clip_marks(p: &egui::Painter, theme: &Theme, r: Rect, clip: &TimelineClip, pictures: bool) {
    if clip.missing {
        p.rect_stroke(
            r,
            4.0,
            Stroke::new(1.5, crate::status::ERROR),
            egui::StrokeKind::Inside,
        );
    }
    draw_what_it_shows(p, theme, r, clip.shows);
    if clip.graded && r.width() > 16.0 && r.height() > 14.0 {
        p.circle_filled(pos2(r.left() + 7.0, r.bottom() - 7.0), 3.0, theme.accent);
    }
    if clip.selected {
        p.rect_stroke(r, 4.0, Stroke::new(2.0, theme.accent), egui::StrokeKind::Inside);
    }
    if r.width() > 30.0 {
        let text = if clip.missing {
            format!("{} (missing)", clip.name)
        } else {
            clip.name.clone()
        };
        let galley = p.layout_no_wrap(text, FontId::proportional(11.0), theme.text);
        let pos = match r.height() < 24.0 {
            true => pos2(r.left() + 6.0, r.center().y - galley.size().y / 2.0),
            false => pos2(r.left() + 6.0, r.top() + 6.0),
        };
        let inside = p.with_clip_rect(r.shrink2(vec2(2.0, 0.0)));
        if pictures {
            // A dark pill keeps the name readable over the pictures.
            let pill = Rect::from_min_size(pos - vec2(3.0, 1.0), galley.size() + vec2(6.0, 2.0));
            inside.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
        }
        inside.galley(pos, galley, theme.text);
    }
}

/// The mark of a clip that plays no file: light stripes across an
/// adjustment layer, a small stack of two frames in the top right corner
/// of a nested timeline, and a "T" there on a title.
fn draw_what_it_shows(p: &egui::Painter, theme: &Theme, r: Rect, shows: ClipShows) {
    match shows {
        ClipShows::File => {}
        ClipShows::Adjustment => {
            let inside = p.with_clip_rect(r.shrink(1.0).intersect(p.clip_rect()));
            let stroke = Stroke::new(1.0, theme.text_secondary.gamma_multiply(0.25));
            let step = 10.0;
            let mut x = r.left() - r.height();
            while x < r.right() {
                inside.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], stroke);
                x += step;
            }
        }
        ClipShows::Title => {
            let seen = r.intersect(p.clip_rect());
            if seen.width() < 40.0 || seen.height() < 14.0 {
                return;
            }
            p.text(
                pos2(seen.right() - 8.0, seen.top() + 4.0),
                Align2::RIGHT_TOP,
                "T",
                FontId::proportional(12.0),
                theme.text_secondary,
            );
        }
        ClipShows::Timeline => {
            // In the top right corner of the part in view.
            let seen = r.intersect(p.clip_rect());
            if seen.width() < 40.0 || seen.height() < 14.0 {
                return;
            }
            let stroke = Stroke::new(1.2, theme.text_secondary);
            let corner = pos2(seen.right() - 18.0, seen.top() + 6.0);
            let back = Rect::from_min_size(corner + vec2(3.0, 0.0), vec2(10.0, 7.0));
            let front = Rect::from_min_size(corner + vec2(0.0, 3.0), vec2(10.0, 7.0));
            p.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
            p.rect_filled(front, 1.0, theme.raised);
            p.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
        }
    }
}

/// Where a clip drawn in `r` shows its pictures and its sound: with both
/// (the rough cut view) pictures fill the top and the sound the bottom,
/// else each the whole clip.
pub(super) fn parts(r: Rect, both: bool) -> (Rect, Rect) {
    if !both {
        return (r, r);
    }
    let split = r.top() + r.height() * 0.62;
    (
        Rect::from_min_max(r.min, pos2(r.right(), split)),
        Rect::from_min_max(pos2(r.left(), split - 17.0), r.max),
    )
}
