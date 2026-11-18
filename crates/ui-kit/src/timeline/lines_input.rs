//! Volume and pan lines worked by the pointer: drawn on every track and
//! clip (with their points when they are the lines shown), and, with
//! lines shown, a click adds a point, a drag moves one, a double-click or
//! the menu removes it.

use super::{
    Layout, LineKind, LineOwner, Rows, TimelineData, TimelineEvent, TimelineView, find_clip, lines,
    track_views,
};
use egui::{CursorIcon, Pos2, Rect, Sense, Ui, pos2};

impl TimelineView {
    /// Draws a track's or a clip's line in `rect`: with its points when
    /// it is the kind of line shown, faintly when it has points but isn't,
    /// not at all otherwise. A line being dragged is drawn as it is now.
    pub(super) fn draw_line(
        &self,
        ui: &Ui,
        p: &egui::Painter,
        owner: LineOwner,
        points: Option<&[(f64, f32)]>,
        rect: Rect,
        x_of: impl Fn(f64) -> f32,
    ) {
        let editable = self
            .lines
            .is_some_and(|l| l.on_clips == matches!(owner, LineOwner::Clip(_)));
        let kind = self.lines.map_or(LineKind::Volume, |l| l.kind);
        let drag = self.line_drag.as_ref().filter(|d| d.owner == owner);
        let points = drag.map(|d| d.points.as_slice()).or(points).unwrap_or_default();
        if !editable && points.is_empty() {
            return;
        }
        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|pos| rect.contains(*pos) && p.clip_rect().contains(*pos));
        let lit = drag.map(|d| d.index).or_else(|| {
            hover.and_then(|pos| lines::hit(points, |t, v| pos2(x_of(t), lines::y_of(kind, rect, v)), pos))
        });
        let look = lines::Look {
            kind,
            editable,
            lit: lit.filter(|_| editable),
            tag_x: drag
                .and(hover.or_else(|| ui.input(|i| i.pointer.hover_pos())))
                .map(|h| h.x),
        };
        lines::draw(p, rect, points, x_of, &look);
    }

    /// With lines shown, the pointer over the audio tracks works the line
    /// of the row (or clip) under it: a click adds a point, a drag moves
    /// one, a double-click or its menu removes it. Each change is one
    /// [`TimelineEvent::Line`].
    pub(super) fn lines_input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        rows: &Rows,
        events: &mut Vec<TimelineEvent>,
    ) {
        let Some(shown) = self.lines.filter(|_| !self.compact) else {
            self.line_drag = None;
            return;
        };
        let left = layout.time_left();
        let area = Rect::from_min_max(pos2(left, layout.audio.top()), layout.audio.max);
        let r = ui.interact(
            area,
            ui.scope_id().with("timeline-lines"),
            Sense::click_and_drag(),
        );
        let frame = 1.0 / data.fps.max(1.0);
        let kind = shown.kind;
        // A point being dragged follows the pointer until let go.
        let pointer_t = r.interact_pointer_pos().map(|p| self.time_at(left, p.x));
        if let Some(drag) = &mut self.line_drag {
            if let Some(p) = r.interact_pointer_pos().filter(|_| r.dragged())
                && let Some(t) = pointer_t
            {
                let t = (t / frame).round() * frame;
                let fine = ui.input(|i| i.modifiers.shift || i.modifiers.alt);
                let value = drag.follow(p.y, fine);
                drag.points = lines::moved(&drag.points, drag.index, (t, value), frame, drag.bounds);
            }
            if r.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                events.push(TimelineEvent::Line {
                    owner: drag.owner,
                    kind: drag.kind,
                    points: drag.points.clone(),
                });
                self.line_drag = None;
            }
            return;
        }
        let pointer = r.interact_pointer_pos().or(r.hover_pos());
        let pointer = if r.drag_started() {
            ui.input(|i| i.pointer.press_origin()).or(pointer)
        } else {
            pointer
        };
        // Whose line is under the pointer, where it is drawn and where its
        // points may go.
        let inset = track_views::clip_inset(self.compact);
        let under = pointer.and_then(|p| {
            let row = rows.at(p.y)?;
            let (track, rect) = (data.audio.get(row)?, *rows.rects().get(row)?);
            let band = Rect::from_min_max(
                pos2(left, rect.top() + inset),
                pos2(area.right(), rect.bottom() - inset),
            );
            if !shown.on_clips {
                let points = track.line.as_deref().unwrap_or_default().to_vec();
                return Some((
                    LineOwner::Track(track.id),
                    band,
                    points,
                    (0.0, data.length.max(0.0)),
                ));
            }
            let t = self.time_at(left, p.x);
            let clip = track.clip_at(t)?;
            let (x0, x1) = (
                self.x_of(left, clip.start),
                self.x_of(left, clip.start + clip.duration),
            );
            let rect = Rect::from_min_max(pos2(x0, band.top()), pos2(x1, band.bottom()));
            let points = clip.line.as_deref().unwrap_or_default().to_vec();
            Some((
                LineOwner::Clip(clip.id),
                rect,
                points,
                (clip.start, clip.start + clip.duration),
            ))
        });
        let point_at = |p: Pos2, rect: Rect, points: &[(f64, f32)]| {
            lines::hit(
                points,
                |t, v| pos2(self.x_of(left, t), lines::y_of(kind, rect, v)),
                p,
            )
        };
        if let Some((owner, rect, points, bounds)) = &under
            && let Some(p) = pointer
        {
            let hit = point_at(p, *rect, points);
            let r = match hit {
                Some(_) => r.clone().on_hover_cursor(CursorIcon::Grab),
                None => r.clone().on_hover_cursor(CursorIcon::Crosshair),
            };
            let t = ((self.time_at(left, p.x) / frame).round() * frame).clamp(bounds.0, bounds.1);
            if r.drag_started_by(egui::PointerButton::Primary) {
                // A point taken where the button went down (the pointer has
                // moved a little by the time a drag is noticed), or one made
                // there.
                let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
                let hit = point_at(origin, *rect, points);
                let t = ((self.time_at(left, origin.x) / frame).round() * frame).clamp(bounds.0, bounds.1);
                let (points, index) = match hit {
                    Some(i) => (points.clone(), i),
                    None => lines::added(points, t, lines::value_at(kind, *rect, origin.y)),
                };
                let height = points
                    .get(index)
                    .map_or(0.5, |(_, value)| lines::height_of(kind, *value));
                self.line_drag = Some(lines::LineDrag {
                    owner: *owner,
                    kind,
                    index,
                    points,
                    bounds: *bounds,
                    rect: *rect,
                    height,
                    // From where the button went down, so the way moved
                    // before the drag was noticed counts too.
                    last_y: origin.y,
                });
            } else if r.double_clicked() {
                if let Some(i) = hit {
                    events.push(TimelineEvent::Line {
                        owner: *owner,
                        kind,
                        points: lines::removed(points, i),
                    });
                }
            } else if r.clicked() && hit.is_none() {
                let (points, _) = lines::added(points, t, lines::value_at(kind, *rect, p.y));
                events.push(TimelineEvent::Line {
                    owner: *owner,
                    kind,
                    points,
                });
            } else if r.secondary_clicked() {
                self.line_menu = Some((*owner, hit));
            }
        }
        // The menu: remove the point, or clear the whole line.
        let menu = self.line_menu;
        let mut close = false;
        r.context_menu(|ui| {
            let Some((owner, point)) = menu else { return };
            let points = match owner {
                LineOwner::Track(id) => data
                    .audio
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.line.clone()),
                LineOwner::Clip(id) => find_clip(data.audio, id).and_then(|c| c.line.clone()),
            }
            .unwrap_or_else(|| Vec::new().into());
            if let Some(i) = point
                && ui.button("Remove this point").clicked()
            {
                events.push(TimelineEvent::Line {
                    owner,
                    kind,
                    points: lines::removed(&points, i),
                });
                close = true;
            }
            let whose = match owner {
                LineOwner::Track(_) => "track's",
                LineOwner::Clip(_) => "clip's",
            };
            let what = match kind {
                LineKind::Volume => "volume",
                LineKind::Pan => "pan",
            };
            if ui
                .add_enabled(
                    !points.is_empty(),
                    egui::Button::new(format!("Clear this {whose} {what} line")),
                )
                .clicked()
            {
                events.push(TimelineEvent::Line {
                    owner,
                    kind,
                    points: Vec::new(),
                });
                close = true;
            }
            if close {
                ui.close();
            }
        });
    }
}
