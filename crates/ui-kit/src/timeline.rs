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
    /// Clips with the same link move and trim together.
    pub link: Option<u64>,
    /// Loudness to draw inside the clip (audio clips), once it has been read.
    pub waveform: Option<ClipWaveform>,
    /// Pictures to draw inside the clip (video clips), once they have been read.
    pub pictures: Option<ClipPictures>,
}

/// Small pictures along a clip's file: one every `every` seconds from the
/// file's start, already uploaded as textures.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPictures {
    pub every: f64,
    pub textures: std::sync::Arc<[egui::TextureId]>,
    /// Width over height of each picture.
    pub aspect: f32,
    /// Seconds into the file where the clip begins.
    pub source_in: f64,
}

/// A clip's loudness: peaks of its whole file, and where in the file the clip starts.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipWaveform {
    pub peaks: std::sync::Arc<[f32]>,
    pub per_second: f32,
    /// Seconds into the file where the clip begins.
    pub source_in: f64,
}

/// One track as the timeline shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTrack {
    pub id: u64,
    pub name: String,
    pub clips: Vec<TimelineClip>,
    pub muted: bool,
    pub solo: bool,
    /// Volume in decibels.
    pub gain_db: f32,
}

/// A timeline label (a named, coloured marker).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMarker {
    pub id: u64,
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
    /// Stretches to mark over every track (seconds), such as the silences a
    /// removal would cut.
    pub highlights: &'a [(f64, f64)],
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
    /// A clip (with its linked clips) was dragged by `by` seconds.
    Move { clip: u64, by: f64 },
    /// A clip's edge was dragged to `to` (seconds on the timeline).
    Trim { clip: u64, edge: ClipEdge, to: f64 },
    /// The blade tool cut a clip at `at`.
    Blade { clip: u64, at: f64 },
    /// The ripple tool trimmed an edge; later clips follow.
    RippleTrim { clip: u64, edge: ClipEdge, to: f64 },
    /// The roll tool moved the cut after `clip` to `to`.
    Roll { clip: u64, to: f64 },
    /// The slip tool shifted what `clip` shows by `by` seconds of its file.
    Slip { clip: u64, by: f64 },
    /// The slide tool moved `clip` by `by` between its neighbours.
    Slide { clip: u64, by: f64 },
    /// A track's mute, solo or volume changed.
    TrackMix {
        track: u64,
        muted: bool,
        solo: bool,
        gain_db: f32,
    },
    /// The region was drawn or adjusted (`None`: cleared).
    SetRegion(Option<(f64, f64)>),
    /// A dragged file was dropped at `at`: over what is there, or (Ctrl
    /// held) inserted, pushing the rest later.
    DropMedia { media: u64, at: f64, insert: bool },
    /// A timeline label was double-clicked (or "Edit" chosen on it).
    EditLabel(u64),
    /// "Add a label here" from the right-click menu.
    AddLabel(f64),
    /// "Split here" from the right-click menu: one clip (with its linked
    /// clips), or every clip there when `None`.
    SplitAt { at: f64, clip: Option<u64> },
}

/// Where the right-click menu was opened: the time, and the clip under the
/// pointer (id, start, end).
type MenuSpot = (f64, Option<(u64, f64, f64)>);

/// The region while it is being drawn or one of its handles dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RegionDrag {
    /// Shift-drag in the ruler from this time.
    New(f64),
    Start,
    End,
}

/// A file being dragged from a list (the media drawer) onto the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedMedia(pub u64);

/// One end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipEdge {
    Start,
    End,
}

/// What a click or drag on a clip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Select, move, and trim at the edges.
    #[default]
    Select,
    /// Cut clips where clicked.
    Blade,
    /// Trim an edge and move everything after it along, so no gap opens.
    Ripple,
    /// Drag the cut between two touching clips.
    Roll,
    /// Drag inside a clip to show another part of its file in the same place.
    Slip,
    /// Drag a clip between its neighbours; they get longer or shorter.
    Slide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Trim(ClipEdge),
    RippleTrim(ClipEdge),
    /// The cut after the dragged clip.
    Roll,
    Slip,
    Slide,
}

/// A clip being dragged: what is moving and by how much so far.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipDrag {
    clip: u64,
    link: Option<u64>,
    kind: DragKind,
    /// Pointer time when the drag began.
    grabbed_at: f64,
    start: f64,
    end: f64,
    /// Current offset in seconds, after snapping.
    delta: f64,
}

impl ClipDrag {
    fn moves(&self, c: &TimelineClip) -> bool {
        c.id == self.clip || (self.link.is_some() && c.link == self.link)
    }

    /// Where `c` is drawn while the drag lasts.
    fn shown(&self, c: &TimelineClip) -> (f64, f64) {
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
    clip_drag: Option<ClipDrag>,
    /// A track volume being dragged: track and its value so far.
    fader_drag: Option<(u64, f32)>,
    region_drag: Option<(RegionDrag, (f64, f64))>,
    /// Where the right-click menu was opened: time, and the clip under it.
    menu_at: Option<MenuSpot>,
    pub tool: Tool,
    /// Moved and trimmed edges snap to clip edges, the playhead, the needle and labels.
    pub snapping: bool,
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
            clip_drag: None,
            fader_drag: None,
            region_drag: None,
            menu_at: None,
            tool: Tool::Select,
            snapping: true,
        }
    }
}

pub const HEADER_WIDTH: f32 = 140.0;
const RULER: f32 = 26.0;
const LABELS: f32 = 18.0;
const DIVIDER: f32 = 6.0;
const SCROLLBAR: f32 = 14.0;
pub const VIDEO_TRACK: f32 = 54.0;
pub const AUDIO_TRACK: f32 = 58.0;
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
                if audio {
                    self.track_controls(
                        ui,
                        theme,
                        track,
                        Rect::from_min_max(pos2(layout.header_x, row.top()), pos2(left, row.bottom()))
                            .intersect(area),
                        &mut events,
                    );
                }
                let colour = if audio {
                    AUDIO_COLOURS[i % AUDIO_COLOURS.len()]
                } else {
                    theme.line_strong
                };
                // Clips a few pixels wide can't be told apart: neighbours
                // merge into one shape, so zooming far out stays cheap.
                let mut merged: Option<Rect> = None;
                for clip in &track.clips {
                    let (start, duration) = match &self.clip_drag {
                        Some(d) if d.moves(clip) => d.shown(clip),
                        _ => (clip.start, clip.duration),
                    };
                    if start + duration < visible.0 || start > visible.1 {
                        continue;
                    }
                    let x0 = self.x_of(left, start);
                    let x1 = self.x_of(left, start + duration).max(x0 + 1.0);
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
                    draw_clip_body(&clip_painter, theme, r, colour, audio);
                    if let Some(pictures) = &clip.pictures {
                        self.draw_pictures(&clip_painter, r, clip, start, pictures);
                    }
                    if let Some(w) = &clip.waveform {
                        self.draw_waveform(&clip_painter, r, clip, start, w, colour, track.muted);
                    }
                    draw_clip_marks(&clip_painter, theme, r, clip);
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

    /// M (mute), S (solo) and a volume fader in an audio track's header.
    fn track_controls(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        header: Rect,
        events: &mut Vec<TimelineEvent>,
    ) {
        if header.height() < 20.0 {
            return;
        }
        let id = ui.scope_id().with(("track", track.id));
        let button = |i: f32| {
            Rect::from_min_size(
                pos2(header.right() - 50.0 + i * 24.0, header.top() + 6.0),
                vec2(20.0, 16.0),
            )
        };
        for (i, (label, on, tip)) in [
            ("M", track.muted, "Mute"),
            ("S", track.solo, "Solo: hear only soloed tracks"),
        ]
        .into_iter()
        .enumerate()
        {
            let rect = button(i as f32);
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
        // Volume: -60 dB at the left, +12 dB at the right; double-click resets to 0.
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
        let to_x = |db: f32| bar.left() + (db + 60.0) / 72.0 * bar.width();
        let r = ui
            .interact(
                bar.expand2(vec2(4.0, 5.0)),
                id.with("volume"),
                Sense::click_and_drag(),
            )
            .on_hover_text(format!("Volume {db:+.1} dB (double-click: 0 dB)"));
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            let v = (((p.x - bar.left()) / bar.width()) * 72.0 - 60.0).clamp(-60.0, 12.0);
            let v = if v.abs() < 1.0 {
                0.0
            } else {
                (v * 2.0).round() / 2.0
            };
            self.fader_drag = Some((track.id, v));
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
            let t = pics.source_in + f64::from(x + tile / 2.0 - left) / f64::from(self.zoom);
            let i = ((t / pics.every).round().max(0.0) as usize).min(pics.textures.len() - 1);
            let rect = Rect::from_min_size(pos2(x, area.top()), vec2(tile, area.height()));
            painter.image(pics.textures[i], rect, uv, Color32::from_white_alpha(230));
            x += tile;
        }
    }

    /// The clip's loudness as one bar per pixel column, centred in the clip.
    #[allow(clippy::too_many_arguments)]
    fn draw_waveform(
        &self,
        p: &egui::Painter,
        r: Rect,
        clip: &TimelineClip,
        shown_start: f64,
        w: &ClipWaveform,
        colour: Color32,
        muted: bool,
    ) {
        // Below the clip's name, down to its bottom edge.
        let below_name = Rect::from_min_max(
            pos2(r.left() + 1.0, r.top() + 19.0),
            pos2(r.right() - 1.0, r.bottom() - 3.0),
        );
        let clip_area = p.clip_rect().intersect(below_name);
        if clip_area.width() < 1.0 || clip_area.height() < 4.0 || w.peaks.is_empty() {
            return;
        }
        let left = r.left() - ((shown_start - clip.start) * f64::from(self.zoom)) as f32;
        let colour = if muted {
            colour.gamma_multiply(0.5)
        } else {
            Color32::from_white_alpha(170)
        };
        let mut mesh = egui::Mesh::default();
        let (mid, half) = (clip_area.center().y, clip_area.height() / 2.0);
        let per_px = f64::from(w.per_second) / f64::from(self.zoom);
        let mut x = clip_area.left().floor();
        while x < clip_area.right() {
            let t = w.source_in + f64::from(x - left) / f64::from(self.zoom);
            let a = (t * f64::from(w.per_second)).max(0.0) as usize;
            let b = ((t * f64::from(w.per_second) + per_px).ceil() as usize)
                .max(a + 1)
                .min(w.peaks.len());
            if a >= w.peaks.len() {
                break;
            }
            let peak = w.peaks[a..b].iter().fold(0f32, |m, v| m.max(*v));
            let h = (peak.sqrt() * half).max(0.5);
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, mid - h), pos2(x + 1.0, mid + h)),
                colour,
            );
            x += 1.0;
        }
        p.add(Shape::mesh(mesh));
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

        // Region handles: drag to adjust.
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
                    let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
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
                    return;
                }
            }
        }

        // Labels: click one to go to it, double-click to rename or recolour it.
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
                return;
            }
            if single {
                events.push(TimelineEvent::Seek(marker.at));
                return;
            }
        }

        // Ruler: click or drag to move the playhead; Shift-drag draws the region.
        let ruler = ui.interact(
            layout.ruler.union(layout.labels),
            id.with("ruler"),
            Sense::click_and_drag(),
        );
        let shift = ui.input(|i| i.modifiers.shift);
        if ruler.drag_started()
            && shift
            && let Some(p) = ruler.interact_pointer_pos()
        {
            let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
            self.region_drag = Some((RegionDrag::New(t), (t, t)));
        }
        if let Some((RegionDrag::New(from), _)) = self.region_drag {
            if ruler.dragged()
                && let Some(p) = ruler.interact_pointer_pos()
            {
                let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
                self.region_drag = Some((RegionDrag::New(from), (from.min(t), from.max(t))));
            }
            if ruler.drag_stopped()
                && let Some((_, (a, b))) = self.region_drag.take()
                && b - a > 1e-6
            {
                events.push(TimelineEvent::SetRegion(Some((a, b))));
            }
        } else if (ruler.clicked() || ruler.dragged())
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

        // Clips: click selects (or cuts, with the blade); drag moves or trims.
        for (area, tracks, height, from_bottom, scroll) in [
            (layout.video, data.video, VIDEO_TRACK, true, self.video_scroll),
            (layout.audio, data.audio, AUDIO_TRACK, false, self.audio_scroll),
        ] {
            let time_area = Rect::from_min_max(pos2(left, area.top()), area.max);
            let r = ui.interact(time_area, id.with(("area", from_bottom)), Sense::click_and_drag());
            let Some(p) = r.interact_pointer_pos().or(r.hover_pos()) else {
                continue;
            };
            let row_rects = rows(&area, tracks.len(), height, scroll, from_bottom);
            let t = self.time_at(left, p.x);
            // A file dragged over the timeline: show where it would land.
            if r.dnd_hover_payload::<DraggedMedia>().is_some() {
                let x = self.x_of(left, t.max(0.0));
                ui.painter().line_segment(
                    [pos2(x, layout.video.top()), pos2(x, layout.audio.bottom())],
                    Stroke::new(2.0, ui.visuals().selection.stroke.color),
                );
            }
            if let Some(dragged) = r.dnd_release_payload::<DraggedMedia>() {
                let insert = ui.input(|i| i.modifiers.command);
                events.push(TimelineEvent::DropMedia {
                    media: dragged.0,
                    at: t.max(0.0),
                    insert,
                });
                continue;
            }
            let hit = tracks
                .iter()
                .zip(&row_rects)
                .filter(|(_, row)| row.y_range().contains(p.y))
                .flat_map(|(track, _)| track.clips.iter())
                .find(|c| c.start <= t && t < c.start + c.duration);
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

            if r.hovered() && self.clip_drag.is_none() {
                match (self.tool, zone) {
                    (Tool::Blade, Some(_)) => ui.ctx().set_cursor_icon(CursorIcon::Crosshair),
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

            if r.drag_started()
                && let (Some(c), Some(zone)) = (hit, zone)
                && let Some((c, kind)) = drag_for(self.tool, zone, c, tracks, &row_rects, p.y)
            {
                self.clip_drag = Some(ClipDrag {
                    clip: c.id,
                    link: c.link,
                    kind,
                    grabbed_at: t,
                    start: c.start,
                    end: c.start + c.duration,
                    delta: 0.0,
                });
                if !c.selected {
                    events.push(TimelineEvent::Select {
                        clip: c.id,
                        add: false,
                    });
                }
            }
            if r.dragged()
                && let Some(drag) = self.clip_drag
            {
                let raw = t - drag.grabbed_at;
                let delta = if self.snapping && !ui.input(|i| i.modifiers.alt) {
                    self.snapped(raw, &drag, data, left)
                } else {
                    raw
                };
                let delta = match drag.kind {
                    DragKind::Move | DragKind::Slide | DragKind::Trim(ClipEdge::Start) => {
                        delta.max(-drag.start)
                    }
                    _ => delta,
                };
                self.clip_drag = Some(ClipDrag { delta, ..drag });
                ui.ctx().set_cursor_icon(match drag.kind {
                    DragKind::Move => CursorIcon::Grabbing,
                    DragKind::Slip | DragKind::Slide => CursorIcon::ResizeColumn,
                    _ => CursorIcon::ResizeHorizontal,
                });
            }
            if r.drag_stopped()
                && let Some(drag) = self.clip_drag.take()
                && drag.delta.abs() > 1e-9
            {
                events.push(match drag.kind {
                    DragKind::Move => TimelineEvent::Move {
                        clip: drag.clip,
                        by: drag.delta,
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
            if r.secondary_clicked() {
                self.menu_at = Some((t, hit.map(|c| (c.id, c.start, c.start + c.duration))));
            }
            let menu_at = self.menu_at;
            let has_region = data.region.is_some();
            r.context_menu(|ui| {
                let Some((at, clip)) = menu_at else { return };
                if ui.button("Add a label here").clicked() {
                    events.push(TimelineEvent::AddLabel(at));
                    ui.close();
                }
                if let Some((id, start, end)) = clip {
                    if ui.button("Split this clip here").clicked() {
                        events.push(TimelineEvent::SplitAt { at, clip: Some(id) });
                        ui.close();
                    }
                    if ui.button("Set the region to this clip").clicked() {
                        events.push(TimelineEvent::SetRegion(Some((start, end))));
                        ui.close();
                    }
                }
                if ui.button("Split every track here").clicked() {
                    events.push(TimelineEvent::SplitAt { at, clip: None });
                    ui.close();
                }
                if has_region && ui.button("Clear the region").clicked() {
                    events.push(TimelineEvent::SetRegion(None));
                    ui.close();
                }
            });
            if r.clicked() {
                let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                events.push(match (self.tool, hit) {
                    (Tool::Blade, Some(c)) => TimelineEvent::Blade { clip: c.id, at: t },
                    (_, Some(c)) => TimelineEvent::Select { clip: c.id, add },
                    (_, None) => TimelineEvent::ClearSelection,
                });
            }
        }
    }

    /// The label drawn nearest to `x`, if one is within a few pixels.
    fn marker_near<'m>(
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

    /// `raw` (seconds) adjusted so the dragged edge lands on the nearest snap
    /// point within a few pixels: other clips' edges, the playhead, the edit
    /// needle, labels and the start.
    fn snapped(&self, raw: f64, drag: &ClipDrag, data: &TimelineData, _left: f32) -> f64 {
        let reach = 8.0 / f64::from(self.zoom);
        let mut points = vec![0.0, data.playhead];
        points.extend(data.needle);
        points.extend(data.markers.iter().map(|m| m.at));
        for track in data.video.iter().chain(data.audio) {
            for c in track.clips.iter().filter(|c| !drag.moves(c)) {
                points.push(c.start);
                points.push(c.start + c.duration);
            }
        }
        let edges: Vec<f64> = match drag.kind {
            DragKind::Move | DragKind::Slide => vec![drag.start + raw, drag.end + raw],
            DragKind::Trim(ClipEdge::Start) | DragKind::RippleTrim(ClipEdge::Start) => vec![drag.start + raw],
            DragKind::Trim(ClipEdge::End) | DragKind::RippleTrim(ClipEdge::End) | DragKind::Roll => {
                vec![drag.end + raw]
            }
            DragKind::Slip => vec![],
        };
        let best = edges
            .iter()
            .flat_map(|e| points.iter().map(move |p| p - e))
            .filter(|d| d.abs() <= reach)
            .min_by(|a, b| a.abs().total_cmp(&b.abs()));
        raw + best.unwrap_or(0.0)
    }
}

/// What the timeline's toolbar asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarClick {
    Tool(Tool),
    Snapping,
    Fit,
}

/// The row of tools above the timeline: Select, Blade, Ripple, Roll, Slip,
/// Slide, then Snapping and Fit. `tips` gives each button's tooltip (with
/// its shortcut), in that order.
pub fn toolbar(ui: &mut Ui, theme: &Theme, view: &TimelineView, tips: [&str; 8]) -> Option<ToolbarClick> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        let size = vec2(30.0, 26.0);
        let items = [
            (ToolbarClick::Tool(Tool::Select), view.tool == Tool::Select, 0),
            (ToolbarClick::Tool(Tool::Blade), view.tool == Tool::Blade, 1),
            (ToolbarClick::Tool(Tool::Ripple), view.tool == Tool::Ripple, 2),
            (ToolbarClick::Tool(Tool::Roll), view.tool == Tool::Roll, 3),
            (ToolbarClick::Tool(Tool::Slip), view.tool == Tool::Slip, 4),
            (ToolbarClick::Tool(Tool::Slide), view.tool == Tool::Slide, 5),
            (ToolbarClick::Snapping, view.snapping, 6),
            (ToolbarClick::Fit, false, 7),
        ];
        for (click, on, i) in items {
            if i == 6 {
                ui.add_space(10.0);
            }
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            let r = r.on_hover_text(tips[i]);
            let fill = if on {
                theme.accent.gamma_multiply(0.25)
            } else if r.hovered() {
                theme.line
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let colour = if on { theme.accent } else { theme.text_secondary };
            tool_icon(ui.painter(), i, rect.center(), colour);
            if r.clicked() {
                clicked = Some(click);
            }
        }
    });
    clicked
}

fn tool_icon(p: &egui::Painter, which: usize, c: Pos2, colour: Color32) {
    let stroke = Stroke::new(1.6, colour);
    match which {
        // Pointer arrow.
        0 => {
            p.add(Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, -7.0),
                    c + vec2(5.0, 1.0),
                    c + vec2(0.5, 1.5),
                    c + vec2(-1.5, 6.5),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        // Blade: a cut line through a clip.
        1 => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(16.0, 9.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [c + vec2(0.0, -8.0), c + vec2(0.0, 8.0)],
                Stroke::new(2.0, colour),
            );
        }
        // Ripple: an edge with the rest of the timeline following.
        2 => {
            p.line_segment(
                [c + vec2(-4.0, -7.0), c + vec2(-4.0, 7.0)],
                Stroke::new(2.0, colour),
            );
            p.rect_stroke(
                Rect::from_min_max(c + vec2(-1.0, -4.0), c + vec2(8.0, 4.0)),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment([c + vec2(-8.0, 0.0), c + vec2(-5.0, 0.0)], stroke);
        }
        // Roll: two clips meeting at a cut that moves.
        3 => {
            for s in [-1.0, 1.0] {
                p.rect_stroke(
                    Rect::from_min_max(
                        c + vec2(if s < 0.0 { -8.0 } else { 1.0 }, -4.0),
                        c + vec2(if s < 0.0 { -1.0 } else { 8.0 }, 4.0),
                    ),
                    1.5,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            p.line_segment(
                [c + vec2(0.0, -8.0), c + vec2(0.0, 8.0)],
                Stroke::new(2.0, colour),
            );
        }
        // Slip: arrows inside a clip.
        4 => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(18.0, 10.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(5.0 * s, 0.0), c + vec2(3.0 * s, -2.0)], stroke);
                p.line_segment([c + vec2(5.0 * s, 0.0), c + vec2(3.0 * s, 2.0)], stroke);
            }
        }
        // Slide: a clip with arrows outside it.
        5 => {
            p.rect_filled(Rect::from_center_size(c, vec2(8.0, 10.0)), 1.5, colour);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(6.0 * s, 0.0), c + vec2(10.0 * s, 0.0)], stroke);
                p.line_segment([c + vec2(10.0 * s, 0.0), c + vec2(8.0 * s, -2.0)], stroke);
                p.line_segment([c + vec2(10.0 * s, 0.0), c + vec2(8.0 * s, 2.0)], stroke);
            }
        }
        // Snapping: a magnet.
        6 => {
            p.add(Shape::line(
                vec![
                    c + vec2(-5.0, -6.0),
                    c + vec2(-5.0, 1.0),
                    c + vec2(-3.0, 5.0),
                    c + vec2(0.0, 6.0),
                    c + vec2(3.0, 5.0),
                    c + vec2(5.0, 1.0),
                    c + vec2(5.0, -6.0),
                ],
                Stroke::new(2.4, colour),
            ));
        }
        // Fit: arrows out to both sides.
        _ => {
            p.line_segment([c + vec2(-7.0, 0.0), c + vec2(7.0, 0.0)], stroke);
            for s in [-1.0, 1.0] {
                p.line_segment([c + vec2(7.0 * s, 0.0), c + vec2(4.0 * s, -3.0)], stroke);
                p.line_segment([c + vec2(7.0 * s, 0.0), c + vec2(4.0 * s, 3.0)], stroke);
            }
        }
    }
}

/// What dragging `c` (grabbed in `zone`) does with `tool`, and which clip
/// it acts on: the roll tool grabbed at a clip's start acts on the cut after
/// the clip before it. `None` when the tool does nothing there.
fn drag_for<'a>(
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

/// What goes over a clip's contents: missing and selected outlines, and its name.
fn draw_clip_marks(p: &egui::Painter, theme: &Theme, r: Rect, clip: &TimelineClip) {
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
        let inside = p.with_clip_rect(r.shrink(2.0));
        if clip.pictures.is_some() {
            // A dark pill keeps the name readable over the pictures.
            let pill = Rect::from_min_size(pos - vec2(3.0, 1.0), galley.size() + vec2(6.0, 2.0));
            inside.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
        }
        inside.galley(pos, galley, theme.text);
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

    fn clip(id: u64, start: f64, duration: f64) -> TimelineClip {
        TimelineClip {
            id,
            start,
            duration,
            name: String::new(),
            info: String::new(),
            selected: false,
            missing: false,
            link: None,
            waveform: None,
            pictures: None,
        }
    }

    #[test]
    fn the_nearest_label_within_reach_is_hit() {
        let v = TimelineView {
            zoom: 100.0,
            scroll: 0.0,
            ..Default::default()
        };
        let marker = |id: u64, at: f64| TimelineMarker {
            id,
            at,
            name: String::new(),
            colour: Color32::WHITE,
        };
        let markers = [marker(1, 1.0), marker(2, 1.1)];
        // At 100 px a second, the labels sit at x = 100 and 110.
        assert_eq!(v.marker_near(0.0, &markers, 103.0).map(|m| m.id), Some(1));
        assert_eq!(v.marker_near(0.0, &markers, 107.0).map(|m| m.id), Some(2));
        assert!(v.marker_near(0.0, &markers, 130.0).is_none());
    }

    #[test]
    fn each_tool_drags_the_right_thing() {
        let track = TimelineTrack {
            id: 1,
            name: "V1".into(),
            clips: vec![clip(1, 0.0, 5.0), clip(2, 5.0, 5.0)],
            muted: false,
            solo: false,
            gain_db: 0.0,
        };
        let tracks = [track];
        let rows = [Rect::from_min_max(pos2(0.0, 0.0), pos2(500.0, 50.0))];
        let second = &tracks[0].clips[1];
        let start = DragKind::Trim(ClipEdge::Start);
        let got = |tool, zone| drag_for(tool, zone, second, &tracks, &rows, 10.0).map(|(c, k)| (c.id, k));
        assert_eq!(got(Tool::Select, start), Some((2, start)));
        assert_eq!(
            got(Tool::Ripple, start),
            Some((2, DragKind::RippleTrim(ClipEdge::Start)))
        );
        assert_eq!(
            got(Tool::Roll, start),
            Some((1, DragKind::Roll)),
            "the cut before: the first clip's end"
        );
        assert_eq!(got(Tool::Roll, DragKind::Move), None);
        assert_eq!(got(Tool::Slip, DragKind::Move), Some((2, DragKind::Slip)));
        assert_eq!(got(Tool::Slide, start), Some((2, DragKind::Slide)));
        assert_eq!(got(Tool::Blade, DragKind::Move), None, "the blade cuts on click");
        // Nothing before the first clip to roll with.
        let first = &tracks[0].clips[0];
        assert!(drag_for(Tool::Roll, start, first, &tracks, &rows, 10.0).is_none());
    }

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
