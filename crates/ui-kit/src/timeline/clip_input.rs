//! The pointer over the clips of the video and the audio area: what is
//! under it ([`Targets`]), things dropped from a list, and the drags and
//! clicks of every tool, one named step after another.

use super::{
    ClipEdge, ClipShows, DraggedEffect, DraggedMedia, DraggedTimeline, DraggedTitle, DraggedTransition,
    KeyframeMark, Layout, MarksChange, NEW_TRACK_EDGE, NEW_TRACK_PAST, Rows, STAYS_ON_ITS_TRACK,
    TimelineClip, TimelineData, TimelineEvent, TimelineTrack, TimelineView, Tool, clip_draw, fades,
    find_clip, gaps, keyframes, level,
    menus::{Menu, clip_menu},
    obstacles::Obstacles,
    slip, transitions,
};
use egui::{CursorIcon, Pos2, Rect, Sense, Ui, pos2};

/// One area of clips (the video or the audio tracks) as its input sees it.
#[derive(Clone, Copy)]
struct Area<'d> {
    tracks: &'d [TimelineTrack],
    rows: &'d Rows,
    /// The area right of the track names.
    time: Rect,
    /// The video area (its tracks stack upward; files and transitions land there).
    video: bool,
    /// Left of the time area.
    left: f32,
}

/// Where the pointer is: on screen, and the time under it.
#[derive(Debug, Clone, Copy)]
struct Pointer {
    p: Pos2,
    t: f64,
}

/// What is being dragged in from a list over an area. While a file is
/// dragged here, the list has the pointer: the area is not pointed at,
/// but the file is over it.
#[derive(Debug, Clone, Copy)]
struct Over {
    title: bool,
    /// A file, a timeline or a title: something that lands as a clip.
    file: bool,
    effect: bool,
    /// Only over the video area.
    transition: bool,
}

impl Over {
    fn of(r: &egui::Response, video: bool) -> Over {
        let title = r.dnd_hover_payload::<DraggedTitle>().is_some();
        Over {
            title,
            file: r.dnd_hover_payload::<DraggedMedia>().is_some()
                || r.dnd_hover_payload::<DraggedTimeline>().is_some()
                || title,
            effect: r.dnd_hover_payload::<DraggedEffect>().is_some(),
            transition: video && r.dnd_hover_payload::<DraggedTransition>().is_some(),
        }
    }

    fn any(self) -> bool {
        self.file || self.effect || self.transition
    }
}

/// What lies under the pointer in an area.
#[derive(Clone, Copy)]
struct Targets<'d> {
    /// The track whose row is under it.
    hit_track: Option<&'d TimelineTrack>,
    /// The clip under it.
    hit: Option<&'d TimelineClip>,
    /// The part of that clip: its edges trim, the rest moves.
    zone: Option<DragKind>,
    /// A keyframe mark of that clip: it comes before the clip itself.
    on_mark: Option<(&'d TimelineClip, &'d KeyframeMark)>,
    /// Its level line (where it is drawn, and its volume in dB).
    on_line: Option<(&'d TimelineClip, Rect, f32)>,
    /// A handle or curve point of one of its fades: it comes before
    /// everything else on the clip.
    on_fade: Option<FadeTarget<'d>>,
    /// The strip along its bottom that its marks lie in, away from them.
    on_strip: Option<&'d TimelineClip>,
}

/// A fade's handle or curve point under the pointer.
#[derive(Debug, Clone, Copy)]
struct FadeTarget<'d> {
    clip: &'d TimelineClip,
    /// Where those fades are drawn.
    rect: Rect,
    /// The sound's fades (else the picture's).
    sound: bool,
    end: fades::FadeEnd,
    grip: fades::FadeGrip,
}

impl FadeTarget<'_> {
    fn fades(&self) -> fades::ClipFades {
        match self.sound {
            true => self.clip.sound.fades,
            false => self.clip.fades,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum DragKind {
    Move,
    Trim(ClipEdge),
    RippleTrim(ClipEdge),
    /// The cut after the dragged clip.
    Roll,
    Slip,
    Slide,
}

/// Where a dragged clip would land if let go now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Landing {
    Track(u64),
    /// Past the last track of its kind: on a track to be made there.
    NewTrack,
}

/// A clip being dragged: what is moving and by how much so far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ClipDrag {
    pub(super) clip: u64,
    pub(super) link: Option<u64>,
    /// The track the clip is on.
    pub(super) track: u64,
    /// Where it would land if let go now: another track of its kind, or a
    /// new one, once a move has gone up or down far enough.
    pub(super) over: Landing,
    /// Let go now, the move would be refused: a clip is in the way.
    pub(super) refused: bool,
    pub(super) kind: DragKind,
    /// Pointer time when the drag began.
    pub(super) grabbed_at: f64,
    pub(super) start: f64,
    pub(super) end: f64,
    /// Current offset in seconds, after snapping.
    pub(super) delta: f64,
    /// A move of one of several selected clips: all the selected clips
    /// (and those linked to them) move along their own tracks together.
    pub(super) together: bool,
}

impl ClipDrag {
    pub(super) fn moves(&self, c: &TimelineClip) -> bool {
        c.id == self.clip || (self.link.is_some() && c.link == self.link) || (self.together && c.selected)
    }

    /// Where the dragged clip is drawn instead of on its own track, when
    /// the drag has taken it off it.
    pub(super) fn lands_elsewhere(&self) -> Option<Landing> {
        (self.kind == DragKind::Move && self.over != Landing::Track(self.track)).then_some(self.over)
    }

    /// Whether a clip is in the way of this move, among `data`'s clips: on
    /// the track the dragged clip would land on, or on the track of a clip
    /// linked to it. Linked sound is not counted when the clip changes
    /// track: it then finds a free track by itself. Overlaps shorter than
    /// half a frame are not counted either, as places are rounded to frames.
    /// `room` holds the clips that stay, gathered for this drag.
    pub(super) fn is_in_the_way(&self, data: &TimelineData, room: &Obstacles) -> bool {
        if self.kind != DragKind::Move {
            return false;
        }
        let slack = 0.5 / data.fps.max(1.0);
        let changes_track = self.lands_elsewhere().is_some();
        for (tracks, audio) in [(data.video, false), (data.audio, true)] {
            for (index, track) in tracks.iter().enumerate() {
                for c in track.clips.iter().filter(|c| self.moves(c)) {
                    let (start, end) = (c.start + self.delta, c.start + c.duration + self.delta);
                    let lands_on = if c.id != self.clip {
                        (!(audio && changes_track)).then_some(index)
                    } else {
                        match self.over {
                            Landing::Track(id) => tracks.iter().position(|t| t.id == id),
                            Landing::NewTrack => None,
                        }
                    };
                    if lands_on.is_some_and(|index| room.taken(audio, index, start, end, slack)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Where `c` is drawn while the drag lasts.
    pub(super) fn shown(&self, c: &TimelineClip) -> (f64, f64) {
        let (s, d) = (c.start, c.duration);
        match self.kind {
            DragKind::Move | DragKind::Slide => (s + self.delta, d),
            DragKind::Trim(ClipEdge::Start) => (s + self.delta, (d - self.delta).max(0.0)),
            DragKind::RippleTrim(ClipEdge::Start) => (s, (d - self.delta).max(0.0)),
            DragKind::Trim(ClipEdge::End) | DragKind::RippleTrim(ClipEdge::End) | DragKind::Roll => {
                (s, (d + self.delta).max(0.0))
            }
            DragKind::Slip => (s, d),
        }
    }
}

impl TimelineView {
    /// Clips, in the video and then the audio area: click selects (or
    /// cuts, with the blade); drag moves or trims; things dragged in from
    /// a list land there.
    pub(super) fn clip_areas_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        rows: [&Rows; 2],
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        let left = layout.time_left();
        for (area, tracks, rows, video) in [
            (layout.video, data.video, rows[0], true),
            (layout.audio, data.audio, rows[1], false),
        ] {
            let area = Area {
                tracks,
                rows,
                time: Rect::from_min_max(pos2(left, area.top()), area.max),
                video,
                left,
            };
            self.clip_area_input(ui, data, &area, id, events);
        }
    }

    /// One area of clips, step by step. The order of the steps decides
    /// which wins: something dropped, then a drag begun, followed or let
    /// go, then double-clicks, the right-click menu and clicks.
    fn clip_area_input(
        &mut self,
        ui: &mut Ui,
        data: &TimelineData,
        area: &Area,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) {
        let r = ui.interact(area.time, id.with(("area", area.video)), Sense::click_and_drag());
        let over = Over::of(&r, area.video);
        let menu_marks = self.menu_marks(area.tracks, data.fps);
        let menu = self.area_menu(&menu_marks, area.tracks, data, area.video);
        let file_at = ui.input(|i| i.pointer.interact_pos()).filter(|_| over.any());
        let Some(p) = r.interact_pointer_pos().or(r.hover_pos()).or(file_at) else {
            // The pointer is elsewhere, perhaps on the menu itself: an
            // open menu is still shown, or it would close under it.
            clip_menu(&r, menu, events);
            return;
        };
        // A drag is only noticed a few pixels after the press: what is
        // dragged, and from where, is what was pressed.
        let p = if r.drag_started() {
            ui.input(|i| i.pointer.press_origin()).unwrap_or(p)
        } else {
            p
        };
        let at = Pointer {
            p,
            t: self.time_at(area.left, p.x),
        };
        if self.drop_payloads(ui, &r, area, over, at, events) {
            return;
        }
        let targets = self.pointer_targets(area, at);
        self.scrub_on_empty(ui, &r, data, area.left, targets.hit, events);
        self.hover_feedback(ui, &r, &targets);
        self.start_drag(ui, &r, data, area, at, &targets, events);
        self.follow_clip_drag(ui, &r, data, area, at);
        self.follow_marks(ui, &r, data, area);
        self.finish_mark_drag(&r, data, events);
        self.level_drag_input(ui, &r, events);
        self.fade_drag_input(ui, &r, events);
        double_clicks(&r, &targets, events);
        self.finish_clip_drag(&r, events);
        self.menu_input(&r, data, area, at, &targets, menu, events);
        self.clicks(ui, &r, data, area.left, at, &targets, events);
    }

    /// The right-click menu of an area as it was opened, from `marks`
    /// ([`TimelineView::menu_marks`]).
    fn area_menu<'m>(
        &self,
        marks: &'m Option<(u64, Vec<&'m KeyframeMark>)>,
        tracks: &'m [TimelineTrack],
        data: &TimelineData<'m>,
        video: bool,
    ) -> Menu<'m> {
        let menu_clip = self
            .menu_at
            .and_then(|(_, clip)| clip)
            .and_then(|(id, ..)| find_clip(tracks, id));
        let chosen = || {
            data.video
                .iter()
                .chain(data.audio)
                .flat_map(|t| &t.clips)
                .filter(|c| c.selected)
        };
        let selected = chosen().take(2).count();
        let first_link = chosen().next().and_then(|c| c.link);
        let links_apart = selected > 1 && (first_link.is_none() || chosen().any(|c| c.link != first_link));
        Menu {
            at: self.menu_at,
            marks: marks.as_ref().map(|(clip, marks)| (*clip, marks.as_slice())),
            eases: data.keyframe_eases,
            has_region: data.region.is_some(),
            video_track: self.menu_track.filter(|_| video),
            on_nested: menu_clip.is_some_and(|c| c.shows == ClipShows::Timeline),
            ends_empty: menu_clip.is_some_and(|c| c.plays_until.is_some()),
            gap: self.menu_gap,
            selection: selected > 0,
            several: selected > 1,
            linked: menu_clip.is_some_and(|c| c.link.is_some()),
            links_apart,
        }
    }

    /// Something dragged in from a list over the area: where it would
    /// land is shown, and let go it is dropped there. True when the
    /// pointer was taken by it.
    fn drop_payloads(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        area: &Area,
        over: Over,
        at: Pointer,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let (Pointer { p, t }, tracks, rows, left) = (at, area.tracks, area.rows, area.left);
        let row_rects = rows.rects();
        // The video track a file dropped here lands on: the one under
        // the pointer, or the nearest above or below the rows.
        let drop_row = rows.nearest(p.y).filter(|_| area.video);
        // A file dragged over the timeline: show where it would land.
        if over.file {
            self.file_over = Some((t.max(0.0), drop_row));
        }
        // Taking a payload of the other type would lose it: ask for
        // the one that is there.
        let timeline_over = r.dnd_hover_payload::<DraggedTimeline>().is_some();
        if timeline_over && let Some(dragged) = r.dnd_release_payload::<DraggedTimeline>() {
            events.push(TimelineEvent::DropTimeline {
                timeline: dragged.0,
                at: t.max(0.0),
                track: drop_row.and_then(|row| tracks.get(row)).map(|t| t.id),
            });
            return true;
        }
        if over.title {
            if let Some(dragged) = r.dnd_release_payload::<DraggedTitle>() {
                events.push(TimelineEvent::DropTitle {
                    title: dragged.0,
                    at: t.max(0.0),
                    track: drop_row.and_then(|row| tracks.get(row)).map(|t| t.id),
                });
            }
            return true;
        }
        if over.transition {
            // The cut nearest the pointer on the clip under it, lit;
            // let go there, the transition goes onto it.
            let under = rows
                .at(p.y)
                .and_then(|row| Some((tracks.get(row)?, *row_rects.get(row)?)))
                .and_then(|(track, band)| Some((transitions::cut_near(track, t)?, band)));
            self.transition_over = under.map(|((_, cut), band)| (band, self.x_of(left, cut)));
            if let Some(dragged) = r.dnd_release_payload::<DraggedTransition>()
                && let Some(((clip, _), _)) = under
            {
                events.push(TimelineEvent::DropTransition {
                    transition: dragged.0,
                    clip,
                });
            }
            return true;
        }
        if over.effect {
            // The clip under the pointer, outlined while the effect is
            // over it; let go there, the effect goes onto it.
            let under = rows
                .at(p.y)
                .and_then(|row| Some((row, tracks.get(row)?)))
                .and_then(|(row, track)| {
                    let clip = track.clip_at(t)?;
                    let band = row_rects.get(row)?;
                    let x = (
                        self.x_of(left, clip.start),
                        self.x_of(left, clip.start + clip.duration),
                    );
                    Some((clip.id, Rect::from_x_y_ranges(x.0..=x.1, band.y_range())))
                });
            self.effect_over = under.map(|(_, rect)| rect);
            if let Some(dragged) = r.dnd_release_payload::<DraggedEffect>()
                && let Some((clip, _)) = under
            {
                events.push(TimelineEvent::DropEffect {
                    effect: dragged.0,
                    clip,
                });
            }
            return true;
        }
        if let Some(dragged) = r.dnd_release_payload::<DraggedMedia>() {
            let insert = ui.input(|i| i.modifiers.command);
            events.push(TimelineEvent::DropMedia {
                media: dragged.0,
                at: t.max(0.0),
                insert,
                track: drop_row.and_then(|row| tracks.get(row)).map(|t| t.id),
            });
            return true;
        }
        false
    }

    /// What lies under the pointer, in the order it is taken: a clip, the
    /// part of it (its edges trim), a keyframe mark, its level line, the
    /// strip its marks lie in.
    fn pointer_targets<'d>(&self, area: &Area<'d>, at: Pointer) -> Targets<'d> {
        let (Pointer { p, t }, left) = (at, area.left);
        let hit_track = area.rows.at(p.y).and_then(|row| area.tracks.get(row));
        let hit = hit_track.and_then(|track| track.clip_at(t));
        let edge_px = 6.0;
        let zone = hit.map(|c| {
            let (x0, x1) = (self.x_of(left, c.start), self.x_of(left, c.start + c.duration));
            if (p.x - x0).abs() <= edge_px && x1 - x0 > edge_px * 3.0 {
                DragKind::Trim(ClipEdge::Start)
            } else if (x1 - p.x).abs() <= edge_px && x1 - x0 > edge_px * 3.0 {
                DragKind::Trim(ClipEdge::End)
            } else {
                DragKind::Move
            }
        });
        // The clip under the pointer as it is drawn, when nothing is being
        // dragged and its parts are shown (not on compact tracks).
        let row = area.rows.at(p.y).and_then(|row| area.rows.rects().get(row));
        let drawn = match (hit, row) {
            (Some(c), Some(row)) if !self.compact && self.clip_drag.is_none() => Some((
                c,
                self.clip_rect(left, c.start, c.duration, row.top() + 3.0, row.bottom() - 3.0),
            )),
            _ => None,
        };
        // A fade's handle or curve point, with the tools that move clips.
        let on_fade = drawn
            .filter(|_| matches!(self.tool, Tool::Select | Tool::Ripple))
            .and_then(|(c, r)| {
                let sound = !area.video;
                let rect = match sound {
                    true => r,
                    false => clip_draw::parts(r, c.pictures.is_some() && c.waveform.is_some()).0,
                };
                let fades = if sound { c.sound.fades } else { c.fades };
                fades::grab(rect, &fades, self.zoom, sound, p).map(|(end, grip)| FadeTarget {
                    clip: c,
                    rect,
                    sound,
                    end,
                    grip,
                })
            });
        // A keyframe mark on the clip under the pointer: it comes before
        // the clip itself.
        let on_mark = drawn.filter(|(c, _)| !c.keyframes.is_empty()).and_then(|(c, r)| {
            keyframes::hit(r, &c.keyframes, |t| self.x_of(left, t), p).map(|mark| (c, mark))
        });
        let moves = zone == Some(DragKind::Move);
        // The level line of an audio clip, when its track shows it: it
        // comes before moving the clip, its edges before the line.
        let on_line = match (drawn, hit_track) {
            (Some((c, r)), Some(track)) if !area.video && self.level_line(track.id) && moves => {
                let db = level::db_of_gain(c.sound.gain);
                level::grabs(r, db, p.y).then_some((c, r, db))
            }
            _ => None,
        };
        // The strip along the bottom of a clip with marks, away from
        // them: a drag there draws a box that chooses marks.
        let on_strip = drawn
            .filter(|(c, _)| on_mark.is_none() && !c.keyframes.is_empty() && moves)
            .and_then(|(c, r)| keyframes::in_strip(r, p).then_some(c));
        Targets {
            hit_track,
            hit,
            zone,
            on_mark,
            on_line,
            on_fade,
            on_strip,
        }
    }

    /// Alt+drag on empty track space scrubs: the playhead follows the
    /// pointer. (On a clip, Alt+drag moves or trims it without snapping.)
    fn scrub_on_empty(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        data: &TimelineData,
        left: f32,
        hit: Option<&TimelineClip>,
        events: &mut Vec<TimelineEvent>,
    ) {
        if r.drag_started_by(egui::PointerButton::Primary) && hit.is_none() && ui.input(|i| i.modifiers.alt) {
            self.scrub = true;
        }
        if self.scrub
            && r.dragged()
            && let Some(now) = r.interact_pointer_pos()
        {
            events.push(TimelineEvent::Seek(self.frame_under(left, now.x, data)));
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    }

    /// The pointer resting over the area, nothing being dragged: the
    /// cursor and the tooltip of what a press there would take.
    fn hover_feedback(&self, ui: &Ui, r: &egui::Response, targets: &Targets) {
        let Targets {
            hit,
            zone,
            on_mark,
            on_line,
            on_fade,
            on_strip,
            ..
        } = *targets;
        let still = self.clip_drag.is_none()
            && self.level_drag.is_none()
            && self.fade_drag.is_none()
            && self.mark_drag.is_none()
            && self.mark_box.is_none();
        if r.hovered()
            && still
            && let Some(fade) = on_fade
        {
            let what = if fade.sound { "sound" } else { "picture" };
            let end = match fade.end {
                fades::FadeEnd::In => "in",
                fades::FadeEnd::Out => "out",
            };
            let says = match fade.grip {
                fades::FadeGrip::Length => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                    format!("Fade the {what} {end}: drag sideways")
                }
                fades::FadeGrip::Curve => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                    format!(
                        "The fade's curve: drag up to come {end} sooner, down for later; double-click for a plain fade"
                    )
                }
            };
            r.clone().on_hover_text_at_pointer(says);
        } else if r.hovered()
            && still
            && let Some((_, mark)) = on_mark
        {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            r.clone().on_hover_text_at_pointer(mark.says());
        } else if r.hovered() && still && on_strip.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        } else if r.hovered() && still && on_line.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            if let Some((_, _, db)) = on_line {
                r.clone().on_hover_text_at_pointer(format!(
                    "Volume {}: drag up or down (Alt: finer), double-click for 0 dB",
                    level::words(f64::from(db))
                ));
            }
        } else if r.hovered() && still {
            match (self.tool, zone) {
                (Tool::Blade, Some(_)) => ui.ctx().set_cursor_icon(CursorIcon::Crosshair),
                (Tool::Text, None) => ui.ctx().set_cursor_icon(CursorIcon::Text),
                (Tool::Select | Tool::Ripple | Tool::Roll, Some(DragKind::Trim(_))) => {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
                }
                (Tool::Select | Tool::Ripple, Some(DragKind::Move)) => {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab)
                }
                (Tool::Slip | Tool::Slide, Some(_)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn),
                _ => {}
            }
            if let Some(c) = hit.filter(|c| !c.info.is_empty()) {
                r.clone().on_hover_text_at_pointer(&c.info);
            }
        }
    }

    /// A drag begun on the area: a keyframe mark, a box along the marks, a
    /// level line, or the clip itself (moved, trimmed, slipped, slid or
    /// rolled, as the tool says).
    #[allow(clippy::too_many_arguments)]
    fn start_drag(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        data: &TimelineData,
        area: &Area,
        at: Pointer,
        targets: &Targets,
        events: &mut Vec<TimelineEvent>,
    ) {
        let (Pointer { p, t }, tracks) = (at, area.tracks);
        let row_rects = area.rows.rects();
        let Targets {
            hit_track,
            hit,
            zone,
            on_mark,
            on_line,
            on_fade,
            on_strip,
        } = *targets;
        if r.drag_started_by(egui::PointerButton::Primary)
            && let Some(fade) = on_fade
        {
            let now = fade.fades();
            self.fade_drag = Some(fades::FadeDrag {
                clip: fade.clip.id,
                sound: fade.sound,
                end: fade.end,
                grip: fade.grip,
                from: now,
                now,
                length: fade.clip.duration,
            });
            self.fade_rect = Some(fade.rect);
            if !fade.clip.selected {
                events.push(TimelineEvent::Select {
                    clip: fade.clip.id,
                    add: false,
                });
            }
        } else if r.drag_started_by(egui::PointerButton::Primary)
            && let Some((c, mark)) = on_mark
        {
            // The chosen marks move with the grabbed one; a mark that
            // isn't chosen is chosen alone.
            let fps = data.fps;
            let chosen = self.chosen_marks.of(c.id, &c.keyframes, fps);
            let others = chosen
                .iter()
                .copied()
                .filter(|at| !keyframes::same(*at, mark.at, fps));
            let moving: Vec<f64> = match chosen.iter().any(|at| keyframes::same(*at, mark.at, fps)) {
                true => std::iter::once(mark.at).chain(others).collect(),
                false => {
                    self.chosen_marks.click(c.id, mark.at, false, fps);
                    vec![mark.at]
                }
            };
            self.mark_drag = Some(keyframes::MarkDrag {
                clip: c.id,
                moving,
                from: t,
                by: 0.0,
                refused: false,
            });
        } else if r.drag_started_by(egui::PointerButton::Primary)
            && let Some(c) = on_strip
        {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let kept = match add {
                true => self.chosen_marks.of(c.id, &c.keyframes, data.fps),
                false => Vec::new(),
            };
            self.mark_box = Some(keyframes::MarkBox {
                clip: c.id,
                from: t,
                to: t,
                kept,
            });
        } else if r.drag_started_by(egui::PointerButton::Primary)
            && let Some((c, drawn, db)) = on_line
        {
            self.level_drag = Some(level::LevelDrag::new(c.id, drawn, db, p.y));
            if !c.selected {
                events.push(TimelineEvent::Select {
                    clip: c.id,
                    add: false,
                });
            }
        } else if r.drag_started_by(egui::PointerButton::Primary)
            && let (Some(c), Some(zone), Some(track)) = (hit, zone, hit_track)
            && let Some((c, kind)) = drag_for(self.tool, zone, c, tracks, row_rects, p.y)
        {
            // One of several selected clips: they all move together.
            let together = kind == DragKind::Move
                && c.selected
                && data
                    .video
                    .iter()
                    .chain(data.audio)
                    .flat_map(|t| &t.clips)
                    .any(|o| o.selected && o.id != c.id && (c.link.is_none() || o.link != c.link));
            self.clip_drag = Some(ClipDrag {
                clip: c.id,
                link: c.link,
                track: track.id,
                over: Landing::Track(track.id),
                refused: false,
                kind,
                grabbed_at: t,
                start: c.start,
                end: c.start + c.duration,
                delta: 0.0,
                together,
            });
            if !c.selected {
                events.push(TimelineEvent::Select {
                    clip: c.id,
                    add: false,
                });
            }
        }
    }

    /// A clip being dragged follows the pointer: snapped, held to the file
    /// there is, onto another track or a new one, refused where a clip is
    /// in the way.
    fn follow_clip_drag(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        data: &TimelineData,
        area: &Area,
        at: Pointer,
    ) {
        let (Pointer { p, t }, tracks, rows, time_area, from_bottom) =
            (at, area.tracks, area.rows, area.time, area.video);
        if r.dragged()
            && let Some(drag) = self.clip_drag
        {
            let raw = t - drag.grabbed_at;
            self.obstacles.refresh(data, |c| drag.moves(c));
            let delta = if self.snapping && !ui.input(|i| i.modifiers.alt) {
                self.snapped(raw, &drag, data)
            } else {
                raw
            };
            // Held to the file there is, as it goes, so letting go is
            // never refused because the file ends.
            // (A trim changes the clip and the clips linked to it only.)
            let room = || {
                slip::room_of(data, |c| {
                    c.id == drag.clip || (drag.link.is_some() && c.link == drag.link)
                })
            };
            let delta = match drag.kind {
                DragKind::Move | DragKind::Slide => delta.max(-drag.start),
                DragKind::Trim(ClipEdge::Start) => {
                    slip::trim_held(delta, room(), data.fps, true).max(-drag.start)
                }
                DragKind::RippleTrim(ClipEdge::Start) => slip::trim_held(delta, room(), data.fps, true),
                DragKind::Trim(ClipEdge::End) | DragKind::RippleTrim(ClipEdge::End) | DragKind::Roll => {
                    slip::trim_held(delta, room(), data.fps, false)
                }
                DragKind::Slip => slip::held(delta, room(), data.fps),
            };
            // A move also follows the pointer to another track of this
            // area, and past the last one to a track of its own.
            let over = match drag.kind {
                // Several clips move along their own tracks only.
                DragKind::Move if drag.together => Landing::Track(drag.track),
                DragKind::Move
                    if self.asks_for_new_track(&drag, tracks, rows, time_area, from_bottom, p.y) =>
                {
                    Landing::NewTrack
                }
                DragKind::Move => {
                    // (`max` last: an area scrolled out of the one list has no height.)
                    let y = p.y.min(time_area.bottom() - 1.0).max(time_area.top());
                    landing_track(tracks, rows, y, drag.track).map_or(drag.over, Landing::Track)
                }
                _ => Landing::Track(drag.track),
            };
            let mut drag = ClipDrag { delta, over, ..drag };
            drag.refused = drag.is_in_the_way(data, &self.obstacles);
            self.clip_drag = Some(drag);
            ui.ctx().set_cursor_icon(match drag.kind {
                DragKind::Move if drag.refused => CursorIcon::NotAllowed,
                DragKind::Move => CursorIcon::Grabbing,
                DragKind::Slip | DragKind::Slide => CursorIcon::ResizeColumn,
                _ => CursorIcon::ResizeHorizontal,
            });
        }
    }

    /// Keyframe marks being dragged, or a box being dragged along them to
    /// choose them, follow the pointer.
    fn follow_marks(&mut self, ui: &Ui, r: &egui::Response, data: &TimelineData, area: &Area) {
        let (tracks, left) = (area.tracks, area.left);
        if r.dragged()
            && let Some(drag) = self.mark_drag.clone()
            && let Some(now) = r.interact_pointer_pos()
            && let Some(c) = find_clip(tracks, drag.clip)
        {
            self.follow_mark_drag(ui, drag, c, data, self.time_at(left, now.x));
        }
        let box_to = r
            .interact_pointer_pos()
            .filter(|_| r.dragged())
            .map(|now| self.time_at(left, now.x));
        if let Some(chosen_box) = &mut self.mark_box {
            if let Some(to) = box_to {
                chosen_box.to = to;
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            if let Some(c) = tracks
                .iter()
                .flat_map(|t| &t.clips)
                .find(|c| c.id == chosen_box.clip)
            {
                self.chosen_marks = chosen_box.chosen(&c.keyframes, data.fps);
            }
            if r.drag_stopped() {
                self.mark_box = None;
            }
        }
    }

    /// Keyframe marks let go: moved, and still chosen where they went.
    fn finish_mark_drag(&mut self, r: &egui::Response, data: &TimelineData, events: &mut Vec<TimelineEvent>) {
        if r.drag_stopped()
            && let Some(drag) = self.mark_drag.take()
            && drag.by.abs() > 0.5 / data.fps.max(1.0)
        {
            if !drag.refused {
                // The chosen marks go along.
                let at = drag.moving.iter().map(|at| at + drag.by).collect();
                self.chosen_marks = keyframes::Chosen { clip: drag.clip, at };
            }
            events.push(TimelineEvent::Keyframes {
                clip: drag.clip,
                marks: drag.moving,
                change: MarksChange::Move(drag.by),
            });
        }
    }

    /// A level line being dragged follows the pointer; let go, the clip
    /// gets that volume.
    fn level_drag_input(&mut self, ui: &Ui, r: &egui::Response, events: &mut Vec<TimelineEvent>) {
        if let Some(drag) = &mut self.level_drag {
            // Where the pointer is now, also on the frame the drag is
            // noticed (when `p` is where it was pressed).
            if r.dragged()
                && let Some(now) = r.interact_pointer_pos()
            {
                drag.follow(now.y, ui.input(|i| i.modifiers.alt || i.modifiers.shift));
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            if r.drag_stopped() {
                let drag = *drag;
                self.level_drag = None;
                if (drag.db - drag.from_db).abs() > 1e-3 {
                    events.push(TimelineEvent::ClipVolume {
                        clip: drag.clip,
                        gain_db: drag.db,
                    });
                }
            }
        }
    }

    /// A fade's handle or curve point being dragged follows the pointer;
    /// let go, the clip gets those fades.
    fn fade_drag_input(&mut self, ui: &Ui, r: &egui::Response, events: &mut Vec<TimelineEvent>) {
        let (Some(drag), Some(rect)) = (&mut self.fade_drag, self.fade_rect) else {
            return;
        };
        if r.dragged()
            && let Some(now) = r.interact_pointer_pos()
        {
            drag.follow(rect, self.zoom, now);
            ui.ctx().set_cursor_icon(match drag.grip {
                fades::FadeGrip::Length => CursorIcon::ResizeHorizontal,
                fades::FadeGrip::Curve => CursorIcon::ResizeVertical,
            });
        }
        if r.drag_stopped() {
            let drag = *drag;
            self.fade_drag = None;
            self.fade_rect = None;
            if drag.changed() {
                events.push(TimelineEvent::ClipFade {
                    clip: drag.clip,
                    sound: drag.sound,
                    fades: drag.now,
                });
            }
        }
    }

    /// A clip let go: the move, trim, roll, slip or slide it was dragged
    /// into.
    fn finish_clip_drag(&mut self, r: &egui::Response, events: &mut Vec<TimelineEvent>) {
        if r.drag_stopped() {
            // Gathered again for the next drag.
            self.obstacles = Obstacles::default();
        }
        if r.drag_stopped()
            && let Some(drag) = self.clip_drag.take()
            && (drag.delta.abs() > 1e-9 || drag.lands_elsewhere().is_some())
        {
            events.push(match drag.kind {
                DragKind::Move => TimelineEvent::Move {
                    clip: drag.clip,
                    by: drag.delta,
                    track: match drag.lands_elsewhere() {
                        Some(Landing::Track(track)) => Some(track),
                        Some(Landing::NewTrack) | None => None,
                    },
                    new_track: drag.over == Landing::NewTrack,
                },
                DragKind::Trim(ClipEdge::Start) => TimelineEvent::Trim {
                    clip: drag.clip,
                    edge: ClipEdge::Start,
                    to: drag.start + drag.delta,
                },
                DragKind::Trim(ClipEdge::End) => TimelineEvent::Trim {
                    clip: drag.clip,
                    edge: ClipEdge::End,
                    to: drag.end + drag.delta,
                },
                DragKind::RippleTrim(edge) => TimelineEvent::RippleTrim {
                    clip: drag.clip,
                    edge,
                    to: match edge {
                        ClipEdge::Start => drag.start + drag.delta,
                        ClipEdge::End => drag.end + drag.delta,
                    },
                },
                DragKind::Roll => TimelineEvent::Roll {
                    clip: drag.clip,
                    to: drag.end + drag.delta,
                },
                // Dragging right brings earlier parts of the file into view.
                DragKind::Slip => TimelineEvent::Slip {
                    clip: drag.clip,
                    by: -drag.delta,
                },
                DragKind::Slide => TimelineEvent::Slide {
                    clip: drag.clip,
                    by: drag.delta,
                },
            });
        }
    }

    /// The right-click menu: opened (on a clip, a gap, a track or marks)
    /// and shown. `menu` is how it was before this frame.
    #[allow(clippy::too_many_arguments)]
    fn menu_input(
        &mut self,
        r: &egui::Response,
        data: &TimelineData,
        area: &Area,
        at: Pointer,
        targets: &Targets,
        menu: Menu,
        events: &mut Vec<TimelineEvent>,
    ) {
        let (t, tracks, from_bottom) = (at.t, area.tracks, area.video);
        let Targets {
            hit_track,
            hit,
            on_mark,
            ..
        } = *targets;
        if r.secondary_clicked() {
            self.menu_at = Some((t, hit.map(|c| (c.id, c.start, c.start + c.duration))));
            self.menu_track = hit_track.map(|track| track.id).filter(|_| from_bottom);
            self.menu_gap = match hit {
                Some(_) => None,
                None => hit_track.and_then(|track| gaps::gap_at(track, t)),
            };
            // On a mark, the menu is the chosen marks' when it is one of
            // them, and that mark's alone otherwise.
            self.mark_menu = on_mark.map(|(c, mark)| {
                let chosen = self.chosen_marks.of(c.id, &c.keyframes, data.fps);
                if !chosen.iter().any(|at| keyframes::same(*at, mark.at, data.fps)) {
                    self.chosen_marks.click(c.id, mark.at, false, data.fps);
                    return (c.id, vec![mark.at]);
                }
                (c.id, chosen)
            });
        }
        let menu_marks = self.menu_marks(tracks, data.fps);
        let menu = Menu {
            marks: menu_marks.as_ref().map(|(clip, marks)| (*clip, marks.as_slice())),
            at: self.menu_at,
            video_track: self.menu_track.filter(|_| from_bottom),
            ..menu
        };
        clip_menu(r, menu, events);
    }

    /// Clicks: Alt+click moves the playhead; a click on a keyframe mark goes
    /// there (or chooses it); elsewhere it selects, cuts with the blade, or
    /// selects the gap between clips.
    #[allow(clippy::too_many_arguments)]
    fn clicks(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        data: &TimelineData,
        left: f32,
        at: Pointer,
        targets: &Targets,
        events: &mut Vec<TimelineEvent>,
    ) {
        let t = at.t;
        let Targets {
            hit_track,
            hit,
            on_mark,
            ..
        } = *targets;
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        if let Some(at) = self.alt_click(ui, r, left, data) {
            // Alt+click: the playhead goes there, with any tool; the
            // selection and the chosen marks stay as they are.
            events.push(TimelineEvent::Seek(at));
        } else if r.clicked()
            && let Some((c, mark)) = on_mark
        {
            // A keyframe mark: the playhead goes there, or (Shift or
            // Ctrl) the mark is chosen with the others.
            self.chosen_marks.click(c.id, mark.at, add, data.fps);
            if !add {
                events.push(TimelineEvent::Seek(mark.at));
            }
        } else if r.clicked() {
            self.chosen_marks = keyframes::Chosen::default();
            // A click between two clips selects the gap there.
            self.gap = match hit {
                Some(_) => None,
                None => hit_track.and_then(|track| gaps::gap_at(track, t)),
            };
            events.push(match (self.tool, hit) {
                (Tool::Blade, Some(c)) => TimelineEvent::Blade { clip: c.id, at: t },
                (Tool::Text, None) => TimelineEvent::AddTitleAt {
                    at: t,
                    track: hit_track
                        .map(|track| track.id)
                        .filter(|id| data.video.iter().any(|v| v.id == *id)),
                },
                (_, Some(c)) => TimelineEvent::Select { clip: c.id, add },
                (_, None) => TimelineEvent::ClearSelection,
            });
        }
    }

    /// The marks the right-click menu was opened on, when they are on a
    /// clip of `tracks` and still there.
    fn menu_marks<'d>(&self, tracks: &'d [TimelineTrack], fps: f64) -> Option<(u64, Vec<&'d KeyframeMark>)> {
        let (clip, times) = self.mark_menu.as_ref()?;
        let c = find_clip(tracks, *clip)?;
        let marks: Vec<&KeyframeMark> = c
            .keyframes
            .iter()
            .filter(|m| times.iter().any(|at| keyframes::same(*at, m.at, fps)))
            .collect();
        (!marks.is_empty()).then_some((*clip, marks))
    }

    /// A keyframe mark dragged on clip `c` with the pointer at time `now`:
    /// how far its marks go, and whether they could be let go there.
    fn follow_mark_drag(
        &mut self,
        ui: &Ui,
        drag: keyframes::MarkDrag,
        c: &TimelineClip,
        data: &TimelineData,
        now: f64,
    ) {
        let fps = data.fps;
        let moving = |at: f64| drag.moving.iter().any(|m| keyframes::same(*m, at, fps));
        // The playhead and the clip's other marks, unless snapping is off.
        let targets: Vec<f64> = match self.snapping && !ui.input(|i| i.modifiers.alt) {
            true => std::iter::once(data.playhead)
                .chain(c.keyframes.iter().map(|m| m.at).filter(|at| !moving(*at)))
                .collect(),
            false => Vec::new(),
        };
        let (first, last) = keyframes::frames_of(c.start, c.start + c.duration, fps);
        let room = keyframes::Room {
            fps,
            first,
            last,
            targets: &targets,
            reach: f64::from(keyframes::SNAP_REACH) / f64::from(self.zoom),
        };
        let by = keyframes::dragged_by(now - drag.from, &drag.moving, room);
        let refused = keyframes::clashes(&c.keyframes, &drag.moving, by, fps);
        ui.ctx().set_cursor_icon(match refused {
            true => CursorIcon::NotAllowed,
            false => CursorIcon::ResizeHorizontal,
        });
        self.mark_drag = Some(keyframes::MarkDrag { by, refused, ..drag });
    }

    /// Whether a clip dragged to screen position `y` is clearly past the
    /// last track of its kind (`tracks`, with their `rows` in `area`): above
    /// the top video track, or under the bottom audio track. Only while
    /// that track's far edge is in view, so a track scrolled out of sight is
    /// not skipped; and not for a clip alone on that last track, which a
    /// new track would only leave empty. When the tracks reach the area's
    /// far edge (no room past the last), the band [`NEW_TRACK_EDGE`] along
    /// that edge, or past it, asks.
    fn asks_for_new_track(
        &self,
        drag: &ClipDrag,
        tracks: &[TimelineTrack],
        rows: &Rows,
        area: Rect,
        video: bool,
        y: f32,
    ) -> bool {
        let (Some(last), Some(row)) = (tracks.last(), rows.rects().get(tracks.len().wrapping_sub(1))) else {
            return false;
        };
        // In the rough cut view an audio track may hold more than is shown.
        let alone = last.id == drag.track && last.clips.len() == 1 && (video || !self.rough_cut);
        if alone {
            return false;
        }
        past_the_last(row, area, video, y)
    }

    /// `raw` (seconds) adjusted so the dragged edge lands on the nearest snap
    /// point within a few pixels: other clips' edges, the playhead, the edit
    /// needle, labels and the start.
    /// The clips' edges are looked up in `self.obstacles`, gathered for
    /// `drag` beforehand. Of two points as near, the dragged start wins
    /// over its end, then the start of the timeline, the playhead, the
    /// needle, labels and clip edges in that order (as listed).
    pub(super) fn snapped(&self, raw: f64, drag: &ClipDrag, data: &TimelineData) -> f64 {
        let reach = 8.0 / f64::from(self.zoom);
        let edges: [Option<f64>; 2] = match drag.kind {
            DragKind::Move | DragKind::Slide => [Some(drag.start + raw), Some(drag.end + raw)],
            DragKind::Trim(ClipEdge::Start) | DragKind::RippleTrim(ClipEdge::Start) => {
                [Some(drag.start + raw), None]
            }
            DragKind::Trim(ClipEdge::End) | DragKind::RippleTrim(ClipEdge::End) | DragKind::Roll => {
                [Some(drag.end + raw), None]
            }
            DragKind::Slip => [None, None],
        };
        let mut best: Option<f64> = None;
        for e in edges.into_iter().flatten() {
            let points = [0.0, data.playhead]
                .into_iter()
                .chain(data.needle)
                .chain(data.markers.iter().map(|m| m.at));
            let clip_edge = self.obstacles.nearest_edge(e, reach);
            for d in points.map(|p| p - e).chain(clip_edge) {
                if d.abs() <= reach && best.is_none_or(|b| d.abs() < b.abs()) {
                    best = Some(d);
                }
            }
        }
        raw + best.unwrap_or(0.0)
    }
}

/// Double-clicks on a clip: on its level line, 0 dB; on a nested
/// timeline's clip, that timeline opens.
fn double_clicks(r: &egui::Response, targets: &Targets, events: &mut Vec<TimelineEvent>) {
    let Targets {
        hit,
        on_line,
        on_fade,
        ..
    } = *targets;
    // A fade's curve point: a plain fade again.
    if r.double_clicked()
        && let Some(fade) = on_fade.filter(|f| f.grip == fades::FadeGrip::Curve)
    {
        let now = fade.fades();
        let (length, curve) = now.of(fade.end);
        if curve != 0.0 {
            events.push(TimelineEvent::ClipFade {
                clip: fade.clip.id,
                sound: fade.sound,
                fades: now.with(fade.end, length, 0.0),
            });
        }
        return;
    }
    if r.double_clicked()
        && on_line.is_none()
        && let Some(c) = hit.filter(|c| c.shows == ClipShows::Timeline)
    {
        events.push(TimelineEvent::OpenNested { clip: c.id });
    }
    if r.double_clicked()
        && let Some((c, _, db)) = on_line
        && db != 0.0
    {
        events.push(TimelineEvent::ClipVolume {
            clip: c.id,
            gain_db: 0.0,
        });
    }
}

/// What dragging `c` (grabbed in `zone`) does with `tool`, and which clip
/// it acts on: the roll tool grabbed at a clip's start acts on the cut after
/// the clip before it. `None` when the tool does nothing there.
pub(super) fn drag_for<'a>(
    tool: Tool,
    zone: DragKind,
    c: &'a TimelineClip,
    tracks: &'a [TimelineTrack],
    rows: &[Rect],
    y: f32,
) -> Option<(&'a TimelineClip, DragKind)> {
    match (tool, zone) {
        (Tool::Select, kind) => Some((c, kind)),
        (Tool::Ripple, DragKind::Trim(edge)) => Some((c, DragKind::RippleTrim(edge))),
        (Tool::Ripple, kind) => Some((c, kind)),
        (Tool::Roll, DragKind::Trim(ClipEdge::End)) => Some((c, DragKind::Roll)),
        (Tool::Roll, DragKind::Trim(ClipEdge::Start)) => {
            let track = tracks
                .iter()
                .zip(rows)
                .find(|(_, row)| row.y_range().contains(y))?
                .0;
            let before = track
                .clips
                .iter()
                .find(|b| (b.start + b.duration - c.start).abs() < 1e-6 && b.id != c.id)?;
            Some((before, DragKind::Roll))
        }
        (Tool::Slip, _) => Some((c, DragKind::Slip)),
        (Tool::Slide, _) => Some((c, DragKind::Slide)),
        _ => None,
    }
}

/// The track a clip dragged to screen position `y` lands on: the one whose
/// row is there (the nearest past the first or the last), but its own
/// track, `own`, until the pointer is clearly past that row's edge.
fn landing_track(tracks: &[TimelineTrack], rows: &Rows, y: f32, own: u64) -> Option<u64> {
    let own_row = tracks
        .iter()
        .position(|t| t.id == own)
        .and_then(|i| rows.rects().get(i));
    if own_row.is_some_and(|r| r.top() - STAYS_ON_ITS_TRACK <= y && y < r.bottom() + STAYS_ON_ITS_TRACK) {
        return Some(own);
    }
    rows.nearest(y).and_then(|row| tracks.get(row)).map(|t| t.id)
}

/// Whether `y` is past the last track's `row` (see
/// `TimelineView::asks_for_new_track`): [`NEW_TRACK_PAST`] past it when
/// there is room, or within [`NEW_TRACK_EDGE`] of the area's edge (or past
/// it) when the row reaches that edge. Never while the row's far edge is
/// out of view.
pub(super) fn past_the_last(row: &Rect, area: Rect, video: bool, y: f32) -> bool {
    if video {
        let room = row.top() - area.top();
        if room < -0.5 {
            false
        } else if room >= NEW_TRACK_PAST + NEW_TRACK_EDGE {
            y < row.top() - NEW_TRACK_PAST
        } else {
            y < area.top() + NEW_TRACK_EDGE
        }
    } else {
        let room = area.bottom() - row.bottom();
        if room < -0.5 {
            false
        } else if room >= NEW_TRACK_PAST + NEW_TRACK_EDGE {
            y >= row.bottom() + NEW_TRACK_PAST
        } else {
            y >= area.bottom() - NEW_TRACK_EDGE
        }
    }
}
