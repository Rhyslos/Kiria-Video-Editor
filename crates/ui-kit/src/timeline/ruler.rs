//! The time axis of the timeline: the ruler (timecode, the playhead's
//! handle, the edit needle's, the region's), the labels row, the scroll
//! bar and the divider, how the pointer works them and how they are drawn.

use super::{
    ClickMoves, LABEL_SIZE, Layout, TimelineData, TimelineEvent, TimelineMarker, TimelineView, cache_bar,
    menus::region_menu,
};
use crate::Theme;
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// The region while it is being drawn or one of its handles dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RegionDrag {
    /// Shift-drag in the ruler from this time.
    New(f64),
    Start,
    End,
}

impl TimelineView {
    /// The ruler and the labels row as one place to click and drag. It
    /// takes the pointer first, so the handles lying on it (the edit
    /// needle's, the region's edges) come after it and are on top: a press
    /// on a handle drags the handle, not the playhead. A right-click in it
    /// offers what can be done with the region.
    pub(super) fn ruler_response(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) -> egui::Response {
        let ruler = ui.interact(
            layout.ruler.union(layout.labels),
            id.with("ruler"),
            Sense::click_and_drag(),
        );
        // A right-click in the ruler offers what can be done with the region.
        if data.region.is_some() {
            ruler.context_menu(|ui| {
                region_menu(ui, events);
                if ui.button("Clear the region").clicked() {
                    events.push(TimelineEvent::SetRegion(None));
                    ui.close();
                }
            });
        }
        ruler
    }

    /// The playhead's handle at the top of the ruler: dragging it moves
    /// the playhead whatever a click in the ruler moves. Not with Shift
    /// held, so a region can be begun at the playhead. True when it took
    /// the pointer this frame.
    pub(super) fn playhead_handle_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let left = layout.time_left();
        let ph = self.x_of(left, data.playhead);
        let handle_id = id.with("playhead");
        let shift = ui.input(|i| i.modifiers.shift);
        self.playhead_lit = false;
        if !shift || ui.ctx().is_being_dragged(handle_id) {
            let r = ui
                .interact(playhead_grab(ph, layout), handle_id, Sense::click_and_drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            self.playhead_lit = r.hovered() || r.dragged();
            if self.needle_moves(data) && !r.dragged() {
                r.clone()
                    .on_hover_text("Drag to move the playhead, or Alt+click anywhere on the timeline");
            }
            if r.dragged_by(egui::PointerButton::Primary)
                && let Some(p) = r.interact_pointer_pos()
            {
                events.push(TimelineEvent::Seek(self.time_inside(left, p.x, data)));
            }
            if let Some(t) = self.alt_click(ui, &r, left, data) {
                events.push(TimelineEvent::Seek(t));
            }
            if r.dragged() || r.drag_stopped() || r.clicked() {
                return true;
            }
        }
        false
    }

    /// The edit needle's handle, dragged along the ruler. True when it
    /// took the pointer this frame.
    pub(super) fn needle_handle_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let left = layout.time_left();
        if let Some(needle) = data.needle {
            let nx = self.x_of(left, needle);
            let handle = Rect::from_center_size(pos2(nx, layout.ruler.bottom() - 6.0), vec2(14.0, 14.0));
            let r = ui
                .interact(handle, id.with("needle"), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                self.needle_drag = Some(self.time_inside(left, p.x, data));
            }
            if r.drag_stopped()
                && let Some(t) = self.needle_drag.take()
            {
                events.push(TimelineEvent::MoveNeedle(t));
            }
            if r.dragged() || r.drag_stopped() {
                return true;
            }
        }
        false
    }

    /// The region's handles: drag to adjust. True when one took the
    /// pointer this frame.
    pub(super) fn region_handles_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        id: egui::Id,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let left = layout.time_left();
        if let Some((a, b)) = data.region {
            for (edge, x) in [
                (RegionDrag::Start, self.x_of(left, a)),
                (RegionDrag::End, self.x_of(left, b)),
            ] {
                let handle = Rect::from_center_size(pos2(x, layout.ruler.bottom() - 4.0), vec2(14.0, 14.0));
                let r = ui
                    .interact(
                        handle,
                        id.with(("region", edge == RegionDrag::Start)),
                        Sense::drag(),
                    )
                    .on_hover_cursor(CursorIcon::ResizeHorizontal);
                if r.drag_started() {
                    self.region_drag = Some((edge, (a, b)));
                }
                if r.dragged()
                    && let (Some(p), Some((kind, (ra, rb)))) = (r.interact_pointer_pos(), self.region_drag)
                {
                    let t = self.time_inside(left, p.x, data);
                    let next = match kind {
                        RegionDrag::Start => (t.min(rb), rb),
                        _ => (ra, t.max(ra)),
                    };
                    self.region_drag = Some((kind, next));
                }
                if r.drag_stopped()
                    && let Some((_, (ra, rb))) = self.region_drag.take()
                {
                    events.push(TimelineEvent::SetRegion((rb - ra > 1e-6).then_some((ra, rb))));
                }
                if r.dragged() || r.drag_stopped() {
                    return true;
                }
            }
        }
        false
    }

    /// Labels: click one to go to it, double-click to rename or recolour
    /// it. True when one took the click.
    pub(super) fn labels_input(
        &mut self,
        ui: &Ui,
        layout: &Layout,
        data: &TimelineData,
        events: &mut Vec<TimelineEvent>,
    ) -> bool {
        let left = layout.time_left();
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && layout.labels.contains(pos)
            && let Some(marker) = self.marker_near(left, data.markers, pos.x)
        {
            let (double, single) = ui.input(|i| {
                (
                    i.pointer.button_double_clicked(egui::PointerButton::Primary),
                    i.pointer.button_clicked(egui::PointerButton::Primary),
                )
            });
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            if double {
                events.push(TimelineEvent::EditLabel(marker.id));
                return true;
            }
            if single && !ui.input(|i| i.modifiers.alt) {
                events.push(TimelineEvent::Seek(marker.at));
                return true;
            }
        }
        false
    }

    /// Ruler: click or drag to move the playhead (or the edit needle, as
    /// set); Shift-drag draws the region, Alt+drag scrubs. The cache bar
    /// explains its colours when pointed at. `alt_clicked`: an Alt+click
    /// already put the playhead somewhere.
    pub(super) fn ruler_clicks(
        &mut self,
        ui: &Ui,
        layout: &Layout,
        data: &TimelineData,
        ruler: &egui::Response,
        alt_clicked: bool,
        events: &mut Vec<TimelineEvent>,
    ) {
        let left = layout.time_left();
        if let Some(p) = ruler.hover_pos()
            && layout.cache_strip().expand2(vec2(0.0, 2.0)).contains(p)
            && !ruler.dragged()
        {
            ruler
                .clone()
                .on_hover_text_at_pointer(cache_bar::legend(data.cache.about));
        }
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        if ruler.drag_started_by(egui::PointerButton::Primary) {
            if shift && let Some(p) = ruler.interact_pointer_pos() {
                let t = self.time_inside(left, p.x, data);
                self.region_drag = Some((RegionDrag::New(t), (t, t)));
            } else if alt {
                self.scrub = true;
            }
        }
        if alt_clicked {
            // Alt+click: the playhead went there above.
        } else if self.scrub {
            // Alt+drag: the playhead follows, whatever a click moves.
            if ruler.dragged()
                && let Some(p) = ruler.interact_pointer_pos()
            {
                events.push(TimelineEvent::Seek(self.frame_under(left, p.x, data)));
            }
        } else if let Some((RegionDrag::New(from), _)) = self.region_drag {
            if ruler.dragged()
                && let Some(p) = ruler.interact_pointer_pos()
            {
                let t = self.time_inside(left, p.x, data);
                self.region_drag = Some((RegionDrag::New(from), (from.min(t), from.max(t))));
            }
            if ruler.drag_stopped()
                && let Some((_, (a, b))) = self.region_drag.take()
                && b - a > 1e-6
            {
                events.push(TimelineEvent::SetRegion(Some((a, b))));
            }
        } else if self.needle_moves(data) {
            // A click puts the edit needle there; a drag takes it along and
            // puts it where it is let go, as dragging its handle does.
            if let Some(p) = ruler.interact_pointer_pos() {
                let t = self.time_inside(left, p.x, data);
                if ruler.clicked() {
                    events.push(TimelineEvent::MoveNeedle(t));
                } else if ruler.dragged_by(egui::PointerButton::Primary) {
                    self.needle_drag = Some(t);
                }
            }
            if ruler.drag_stopped()
                && let Some(t) = self.needle_drag.take()
            {
                events.push(TimelineEvent::MoveNeedle(t));
            }
        } else if (ruler.clicked() || ruler.dragged_by(egui::PointerButton::Primary))
            && let Some(p) = ruler.interact_pointer_pos()
        {
            events.push(TimelineEvent::Seek(self.time_inside(left, p.x, data)));
        }
    }

    /// The scroll bar: drag the thumb, or grab beside it to centre the
    /// view there.
    pub(super) fn scrollbar_input(&mut self, ui: &Ui, layout: &Layout, data: &TimelineData, id: egui::Id) {
        let left = layout.time_left();
        let (thumb, total) = scrollbar_thumb(layout, self, data.length);
        let bar = ui.interact(layout.scrollbar, id.with("scrollbar"), Sense::click_and_drag());
        if bar.drag_started()
            && let Some(p) = bar.interact_pointer_pos()
        {
            self.scrollbar_drag = Some((p.x, if thumb.contains(p) { self.scroll } else { -1.0 }));
        }
        if let (Some((start_x, start_scroll)), Some(p)) = (self.scrollbar_drag, bar.interact_pointer_pos()) {
            let per_px = total / f64::from(layout.width().max(1.0));
            if start_scroll >= 0.0 {
                self.scroll = (start_scroll + f64::from(p.x - start_x) * per_px).max(0.0);
            } else {
                // Grabbed beside the thumb: centre the view there.
                let page = f64::from(layout.width()) / f64::from(self.zoom);
                self.scroll = (f64::from(p.x - left) * per_px - page / 2.0).max(0.0);
            }
        }
        if bar.drag_stopped() {
            self.scrollbar_drag = None;
        }
    }

    /// The start of the frame under screen x, inside the timeline: where
    /// an Alt+click or an Alt+drag puts the playhead.
    pub(super) fn frame_under(&self, left: f32, x: f32, data: &TimelineData) -> f64 {
        let t = self.time_inside(left, x, data);
        if data.fps <= 0.0 {
            return t;
        }
        // A hair over, so a time on a frame's start stays on that frame.
        (t * data.fps + 1e-6).floor() / data.fps
    }

    /// Where an Alt+click on `r` puts the playhead: a click as egui tells
    /// it (pressed and let go without dragging) with Alt held. Alt+drag
    /// keeps its own meaning (no snapping, or scrubbing).
    pub(super) fn alt_click(
        &self,
        ui: &Ui,
        r: &egui::Response,
        left: f32,
        data: &TimelineData,
    ) -> Option<f64> {
        if !r.clicked_by(egui::PointerButton::Primary) || !ui.input(|i| i.modifiers.alt) {
            return None;
        }
        let p = r.interact_pointer_pos()?;
        Some(self.frame_under(left, p.x, data))
    }

    /// Whether a click in the ruler moves the edit needle: so set, and the
    /// timeline has a needle of its own.
    pub(super) fn needle_moves(&self, data: &TimelineData) -> bool {
        self.click_moves == ClickMoves::EditNeedle && data.needle.is_some()
    }

    /// The divider between the video and the audio area: drag it to share
    /// the body between them.
    pub(super) fn divider_input(&mut self, ui: &Ui, layout: &Layout, data: &TimelineData, id: egui::Id) {
        let divider = ui
            .interact(
                layout.divider.expand2(vec2(0.0, 2.0)),
                id.with("divider"),
                Sense::drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical);
        if divider.drag_started() {
            // From where the divider is drawn, which the video tracks may
            // have pushed past the place it was last dragged to.
            self.split = layout.video_share();
        }
        if divider.dragged() {
            let body = layout.video.height() + layout.audio.height();
            self.split = (self.split + divider.drag_delta().y / body.max(1.0)).clamp(0.1, 0.9);
            // Dragged to where the video tracks don't all show, it stays
            // there: that was asked for.
            self.fit_video = self.split + 0.5 / body.max(1.0) >= self.share_for_video_tracks(body, data);
        }
    }

    /// The label drawn nearest to `x`, if one is within a few pixels.
    pub(super) fn marker_near<'m>(
        &self,
        left: f32,
        markers: &'m [TimelineMarker],
        x: f32,
    ) -> Option<&'m TimelineMarker> {
        markers
            .iter()
            .map(|m| (m, (self.x_of(left, m.at) - x).abs()))
            .filter(|(_, d)| *d <= 7.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(m, _)| m)
    }

    /// The ruler and the labels row's background, the ruler's marks and
    /// times, and the stretches the page highlights.
    pub(super) fn draw_ruler_row(
        &self,
        ui: &Ui,
        theme: &Theme,
        data: &TimelineData,
        rect: Rect,
        layout: &Layout,
    ) {
        let left = layout.time_left();
        let painter = ui.painter_at(rect);
        // Ruler and labels row.
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), rect.top()), layout.labels.right_bottom()),
            0.0,
            theme.panel,
        );
        draw_ruler(&ui.painter_at(layout.ruler), theme, self, layout, data.fps);
        if !data.highlights.is_empty() {
            let body = Rect::from_min_max(
                pos2(left, layout.labels.bottom()),
                pos2(rect.right(), layout.scrollbar.top()),
            );
            let mark = ui.painter_at(body);
            let fill = crate::status::ERROR.gamma_multiply(0.22);
            for &(a, b) in data.highlights {
                let (x0, x1) = (self.x_of(left, a), self.x_of(left, b));
                if x1 >= body.left() && x0 <= body.right() {
                    mark.rect_filled(
                        Rect::from_min_max(pos2(x0, body.top()), pos2(x1.max(x0 + 1.0), body.bottom())),
                        0.0,
                        fill,
                    );
                }
            }
        }
    }

    /// The region (or the one being drawn): everything outside it
    /// darkened, and its bar with round handles in the ruler.
    pub(super) fn draw_region(
        &self,
        ui: &Ui,
        theme: &Theme,
        data: &TimelineData,
        rect: Rect,
        layout: &Layout,
    ) {
        let left = layout.time_left();
        let region = self.region_drag.map(|(_, r)| r).or(data.region);
        if let Some((a, b)) = region {
            let (x0, x1) = (self.x_of(left, a), self.x_of(left, b));
            // Everything outside the region is darkened.
            let body = Rect::from_min_max(
                pos2(left, layout.labels.bottom()),
                pos2(rect.right(), layout.scrollbar.top()),
            );
            let shade = ui.painter_at(body);
            let dim = Color32::from_black_alpha(90);
            shade.rect_filled(Rect::from_min_max(body.min, pos2(x0, body.bottom())), 0.0, dim);
            shade.rect_filled(Rect::from_min_max(pos2(x1, body.top()), body.max), 0.0, dim);
            // The bar and its round handles.
            let rp = ui.painter_at(layout.ruler.expand2(vec2(8.0, 0.0)));
            let y = layout.ruler.bottom() - 4.0;
            rp.rect_filled(
                Rect::from_min_max(pos2(x0, y - 2.5), pos2(x1, y + 2.5)),
                0.0,
                theme.accent.gamma_multiply(0.6),
            );
            for x in [x0, x1] {
                rp.circle_filled(pos2(x, y), 5.0, theme.accent);
            }
        }
    }

    /// The labels row: the cache bar along its top edge, and each label's
    /// mark and name (only those in view).
    pub(super) fn draw_labels(
        &self,
        ui: &Ui,
        theme: &Theme,
        data: &TimelineData,
        layout: &Layout,
        visible: (f64, f64),
    ) {
        let left = layout.time_left();
        let label_painter = ui.painter_at(layout.labels);
        cache_bar::draw(
            &label_painter,
            theme,
            layout.cache_strip(),
            &data.cache,
            visible,
            |t| self.x_of(left, t),
        );
        let labels_seen = label_painter.clip_rect().x_range();
        for marker in data.markers {
            let x = self.x_of(left, marker.at);
            // Out of view: its mark and its name (never wider than a
            // label's size per byte) would be clipped away whole.
            let widest = marker.name.len() as f32 * LABEL_SIZE;
            if x + 8.0 + widest < labels_seen.min || x - 5.0 > labels_seen.max {
                continue;
            }
            label_painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 5.0, layout.labels.top() + cache_bar::HEIGHT + 1.0),
                    pos2(x + 5.0, layout.labels.top() + cache_bar::HEIGHT + 1.0),
                    pos2(x, layout.labels.bottom() - 3.0),
                ],
                marker.colour,
                Stroke::NONE,
            ));
            label_painter.text(
                pos2(x + 8.0, layout.labels.center().y),
                Align2::LEFT_CENTER,
                &marker.name,
                FontId::proportional(LABEL_SIZE),
                theme.muted,
            );
        }
    }

    /// The scroll bar along the bottom and its thumb.
    pub(super) fn draw_scrollbar(
        &self,
        painter: &egui::Painter,
        theme: &Theme,
        data: &TimelineData,
        layout: &Layout,
    ) {
        let (thumb, _) = scrollbar_thumb(layout, self, data.length);
        painter.rect_filled(layout.scrollbar, 0.0, theme.bg);
        painter.rect_filled(thumb.shrink(3.0), 4.0, theme.line_strong);
    }

    /// The playhead and the edit needle, over everything in the time area.
    pub(super) fn draw_playhead_and_needle(
        &self,
        ui: &Ui,
        theme: &Theme,
        data: &TimelineData,
        rect: Rect,
        layout: &Layout,
    ) {
        let left = layout.time_left();
        let over = ui.painter_at(Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(rect.right(), layout.scrollbar.top()),
        ));
        let ph = self.x_of(left, data.playhead);
        over.line_segment(
            [pos2(ph, rect.top()), pos2(ph, layout.scrollbar.top())],
            Stroke::new(1.5, theme.text),
        );
        draw_playhead_handle(
            &over,
            theme,
            ph,
            layout,
            self.needle_moves(data),
            self.playhead_lit,
        );
        if let Some(needle) = self.needle_drag.or(data.needle) {
            let nx = self.x_of(left, needle);
            over.line_segment(
                [
                    pos2(nx, layout.ruler.top() + 8.0),
                    pos2(nx, layout.scrollbar.top()),
                ],
                Stroke::new(1.5, crate::status::EDIT_NEEDLE),
            );
            over.add(Shape::convex_polygon(
                needle_handle(nx, layout),
                crate::status::EDIT_NEEDLE,
                Stroke::NONE,
            ));
        }
    }
}

/// Where the playhead's handle can be taken hold of: its head at the top
/// of the ruler, a little wider than it is drawn.
fn playhead_grab(x: f32, layout: &Layout) -> Rect {
    Rect::from_min_max(
        pos2(x - 8.0, layout.ruler.top()),
        pos2(x + 8.0, layout.ruler.top() + 18.0),
    )
}

/// The playhead's head at the top of the ruler. When a click in the ruler
/// moves the edit needle (`grab`), the head is the way to move the
/// playhead: drawn larger, with grip lines; lit while pointed at or dragged.
fn draw_playhead_handle(p: &egui::Painter, theme: &Theme, x: f32, layout: &Layout, grab: bool, lit: bool) {
    let top = layout.ruler.top();
    let (half, side, tip) = if grab { (7.0, 11.0, 18.0) } else { (6.0, 8.0, 14.0) };
    let colour = if lit { theme.accent } else { theme.text };
    p.add(Shape::convex_polygon(
        vec![
            pos2(x - half, top),
            pos2(x + half, top),
            pos2(x + half, top + side),
            pos2(x, top + tip),
            pos2(x - half, top + side),
        ],
        colour,
        Stroke::NONE,
    ));
    if grab {
        for dx in [-2.5, 0.0, 2.5] {
            p.line_segment(
                [pos2(x + dx, top + 3.0), pos2(x + dx, top + 9.0)],
                Stroke::new(1.0, theme.panel),
            );
        }
    }
}

fn needle_handle(x: f32, layout: &Layout) -> Vec<Pos2> {
    let b = layout.ruler.bottom();
    vec![
        pos2(x - 6.0, b - 12.0),
        pos2(x + 6.0, b - 12.0),
        pos2(x + 6.0, b - 5.0),
        pos2(x, b),
        pos2(x - 6.0, b - 5.0),
    ]
}

/// The thumb's rectangle and the total time the bar spans.
fn scrollbar_thumb(layout: &Layout, view: &TimelineView, length: f64) -> (Rect, f64) {
    let page = f64::from(layout.width()) / f64::from(view.zoom);
    let total = (length * 1.1).max(view.scroll + page).max(page);
    let w = f64::from(layout.width());
    let x0 = layout.scrollbar.left() + (view.scroll / total * w) as f32;
    let x1 = layout.scrollbar.left() + ((view.scroll + page) / total * w) as f32;
    (
        Rect::from_min_max(
            pos2(x0, layout.scrollbar.top()),
            pos2(x1.max(x0 + 20.0), layout.scrollbar.bottom()),
        ),
        total,
    )
}

/// The step between ruler marks, in seconds: the smallest that leaves at
/// least `min_px` between labels.
pub fn ruler_step(zoom: f32, fps: f64, min_px: f32) -> f64 {
    let frame = 1.0 / fps.max(1.0);
    let steps = [
        frame,
        2.0 * frame,
        5.0 * frame,
        10.0 * frame,
        1.0,
        2.0,
        5.0,
        10.0,
        15.0,
        30.0,
        60.0,
        120.0,
        300.0,
        600.0,
        900.0,
        1800.0,
        3600.0,
    ];
    steps
        .into_iter()
        .find(|s| (*s * f64::from(zoom)) as f32 >= min_px)
        .unwrap_or(3600.0)
}

/// The step between the small ruler marks under a label step of `step`
/// seconds: a fifth of it, but never finer than a frame, so zoomed all the
/// way in there is one mark per frame.
pub fn minor_step(step: f64, fps: f64) -> f64 {
    (step / 5.0).max(1.0 / fps.max(1.0))
}

fn draw_ruler(p: &egui::Painter, theme: &Theme, view: &TimelineView, layout: &Layout, fps: f64) {
    let left = layout.time_left();
    let step = ruler_step(view.zoom, fps, 90.0);
    let minor = minor_step(step, fps);
    let start = (view.scroll / minor).floor() * minor;
    let end = view.time_at(left, layout.ruler.right());
    let mut t = start;
    let mut i = 0;
    while t <= end && i < 10_000 {
        let x = view.x_of(left, t);
        let major = ((t / step).round() * step - t).abs() < minor * 0.01;
        let h = if major { 10.0 } else { 5.0 };
        p.line_segment(
            [pos2(x, layout.ruler.bottom() - h), pos2(x, layout.ruler.bottom())],
            Stroke::new(1.0, theme.line_strong),
        );
        if major {
            p.text(
                pos2(x + 4.0, layout.ruler.top() + 4.0),
                Align2::LEFT_TOP,
                crate::time_text::timecode(t, fps),
                crate::fonts::mono(10.5),
                theme.muted,
            );
        }
        t = start + minor * f64::from(i + 1);
        i += 1;
    }
}
