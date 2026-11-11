//! The timeline: one custom widget, the same on every page that shows one.
//!
//! It draws only what is visible (clips outside the view are skipped), so
//! long projects cost the same as short ones. See `docs/research/timeline.md`.
//!
//! Layout, top to bottom: the ruler (timecode from 00:00:00:00), the labels
//! row, the video area, a draggable divider, the audio area, and a scroll bar
//! along the bottom. Track names sit in a column on the left. Video and audio
//! scroll up and down separately.
//!
//! The widget knows nothing about projects: the page hands it plain data
//! (seconds as `f64`, for drawing only) and gets back [`TimelineEvent`]s.

use crate::Theme;
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

/// One clip as the timeline shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineClip {
    pub id: u64,
    pub start: f64,
    pub duration: f64,
    pub name: String,
    /// Shown when the pointer rests on the clip.
    pub info: String,
    pub selected: bool,
    /// Its file can't be found.
    pub missing: bool,
}

/// One track as the timeline shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTrack {
    pub id: u64,
    pub name: String,
    pub clips: Vec<TimelineClip>,
    pub muted: bool,
    pub solo: bool,
}

/// A timeline label (a named, coloured marker).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMarker {
    pub at: f64,
    pub name: String,
    pub colour: Color32,
}

/// What to draw.
#[derive(Debug, Clone, Copy)]
pub struct TimelineData<'a> {
    /// Video tracks, V1 first (drawn at the bottom of the video area).
    pub video: &'a [TimelineTrack],
    /// Audio tracks, A1 first (drawn at the top of the audio area).
    pub audio: &'a [TimelineTrack],
    pub length: f64,
    /// Frames per second, for the ruler's timecode.
    pub fps: f64,
    pub playhead: f64,
    /// The red edit needle; `None` when merged with the playhead.
    pub needle: Option<f64>,
    pub region: Option<(f64, f64)>,
    pub markers: &'a [TimelineMarker],
    /// Keep the playhead in view (while playing).
    pub follow: bool,
}

/// Something the person did on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimelineEvent {
    /// Click or drag in the ruler: move the playhead there.
    Seek(f64),
    /// The edit needle was dropped here.
    MoveNeedle(f64),
    /// A clip was clicked; `add` with Ctrl or Shift held.
    Select { clip: u64, add: bool },
    /// Empty space was clicked.
    ClearSelection,
}

/// Where the timeline is looking. Kept by the page between frames.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineView {
    /// Pixels per second.
    pub zoom: f32,
    /// Time at the left edge, in seconds.
    pub scroll: f64,
    pub video_scroll: f32,
    pub audio_scroll: f32,
    /// Share of the body given to video (the divider's place).
    pub split: f32,
    /// The edit needle while it is being dragged.
    needle_drag: Option<f64>,
    scrollbar_drag: Option<(f32, f64)>,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView {
            zoom: 60.0,
            scroll: 0.0,
            video_scroll: 0.0,
            audio_scroll: 0.0,
            split: 0.5,
            needle_drag: None,
            scrollbar_drag: None,
        }
    }
}

pub const HEADER_WIDTH: f32 = 140.0;
const RULER: f32 = 26.0;
const LABELS: f32 = 18.0;
const DIVIDER: f32 = 6.0;
const SCROLLBAR: f32 = 14.0;
pub const VIDEO_TRACK: f32 = 54.0;
pub const AUDIO_TRACK: f32 = 46.0;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 2400.0;

/// Colours of audio tracks, one per track in turn.
const AUDIO_COLOURS: [Color32; 6] = [
    Color32::from_rgb(0x3E, 0x8E, 0x7E),
    Color32::from_rgb(0x5B, 0x7F, 0xC4),
    Color32::from_rgb(0x9A, 0x6C, 0xC4),
    Color32::from_rgb(0xC4, 0x7A, 0x4F),
    Color32::from_rgb(0x6F, 0x9E, 0x4C),
    Color32::from_rgb(0xB8, 0x5C, 0x7A),
];

/// The areas the timeline is made of.
#[derive(Debug, Clone, Copy)]
struct Layout {
    ruler: Rect,
    labels: Rect,
    video: Rect,
    divider: Rect,
    audio: Rect,
    scrollbar: Rect,
    /// Left of the time areas: track names.
    header_x: f32,
}

impl Layout {
    fn new(rect: Rect, split: f32) -> Layout {
        let left = rect.left() + HEADER_WIDTH;
        let ruler = Rect::from_min_max(pos2(left, rect.top()), pos2(rect.right(), rect.top() + RULER));
        let labels = Rect::from_min_max(
            pos2(left, ruler.bottom()),
            pos2(rect.right(), ruler.bottom() + LABELS),
        );
        let scrollbar = Rect::from_min_max(pos2(left, rect.bottom() - SCROLLBAR), rect.right_bottom());
        let body_top = labels.bottom();
        let body_h = (scrollbar.top() - body_top - DIVIDER).max(20.0);
        let video_h = (body_h * split.clamp(0.1, 0.9)).round();
        let video = Rect::from_min_max(
            pos2(rect.left(), body_top),
            pos2(rect.right(), body_top + video_h),
        );
        let divider = Rect::from_min_max(
            pos2(rect.left(), video.bottom()),
            pos2(rect.right(), video.bottom() + DIVIDER),
        );
        let audio = Rect::from_min_max(
            pos2(rect.left(), divider.bottom()),
            pos2(rect.right(), scrollbar.top()),
        );
        Layout {
            ruler,
            labels,
            video,
            divider,
            audio,
            scrollbar,
            header_x: rect.left(),
        }
    }

    fn time_left(&self) -> f32 {
        self.ruler.left()
    }

    fn width(&self) -> f32 {
        self.ruler.width()
    }
}

impl TimelineView {
    /// Screen x of time `t`.
    pub fn x_of(&self, left: f32, t: f64) -> f32 {
        left + ((t - self.scroll) * f64::from(self.zoom)) as f32
    }

    /// Time at screen x.
    pub fn time_at(&self, left: f32, x: f32) -> f64 {
        self.scroll + f64::from(x - left) / f64::from(self.zoom)
    }

    /// Zooms by `factor`, keeping the time under screen x `anchor` in place.
    pub fn zoom_around(&mut self, left: f32, anchor: f32, factor: f32) {
        let t = self.time_at(left, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll = (t - f64::from(anchor - left) / f64::from(self.zoom)).max(0.0);
    }

    /// Fits `length` seconds in `width` pixels.
    pub fn fit(&mut self, length: f64, width: f32) {
        if length > 0.0 && width > 0.0 {
            self.zoom = ((width * 0.95) / length as f32).clamp(MIN_ZOOM, MAX_ZOOM);
            self.scroll = 0.0;
        }
    }

    /// Draws the timeline in `size` and returns what was done to it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        data: &TimelineData,
        size: Vec2,
    ) -> Vec<TimelineEvent> {
        let mut events = Vec::new();
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let layout = Layout::new(rect, self.split);
        let left = layout.time_left();
        let visible = (self.time_at(left, left), self.time_at(left, rect.right()));

        if data.follow {
            let page = f64::from(layout.width()) / f64::from(self.zoom);
            if data.playhead < visible.0 || data.playhead > visible.1 - page * 0.05 {
                self.scroll = (data.playhead - page * 0.1).max(0.0);
            }
        }

        self.input(ui, &layout, data, &mut events);
        let visible = (self.time_at(left, left), self.time_at(left, rect.right()));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.panel);

        // Body: video tracks bottom-up above the divider, audio top-down below it.
        let video_rows = rows(
            &layout.video,
            data.video.len(),
            VIDEO_TRACK,
            self.video_scroll,
            true,
        );
        let audio_rows = rows(
            &layout.audio,
            data.audio.len(),
            AUDIO_TRACK,
            self.audio_scroll,
            false,
        );
        for (area, tracks, rows, audio) in [
            (layout.video, data.video, &video_rows, false),
            (layout.audio, data.audio, &audio_rows, true),
        ] {
            let clip_painter = ui.painter_at(Rect::from_min_max(pos2(left, area.top()), area.max));
            let header_painter = ui.painter_at(Rect::from_min_max(area.min, pos2(left, area.bottom())));
            for (i, (track, row)) in tracks.iter().zip(rows).enumerate() {
                if !row.intersects(area) {
                    continue;
                }
                let band = Rect::from_min_max(pos2(left, row.top()), pos2(area.right(), row.bottom()));
                clip_painter.rect_filled(band.shrink2(vec2(0.0, 1.0)), 0.0, theme.bg);
                header_painter.text(
                    pos2(layout.header_x + 12.0, row.center().y),
                    Align2::LEFT_CENTER,
                    &track.name,
                    FontId::proportional(12.5),
                    if track.muted { theme.faint } else { theme.text },
                );
                let colour = if audio {
                    AUDIO_COLOURS[i % AUDIO_COLOURS.len()]
                } else {
                    theme.line_strong
                };
                // Clips a few pixels wide can't be told apart: neighbours
                // merge into one shape, so zooming far out stays cheap.
                let mut merged: Option<Rect> = None;
                for clip in &track.clips {
                    if clip.start + clip.duration < visible.0 || clip.start > visible.1 {
                        continue;
                    }
                    let x0 = self.x_of(left, clip.start);
                    let x1 = self.x_of(left, clip.start + clip.duration).max(x0 + 1.0);
                    let r = Rect::from_min_max(pos2(x0, row.top() + 3.0), pos2(x1, row.bottom() - 3.0));
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
                    draw_clip(&clip_painter, theme, r, clip, colour, audio);
                }
                if let Some(m) = merged {
                    clip_painter.rect_filled(m, 1.0, tiny_fill(theme, colour, audio));
                }
            }
        }
        // Track header column and its edge.
        painter.line_segment(
            [
                pos2(left, layout.labels.bottom()),
                pos2(left, layout.scrollbar.top()),
            ],
            Stroke::new(1.0, theme.line),
        );
        painter.rect_filled(layout.divider, 0.0, theme.line);

        // Ruler and labels row.
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), rect.top()), layout.labels.right_bottom()),
            0.0,
            theme.panel,
        );
        draw_ruler(&ui.painter_at(layout.ruler), theme, self, &layout, data.fps);
        if let Some((a, b)) = data.region {
            let (x0, x1) = (self.x_of(left, a), self.x_of(left, b));
            let r = Rect::from_min_max(
                pos2(x0, layout.ruler.bottom() - 6.0),
                pos2(x1, layout.ruler.bottom()),
            );
            ui.painter_at(layout.ruler)
                .rect_filled(r, 2.0, theme.accent.gamma_multiply(0.5));
        }
        let label_painter = ui.painter_at(layout.labels);
        for marker in data.markers {
            let x = self.x_of(left, marker.at);
            label_painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 5.0, layout.labels.top() + 3.0),
                    pos2(x + 5.0, layout.labels.top() + 3.0),
                    pos2(x, layout.labels.bottom() - 3.0),
                ],
                marker.colour,
                Stroke::NONE,
            ));
            label_painter.text(
                pos2(x + 8.0, layout.labels.center().y),
                Align2::LEFT_CENTER,
                &marker.name,
                FontId::proportional(10.5),
                theme.muted,
            );
        }

        // Scroll bar.
        let (thumb, _) = scrollbar_thumb(&layout, self, data.length);
        painter.rect_filled(layout.scrollbar, 0.0, theme.bg);
        painter.rect_filled(thumb.shrink(3.0), 4.0, theme.line_strong);

        // Playhead and edit needle over everything in the time area.
        let over = ui.painter_at(Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(rect.right(), layout.scrollbar.top()),
        ));
        let ph = self.x_of(left, data.playhead);
        over.line_segment(
            [pos2(ph, rect.top()), pos2(ph, layout.scrollbar.top())],
            Stroke::new(1.5, theme.text),
        );
        over.add(Shape::convex_polygon(
            vec![
                pos2(ph - 6.0, rect.top()),
                pos2(ph + 6.0, rect.top()),
                pos2(ph + 6.0, rect.top() + 8.0),
                pos2(ph, rect.top() + 14.0),
                pos2(ph - 6.0, rect.top() + 8.0),
            ],
            theme.text,
            Stroke::NONE,
        ));
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
                needle_handle(nx, &layout),
                crate::status::EDIT_NEEDLE,
                Stroke::NONE,
            ));
        }
        events
    }

    fn input(&mut self, ui: &mut Ui, layout: &Layout, data: &TimelineData, events: &mut Vec<TimelineEvent>) {
        let left = layout.time_left();
        let id = ui.scope_id().with("timeline");

        // Wheel: Ctrl zooms at the pointer, Shift scrolls sideways, plain scrolls the area under the pointer.
        let hover = ui.input(|i| i.pointer.hover_pos());
        let (wheel, mods) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        if let Some(pos) = hover
            && Rect::from_min_max(pos2(layout.header_x, layout.ruler.top()), layout.scrollbar.max)
                .contains(pos)
            && wheel != Vec2::ZERO
        {
            if mods.command {
                self.zoom_around(left, pos.x.max(left), (wheel.y / 200.0).exp());
            } else if mods.shift || wheel.x != 0.0 {
                let dx = if mods.shift { wheel.y } else { wheel.x };
                self.scroll = (self.scroll - f64::from(dx) / f64::from(self.zoom)).max(0.0);
            } else if layout.video.contains(pos) {
                let content = data.video.len() as f32 * VIDEO_TRACK;
                self.video_scroll =
                    (self.video_scroll + wheel.y).clamp(0.0, (content - layout.video.height()).max(0.0));
            } else if layout.audio.contains(pos) {
                let content = data.audio.len() as f32 * AUDIO_TRACK;
                self.audio_scroll =
                    (self.audio_scroll - wheel.y).clamp(0.0, (content - layout.audio.height()).max(0.0));
            }
        }

        // The edit needle's handle, dragged along the ruler.
        if let Some(needle) = data.needle {
            let nx = self.x_of(left, needle);
            let handle = Rect::from_center_size(pos2(nx, layout.ruler.bottom() - 6.0), vec2(14.0, 14.0));
            let r = ui
                .interact(handle, id.with("needle"), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                self.needle_drag = Some(self.time_at(left, p.x).clamp(0.0, data.length.max(0.0)));
            }
            if r.drag_stopped()
                && let Some(t) = self.needle_drag.take()
            {
                events.push(TimelineEvent::MoveNeedle(t));
            }
            if r.dragged() || r.drag_stopped() {
                return;
            }
        }

        // Ruler: click or drag to move the playhead.
        let ruler = ui.interact(
            layout.ruler.union(layout.labels),
            id.with("ruler"),
            Sense::click_and_drag(),
        );
        if (ruler.clicked() || ruler.dragged())
            && let Some(p) = ruler.interact_pointer_pos()
        {
            events.push(TimelineEvent::Seek(
                self.time_at(left, p.x).clamp(0.0, data.length.max(0.0)),
            ));
        }

        // Divider.
        let divider = ui
            .interact(
                layout.divider.expand2(vec2(0.0, 2.0)),
                id.with("divider"),
                Sense::drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical);
        if divider.dragged() {
            let body = layout.audio.bottom() - layout.video.top();
            self.split = (self.split + divider.drag_delta().y / body.max(1.0)).clamp(0.1, 0.9);
        }

        // Scroll bar: drag the thumb.
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

        // Clicks on clips select; clicks on empty space clear the selection.
        for (area, tracks, height, from_bottom, scroll) in [
            (layout.video, data.video, VIDEO_TRACK, true, self.video_scroll),
            (layout.audio, data.audio, AUDIO_TRACK, false, self.audio_scroll),
        ] {
            let time_area = Rect::from_min_max(pos2(left, area.top()), area.max);
            let r = ui.interact(time_area, id.with(("area", from_bottom)), Sense::click());
            let pointer = if r.clicked() {
                r.interact_pointer_pos()
            } else {
                r.hover_pos()
            };
            let Some(p) = pointer else {
                continue;
            };
            let row_rects = rows(&area, tracks.len(), height, scroll, from_bottom);
            let t = self.time_at(left, p.x);
            let hit = tracks
                .iter()
                .zip(&row_rects)
                .filter(|(_, row)| row.y_range().contains(p.y))
                .flat_map(|(track, _)| track.clips.iter())
                .find(|c| c.start <= t && t < c.start + c.duration);
            if !r.clicked() {
                if let Some(c) = hit.filter(|c| !c.info.is_empty()) {
                    r.on_hover_text_at_pointer(&c.info);
                }
                continue;
            }
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            events.push(match hit {
                Some(c) => TimelineEvent::Select { clip: c.id, add },
                None => TimelineEvent::ClearSelection,
            });
        }
    }
}

/// Track rows inside `area`. Video rows grow upward from the area's bottom
/// (V1 lowest); audio rows grow downward from its top.
fn rows(area: &Rect, count: usize, height: f32, scroll: f32, from_bottom: bool) -> Vec<Rect> {
    (0..count)
        .map(|i| {
            let i = i as f32;
            if from_bottom {
                let bottom = area.bottom() - i * height + scroll;
                Rect::from_min_max(pos2(area.left(), bottom - height), pos2(area.right(), bottom))
            } else {
                let top = area.top() + i * height - scroll;
                Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + height))
            }
        })
        .collect()
}

fn tiny_fill(theme: &Theme, colour: Color32, audio: bool) -> Color32 {
    if audio {
        colour.gamma_multiply(0.55)
    } else {
        theme.line_strong
    }
}

fn draw_clip(p: &egui::Painter, theme: &Theme, r: Rect, clip: &TimelineClip, colour: Color32, audio: bool) {
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
    if clip.missing {
        p.rect_stroke(
            r,
            4.0,
            Stroke::new(1.5, crate::status::ERROR),
            egui::StrokeKind::Inside,
        );
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
        let pos = pos2(r.left() + 6.0, r.top() + 6.0);
        p.with_clip_rect(r.shrink(2.0)).galley(pos, galley, theme.text);
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

/// `HH:MM:SS:FF` for `seconds` at `fps` (whole frames per second for the frame part).
pub fn timecode(seconds: f64, fps: f64) -> String {
    let nominal = fps.round().max(1.0) as i64;
    let frames = (seconds * fps).round() as i64;
    let (total_secs, ff) = (frames / nominal, frames % nominal);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total_secs / 3600,
        total_secs / 60 % 60,
        total_secs % 60,
        ff
    )
}

fn draw_ruler(p: &egui::Painter, theme: &Theme, view: &TimelineView, layout: &Layout, fps: f64) {
    let left = layout.time_left();
    let step = ruler_step(view.zoom, fps, 90.0);
    let minor = step / 5.0;
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
                timecode(t, fps),
                crate::fonts::mono(10.5),
                theme.muted,
            );
        }
        t = start + minor * f64::from(i + 1);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_screen_positions_round_trip() {
        let mut v = TimelineView {
            zoom: 100.0,
            scroll: 2.0,
            ..Default::default()
        };
        assert_eq!(v.x_of(140.0, 3.0), 240.0);
        assert!((v.time_at(140.0, 240.0) - 3.0).abs() < 1e-9);
        v.zoom_around(140.0, 240.0, 2.0);
        assert!(
            (v.time_at(140.0, 240.0) - 3.0).abs() < 1e-6,
            "the time under the pointer stays put"
        );
        assert_eq!(v.zoom, 200.0);
        v.fit(100.0, 1000.0);
        assert!((v.zoom - 9.5).abs() < 1e-4);
        assert_eq!(v.scroll, 0.0);
    }

    #[test]
    fn ruler_marks_and_timecode() {
        assert_eq!(ruler_step(100.0, 60.0, 90.0), 1.0);
        assert_eq!(ruler_step(10.0, 60.0, 90.0), 10.0);
        assert!(
            (ruler_step(6000.0, 60.0, 90.0) - 1.0 / 60.0).abs() < 1e-9,
            "frames when zoomed in"
        );
        assert_eq!(timecode(0.0, 60.0), "00:00:00:00");
        assert_eq!(timecode(3661.5, 60.0), "01:01:01:30");
        assert_eq!(
            timecode(10.0, 60_000.0 / 1001.0),
            "00:00:09:59",
            "59.94: frame counts run slightly behind the clock"
        );
    }

    #[test]
    fn video_rows_grow_up_and_audio_rows_down() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0));
        let v = rows(&area, 2, 50.0, 0.0, true);
        assert_eq!((v[0].top(), v[0].bottom()), (150.0, 200.0), "V1 at the bottom");
        assert_eq!(v[1].bottom(), 150.0);
        let a = rows(&area, 2, 40.0, 10.0, false);
        assert_eq!((a[0].top(), a[1].top()), (-10.0, 30.0));
    }
}
