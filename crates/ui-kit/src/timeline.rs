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
//! Moving around, the same over the ruler and both areas: Ctrl+wheel (or a
//! pinch) zooms time at the pointer, Shift+wheel or a sideways wheel pans
//! time, the plain wheel scrolls the area under the pointer (and pans time
//! over the ruler and the scroll bar), and dragging with the middle button
//! pans both ways at once.
//!
//! Every track has its own height ([`TrackHeights`]): drag the edge of its
//! name (the bottom edge of an audio track, the top edge of a video track,
//! which is where each area grows) or double-click it for the usual height;
//! Alt+wheel makes every track of the area under the pointer taller or
//! shorter. Heights are view state the page keeps; they are never edits.
//!
//! The widget knows nothing about projects: the page hands it plain data
//! (seconds as `f64`, for drawing only) and gets back [`TimelineEvent`]s.

mod rows;

pub use rows::{MAX_TRACK, MIN_TRACK, TrackHeights};

use crate::Theme;
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};
use rows::Rows;

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
    /// How fast the clip plays its file.
    pub speed: f64,
}

/// A clip's loudness: peaks of its whole file, and where in the file the clip starts.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipWaveform {
    pub peaks: std::sync::Arc<[f32]>,
    pub per_second: f32,
    /// Seconds into the file where the clip begins.
    pub source_in: f64,
    /// How fast the clip plays its file.
    pub speed: f64,
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
    /// "Speed" from a clip's right-click menu: play it `num / den` times as fast.
    SetSpeed { clip: u64, num: u32, den: u32 },
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

/// A track's height being changed by dragging the edge of its name.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HeightDrag {
    track: u64,
    /// The height and the pointer's y when the drag began.
    from_height: f32,
    from_y: f32,
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
    /// Each track's height. View state: the page keeps and saves it.
    pub heights: TrackHeights,
    /// A track's edge being dragged to change its height.
    height_drag: Option<HeightDrag>,
    /// The area the pointer is over, for commands that act on one area.
    hover_half: Option<Half>,
    /// The edit needle while it is being dragged.
    needle_drag: Option<f64>,
    scrollbar_drag: Option<(f32, f64)>,
    clip_drag: Option<ClipDrag>,
    /// A track volume being dragged: track and its value so far.
    fader_drag: Option<(u64, f32)>,
    region_drag: Option<(RegionDrag, (f64, f64))>,
    /// Where the right-click menu was opened: time, and the clip under it.
    menu_at: Option<MenuSpot>,
    /// A drag with the middle button that pans the view: the area it began
    /// over (`None`: the ruler or the scroll bar, which only pan time).
    pan_drag: Option<Option<Half>>,
    /// Steps commands asked for, taken when the timeline is next drawn.
    steps: Vec<ViewStep>,
    /// The closest zoom allowed: there a frame is wide enough to click.
    max_zoom: f32,
    pub tool: Tool,
    /// Moved and trimmed edges snap to clip edges, the playhead, the needle and labels.
    pub snapping: bool,
    /// The rough cut view: taller video rows, each clip's sound drawn inside it.
    pub rough_cut: bool,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView {
            zoom: 60.0,
            scroll: 0.0,
            video_scroll: 0.0,
            audio_scroll: 0.0,
            split: 0.5,
            heights: TrackHeights::default(),
            height_drag: None,
            hover_half: None,
            needle_drag: None,
            scrollbar_drag: None,
            clip_drag: None,
            fader_drag: None,
            region_drag: None,
            menu_at: None,
            pan_drag: None,
            steps: Vec::new(),
            max_zoom: MAX_ZOOM,
            tool: Tool::Select,
            snapping: true,
            rough_cut: false,
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
/// Height of a video row in the rough cut view, with its sound folded in.
pub const ROUGH_CUT_TRACK: f32 = 96.0;
const MIN_ZOOM: f32 = 0.02;
/// The closest zoom (pixels per second) at 60 frames a second and below.
const MAX_ZOOM: f32 = 2400.0;
/// At the closest zoom a frame is at least this wide, whatever the frame rate.
const FRAME_WIDTH_ZOOMED_IN: f32 = 40.0;
/// How much one Zoom in or Zoom out command changes the zoom.
const ZOOM_STEP: f32 = 1.5;
/// How much one Taller tracks or Shorter tracks command changes heights.
const HEIGHT_STEP: f32 = 1.25;
/// Clips shorter than this draw their waveform behind the name instead of under it.
const SHORT_CLIP: f32 = 40.0;

/// The closest zoom for a timeline at `fps`: single frames stay easy to click.
pub fn max_zoom(fps: f64) -> f32 {
    (fps as f32 * FRAME_WIDTH_ZOOMED_IN).max(MAX_ZOOM)
}

/// One of the timeline's two areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    Video,
    Audio,
}

/// A change to the view that a command asks for (a key or the search
/// palette); the timeline takes it the next time it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewStep {
    /// Closer in time, around the playhead when it is in view.
    ZoomIn,
    /// Further out in time.
    ZoomOut,
    /// Every track of the area under the pointer taller (both areas when
    /// the pointer is elsewhere).
    TallerTracks,
    ShorterTracks,
    /// Those tracks back to their usual height.
    UsualTrackHeights,
}

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

    /// Everything the timeline covers.
    fn whole(&self) -> Rect {
        Rect::from_min_max(pos2(self.header_x, self.ruler.top()), self.scrollbar.max)
    }

    /// The area (with its track names) that `pos` is in.
    fn half_at(&self, pos: Pos2) -> Option<Half> {
        if self.video.contains(pos) {
            Some(Half::Video)
        } else if self.audio.contains(pos) {
            Some(Half::Audio)
        } else {
            None
        }
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
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, self.max_zoom);
        self.scroll = (t - f64::from(anchor - left) / f64::from(self.zoom)).max(0.0);
    }

    /// Asks for a step (from a command); it happens when the timeline is next drawn.
    pub fn step(&mut self, step: ViewStep) {
        self.steps.push(step);
    }

    /// Takes the steps commands asked for.
    fn take_steps(&mut self, layout: &Layout, data: &TimelineData) {
        let left = layout.time_left();
        for step in std::mem::take(&mut self.steps) {
            // Around the playhead when it is in view, else the middle.
            let playhead = self.x_of(left, data.playhead);
            let anchor = if (left..=layout.ruler.right()).contains(&playhead) {
                playhead
            } else {
                layout.ruler.center().x
            };
            let halves = match self.hover_half {
                Some(half) => vec![half],
                None => vec![Half::Video, Half::Audio],
            };
            match step {
                ViewStep::ZoomIn => self.zoom_around(left, anchor, ZOOM_STEP),
                ViewStep::ZoomOut => self.zoom_around(left, anchor, 1.0 / ZOOM_STEP),
                ViewStep::TallerTracks => {
                    for half in halves {
                        self.scale_tracks(half, HEIGHT_STEP, None, layout, data);
                    }
                }
                ViewStep::ShorterTracks => {
                    for half in halves {
                        self.scale_tracks(half, 1.0 / HEIGHT_STEP, None, layout, data);
                    }
                }
                ViewStep::UsualTrackHeights => {
                    for half in halves {
                        for track in tracks_of(half, data) {
                            self.heights.reset(track.id);
                        }
                    }
                }
            }
        }
    }

    /// The usual height of a track in `half` in the current view.
    fn usual_height(&self, half: Half) -> f32 {
        match half {
            Half::Video if self.rough_cut => ROUGH_CUT_TRACK,
            Half::Video => VIDEO_TRACK,
            Half::Audio => AUDIO_TRACK,
        }
    }

    /// The rows of `half`, each track with its own height.
    fn rows(&self, half: Half, layout: &Layout, data: &TimelineData) -> Rows {
        let usual = self.usual_height(half);
        let heights = tracks_of(half, data)
            .iter()
            .map(|t| self.heights.height(t.id, usual));
        match half {
            Half::Video => Rows::new(layout.video, heights, self.video_scroll, true),
            Half::Audio => Rows::new(layout.audio, heights, self.audio_scroll, false),
        }
    }

    /// Makes every track of `half` `factor` times as tall, within the
    /// limits. With `keep` (a screen y: the pointer), the spot of the track
    /// under it stays under it.
    fn scale_tracks(
        &mut self,
        half: Half,
        factor: f32,
        keep: Option<f32>,
        layout: &Layout,
        data: &TimelineData,
    ) {
        let before = self.rows(half, layout, data);
        let usual = self.usual_height(half);
        self.heights
            .scale(tracks_of(half, data).iter().map(|t| t.id), usual, factor);
        let Some(y) = keep else { return };
        let Some(i) = before.at(y) else { return };
        let was = before.rects()[i];
        let share = (y - was.top()) / was.height().max(1.0);
        let now = self.rows(half, layout, data).rects()[i];
        // How far down the spot went; scroll it back.
        let moved = now.top() + share * now.height() - y;
        match half {
            Half::Video => self.video_scroll -= moved,
            Half::Audio => self.audio_scroll += moved,
        }
    }

    /// Never scrolled past the first track or further than the last one needs.
    fn keep_scrolls_in_range(&mut self, layout: &Layout, data: &TimelineData) {
        let most = self
            .rows(Half::Video, layout, data)
            .max_scroll(layout.video.height());
        self.video_scroll = self.video_scroll.clamp(0.0, most);
        let most = self
            .rows(Half::Audio, layout, data)
            .max_scroll(layout.audio.height());
        self.audio_scroll = self.audio_scroll.clamp(0.0, most);
    }

    /// Fits `length` seconds in `width` pixels.
    pub fn fit(&mut self, length: f64, width: f32) {
        if length > 0.0 && width > 0.0 {
            self.zoom = ((width * 0.95) / length as f32).clamp(MIN_ZOOM, self.max_zoom);
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

        self.max_zoom = max_zoom(data.fps);
        self.zoom = self.zoom.clamp(MIN_ZOOM, self.max_zoom);
        self.hover_half = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|_| ui.rect_contains_pointer(layout.whole()))
            .and_then(|pos| layout.half_at(pos));
        self.take_steps(&layout, data);
        self.wheel(ui, &layout, data);
        let panning = self.middle_pan(ui, &layout);
        self.keep_scrolls_in_range(&layout, data);
        // Body: video tracks bottom-up above the divider, audio top-down below it.
        let video_rows = self.rows(Half::Video, &layout, data);
        let audio_rows = self.rows(Half::Audio, &layout, data);
        // While the middle button pans, nothing else reacts to the pointer.
        if !panning {
            self.input(ui, &layout, data, [&video_rows, &audio_rows], &mut events);
        }
        let visible = (self.time_at(left, left), self.time_at(left, rect.right()));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.panel);

        for (area, tracks, rows, half) in [
            (layout.video, data.video, &video_rows, Half::Video),
            (layout.audio, data.audio, &audio_rows, Half::Audio),
        ] {
            let audio = half == Half::Audio;
            let clip_painter = ui.painter_at(Rect::from_min_max(pos2(left, area.top()), area.max));
            let header_painter = ui.painter_at(Rect::from_min_max(area.min, pos2(left, area.bottom())));
            for (i, (track, row)) in tracks.iter().zip(rows.rects()).enumerate() {
                // The edge that resizes the track is reachable even when the
                // row itself has just left the area.
                self.track_edge(ui, theme, half, track, *row, area);
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
                    // With both (the rough cut view), pictures fill the top
                    // and the sound the bottom of the clip.
                    let (pic_r, wave_r) = match (&clip.pictures, &clip.waveform) {
                        (Some(_), Some(_)) => {
                            let split = r.top() + r.height() * 0.62;
                            (
                                Rect::from_min_max(r.min, pos2(r.right(), split)),
                                Rect::from_min_max(pos2(r.left(), split - 17.0), r.max),
                            )
                        }
                        _ => (r, r),
                    };
                    if let Some(pictures) = &clip.pictures {
                        self.draw_pictures(&clip_painter, pic_r, clip, start, pictures);
                    }
                    if let Some(w) = &clip.waveform {
                        let wave_colour = if audio { colour } else { AUDIO_COLOURS[0] };
                        self.draw_waveform(&clip_painter, wave_r, clip, start, w, wave_colour, track.muted);
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
            let t = pics.source_in + f64::from(x + tile / 2.0 - left) / f64::from(self.zoom) * pics.speed;
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
        // Below the clip's name, down to its bottom edge; in a short clip
        // there is no room under the name, so the whole clip is used.
        let top = if r.height() < SHORT_CLIP { 3.0 } else { 19.0 };
        let below_name = Rect::from_min_max(
            pos2(r.left() + 1.0, r.top() + top),
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
        let per_px = f64::from(w.per_second) / f64::from(self.zoom) * w.speed;
        let mut x = clip_area.left().floor();
        while x < clip_area.right() {
            let t = w.source_in + f64::from(x - left) / f64::from(self.zoom) * w.speed;
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

    /// The wheel, wherever the pointer is on the timeline: Ctrl (or a pinch)
    /// zooms time at the pointer, sideways movement (Shift, a tilting wheel,
    /// two fingers on a touchpad) pans time, and up and down scrolls the area
    /// under the pointer, or pans time over the ruler and the scroll bar. With
    /// Alt it makes the tracks of the area under the pointer taller or shorter.
    fn wheel(&mut self, ui: &Ui, layout: &Layout, data: &TimelineData) {
        let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        // Not when a window or a menu lies over the timeline there.
        if !layout.whole().contains(pos) || !ui.rect_contains_pointer(layout.whole()) {
            return;
        }
        // egui has already sorted the wheel by modifier: with Ctrl it arrives
        // as a zoom factor, with Shift as sideways scrolling.
        let (zoom, scroll, alt) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta(), i.modifiers.alt));
        let left = layout.time_left();
        if zoom != 1.0 {
            self.zoom_around(left, pos.x.max(left), zoom);
        }
        if scroll.x != 0.0 {
            self.pan_time(scroll.x);
        }
        if scroll.y != 0.0 {
            match layout.half_at(pos) {
                // Alt: every track of this area taller or shorter.
                Some(half) if alt => {
                    self.scale_tracks(half, (scroll.y / 200.0).exp(), Some(pos.y), layout, data);
                }
                Some(half) => self.scroll_half(half, scroll.y),
                None if !alt && !layout.divider.contains(pos) => self.pan_time(scroll.y),
                None => {}
            }
        }
    }

    /// Moves the view sideways so what is shown shifts right by `pixels`.
    fn pan_time(&mut self, pixels: f32) {
        self.scroll = (self.scroll - f64::from(pixels) / f64::from(self.zoom)).max(0.0);
    }

    /// Scrolls one area so its tracks shift down by `pixels`
    /// ([`Self::keep_scrolls_in_range`] stops it at the ends).
    fn scroll_half(&mut self, half: Half, pixels: f32) {
        match half {
            // Video tracks stack upward: shifting them down shows higher ones.
            Half::Video => self.video_scroll += pixels,
            Half::Audio => self.audio_scroll -= pixels,
        }
    }

    /// The edge of a track's name that resizes the track: the bottom edge of
    /// an audio track, the top edge of a video track (the side each area
    /// grows towards). Drag it, or double-click for the usual height.
    fn track_edge(
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
            pos2(area.left() + HEADER_WIDTH, edge + 3.0),
        )
        .intersect(area);
        // A drag goes on while the button is held, also when the edge has
        // been dragged out of the area and can't be pointed at any more.
        if !ui.input(|i| i.pointer.primary_down()) {
            self.height_drag = None;
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

    /// Dragging with the middle button pans: time sideways, and the area the
    /// drag began over up and down. True while such a drag lasts.
    fn middle_pan(&mut self, ui: &Ui, layout: &Layout) -> bool {
        let middle = egui::PointerButton::Middle;
        let (pressed, down, delta, pos) = ui.input(|i| {
            (
                i.pointer.button_pressed(middle),
                i.pointer.button_down(middle),
                i.pointer.delta(),
                i.pointer.interact_pos(),
            )
        });
        if pressed
            && let Some(pos) = pos
            && layout.whole().contains(pos)
            && ui.rect_contains_pointer(layout.whole())
        {
            self.pan_drag = Some(layout.half_at(pos));
        }
        if !down {
            self.pan_drag = None;
        }
        let Some(half) = self.pan_drag else {
            return false;
        };
        self.pan_time(delta.x);
        if let Some(half) = half {
            self.scroll_half(half, delta.y);
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        true
    }

    fn input(
        &mut self,
        ui: &mut Ui,
        layout: &Layout,
        data: &TimelineData,
        rows: [&Rows; 2],
        events: &mut Vec<TimelineEvent>,
    ) {
        let left = layout.time_left();
        let id = ui.scope_id().with("timeline");

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
        if ruler.drag_started_by(egui::PointerButton::Primary)
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
        } else if (ruler.clicked() || ruler.dragged_by(egui::PointerButton::Primary))
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
        for (area, tracks, rows, from_bottom) in [
            (layout.video, data.video, rows[0], true),
            (layout.audio, data.audio, rows[1], false),
        ] {
            let time_area = Rect::from_min_max(pos2(left, area.top()), area.max);
            let r = ui.interact(time_area, id.with(("area", from_bottom)), Sense::click_and_drag());
            let Some(p) = r.interact_pointer_pos().or(r.hover_pos()) else {
                continue;
            };
            // A drag is only noticed a few pixels after the press: what is
            // dragged, and from where, is what was pressed.
            let p = if r.drag_started() {
                ui.input(|i| i.pointer.press_origin()).unwrap_or(p)
            } else {
                p
            };
            let row_rects = rows.rects();
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
            let hit = rows.at(p.y).and_then(|row| tracks.get(row)).and_then(|track| {
                track
                    .clips
                    .iter()
                    .find(|c| c.start <= t && t < c.start + c.duration)
            });
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

            if r.drag_started_by(egui::PointerButton::Primary)
                && let (Some(c), Some(zone)) = (hit, zone)
                && let Some((c, kind)) = drag_for(self.tool, zone, c, tracks, row_rects, p.y)
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
                    ui.menu_button("Speed", |ui| {
                        for (text, num, den) in [
                            ("25% (slow motion)", 1, 4),
                            ("50% (slow motion)", 1, 2),
                            ("100% (normal)", 1, 1),
                            ("200%", 2, 1),
                            ("400%", 4, 1),
                        ] {
                            if ui.button(text).clicked() {
                                events.push(TimelineEvent::SetSpeed { clip: id, num, den });
                                ui.close();
                            }
                        }
                    });
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
    RoughCut,
}

/// The row of tools above the timeline: Select, Blade, Ripple, Roll, Slip,
/// Slide, then Snapping, Fit and the rough cut view. `tips` gives each
/// button's tooltip (with its shortcut), in that order.
pub fn toolbar(ui: &mut Ui, theme: &Theme, view: &TimelineView, tips: [&str; 9]) -> Option<ToolbarClick> {
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
            (ToolbarClick::RoughCut, view.rough_cut, 8),
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
        // Rough cut: a picture with its sound underneath, as one strip.
        8 => {
            p.rect_filled(
                Rect::from_min_max(c + vec2(-8.0, -6.0), c + vec2(8.0, 0.0)),
                1.5,
                colour,
            );
            for (i, h) in [2.0f32, 4.0, 3.0, 5.0, 2.0, 4.0, 3.0].iter().enumerate() {
                let x = c.x - 7.0 + i as f32 * 2.3;
                p.line_segment(
                    [pos2(x, c.y + 4.0 - h / 2.0), pos2(x, c.y + 4.0 + h / 2.0)],
                    stroke,
                );
            }
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

/// The tracks of one area.
fn tracks_of<'a>(half: Half, data: &TimelineData<'a>) -> &'a [TimelineTrack] {
    match half {
        Half::Video => data.video,
        Half::Audio => data.audio,
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

/// The step between the small ruler marks under a label step of `step`
/// seconds: a fifth of it, but never finer than a frame, so zoomed all the
/// way in there is one mark per frame.
pub fn minor_step(step: f64, fps: f64) -> f64 {
    (step / 5.0).max(1.0 / fps.max(1.0))
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

    fn track(id: u64, clips: Vec<TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: format!("T{id}"),
            clips,
            muted: false,
            solo: false,
            gain_db: 0.0,
        }
    }

    /// A 1000×400 timeline in the corner of a window, for sending it input.
    /// The names take x 0 to 140; the ruler y 0 to 26, the labels to 44, the
    /// video area to 212, the divider to 218, the audio area to 386 and the
    /// scroll bar the rest.
    struct Rig {
        ctx: egui::Context,
        view: TimelineView,
        video: Vec<TimelineTrack>,
        audio: Vec<TimelineTrack>,
        fps: f64,
        modifiers: egui::Modifiers,
    }

    const OVER_RULER: Pos2 = pos2(640.0, 10.0);
    const OVER_VIDEO: Pos2 = pos2(640.0, 150.0);
    const OVER_AUDIO: Pos2 = pos2(640.0, 300.0);
    const OVER_SCROLLBAR: Pos2 = pos2(640.0, 393.0);

    impl Rig {
        /// `video` and `audio` tracks, each with one clip from 1 s to 5 s.
        fn new(video: u64, audio: u64) -> Rig {
            Rig {
                ctx: egui::Context::default(),
                view: TimelineView::default(),
                video: (0..video).map(|i| track(i, vec![clip(i, 1.0, 4.0)])).collect(),
                audio: (0..audio)
                    .map(|i| track(100 + i, vec![clip(100 + i, 1.0, 4.0)]))
                    .collect(),
                fps: 60.0,
                modifiers: egui::Modifiers::NONE,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<TimelineEvent> {
            let size = vec2(1000.0, 400.0);
            // The keys held come as an event of their own, before the rest.
            let events = std::iter::once(egui::Event::ModifiersChanged(self.modifiers))
                .chain(events)
                .collect();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let output = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        let data = TimelineData {
                            video: &self.video,
                            audio: &self.audio,
                            length: 60.0,
                            fps: self.fps,
                            playhead: 2.0,
                            needle: None,
                            region: None,
                            markers: &[],
                            highlights: &[],
                            follow: false,
                        };
                        out = self.view.show(ui, &Theme::ASH, &data, size);
                    });
            });
            // Nothing draws these frames.
            output.drop_without_applying_deltas();
            out
        }

        /// Turns the wheel by `delta` points with the pointer at `at`.
        fn wheel(&mut self, at: Pos2, delta: Vec2, modifiers: egui::Modifiers) {
            self.modifiers = modifiers;
            self.frame(vec![egui::Event::PointerMoved(at)]);
            self.frame(vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers,
            }]);
            self.modifiers = egui::Modifiers::NONE;
            self.frame(Vec::new());
        }

        fn button(&mut self, at: Pos2, button: egui::PointerButton, pressed: bool) -> Vec<TimelineEvent> {
            self.frame(vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: self.modifiers,
            }])
        }

        fn move_to(&mut self, at: Pos2) -> Vec<TimelineEvent> {
            self.frame(vec![egui::Event::PointerMoved(at)])
        }
    }

    #[test]
    fn ctrl_and_the_wheel_zoom_at_the_pointer_over_the_ruler_and_both_areas() {
        for at in [OVER_RULER, OVER_VIDEO, OVER_AUDIO] {
            let mut rig = Rig::new(2, 2);
            rig.view.scroll = 3.0;
            let under_pointer = rig.view.time_at(HEADER_WIDTH, at.x);
            rig.wheel(at, vec2(0.0, 6.0), egui::Modifiers::COMMAND);
            assert!(rig.view.zoom > 60.5, "zoomed in at {at:?}: {}", rig.view.zoom);
            assert!(
                (rig.view.time_at(HEADER_WIDTH, at.x) - under_pointer).abs() < 1e-4,
                "the time under the pointer stays put at {at:?}"
            );
            let zoomed = rig.view.zoom;
            rig.wheel(at, vec2(0.0, -6.0), egui::Modifiers::COMMAND);
            assert!(rig.view.zoom < zoomed, "and back out");
            assert!((rig.view.time_at(HEADER_WIDTH, at.x) - under_pointer).abs() < 1e-4);
        }
    }

    #[test]
    fn shift_and_the_wheel_or_a_sideways_wheel_pan_time_everywhere() {
        for at in [OVER_RULER, OVER_VIDEO, OVER_AUDIO, OVER_SCROLLBAR] {
            let mut rig = Rig::new(2, 2);
            rig.view.scroll = 3.0;
            // Wheel down with Shift: later times come into view.
            rig.wheel(at, vec2(0.0, -6.0), egui::Modifiers::SHIFT);
            assert!(
                (rig.view.scroll - 3.1).abs() < 1e-6,
                "{at:?}: {}",
                rig.view.scroll
            );
            // A sideways wheel, or two fingers on a touchpad.
            rig.wheel(at, vec2(6.0, 0.0), egui::Modifiers::NONE);
            assert!(
                (rig.view.scroll - 3.0).abs() < 1e-6,
                "{at:?}: {}",
                rig.view.scroll
            );
            assert_eq!(rig.view.zoom, 60.0, "panning never zooms");
        }
    }

    #[test]
    fn the_plain_wheel_scrolls_the_area_under_it_and_pans_over_the_ruler() {
        // Eight tracks of each kind: more than either area shows.
        let mut rig = Rig::new(8, 8);
        rig.wheel(OVER_AUDIO, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert_eq!((rig.view.video_scroll, rig.view.audio_scroll), (0.0, 6.0));
        rig.wheel(OVER_VIDEO, vec2(0.0, 6.0), egui::Modifiers::NONE);
        assert_eq!(
            (rig.view.video_scroll, rig.view.audio_scroll),
            (6.0, 6.0),
            "each area on its own; video tracks stack upward, so wheel up shows higher ones"
        );
        assert_eq!(rig.view.scroll, 0.0);
        rig.wheel(OVER_RULER, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert!(
            (rig.view.scroll - 0.1).abs() < 1e-6,
            "over the ruler the wheel pans"
        );
        rig.wheel(OVER_SCROLLBAR, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert!((rig.view.scroll - 0.2).abs() < 1e-6, "and over the scroll bar");
        // With everything in view there is nothing to scroll.
        let mut rig = Rig::new(1, 1);
        rig.wheel(OVER_AUDIO, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert_eq!(rig.view.audio_scroll, 0.0);
    }

    #[test]
    fn the_middle_button_drags_the_view_both_ways() {
        let mut rig = Rig::new(8, 8);
        rig.view.scroll = 3.0;
        let middle = egui::PointerButton::Middle;
        rig.move_to(OVER_AUDIO);
        rig.button(OVER_AUDIO, middle, true);
        // Left by 60 points (one second) and up by 20.
        let events = rig.move_to(OVER_AUDIO + vec2(-60.0, -20.0));
        assert!(events.is_empty(), "panning selects and moves nothing");
        assert!((rig.view.scroll - 4.0).abs() < 1e-6, "{}", rig.view.scroll);
        assert_eq!(rig.view.audio_scroll, 20.0);
        assert_eq!(rig.view.video_scroll, 0.0, "only the area the drag began over");
        let events = rig.button(OVER_AUDIO + vec2(-60.0, -20.0), middle, false);
        assert!(events.is_empty());
        // Begun over the ruler: time only.
        rig.move_to(OVER_RULER);
        rig.button(OVER_RULER, middle, true);
        let events = rig.move_to(OVER_RULER + vec2(60.0, 30.0));
        rig.button(OVER_RULER + vec2(60.0, 30.0), middle, false);
        assert!(events.is_empty(), "the playhead stays where it was");
        assert!((rig.view.scroll - 3.0).abs() < 1e-6);
        assert_eq!(rig.view.audio_scroll, 20.0);
    }

    #[test]
    fn the_left_button_still_moves_clips_and_the_playhead() {
        let mut rig = Rig::new(1, 1);
        let primary = egui::PointerButton::Primary;
        // The clip runs from x = 200 to 440; grab its middle and drag right by a second.
        let grab = pos2(300.0, 190.0);
        rig.move_to(grab);
        rig.button(grab, primary, true);
        rig.move_to(grab + vec2(30.0, 0.0));
        rig.move_to(grab + vec2(60.0, 0.0));
        let events = rig.button(grab + vec2(60.0, 0.0), primary, false);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::Move { clip: 0, by } if (by - 1.0).abs() < 1e-6)),
            "{events:?}"
        );
        rig.move_to(OVER_RULER);
        let events = rig.button(OVER_RULER, primary, true);
        let events: Vec<_> = events
            .into_iter()
            .chain(rig.button(OVER_RULER, primary, false))
            .collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::Seek(t) if (t - 500.0 / 60.0).abs() < 1e-6)),
            "{events:?}"
        );
    }

    #[test]
    fn zoom_commands_step_around_the_playhead_and_stop_at_single_frames() {
        let mut rig = Rig::new(1, 1);
        // The playhead (2 s) is in view at x = 260.
        rig.view.step(ViewStep::ZoomIn);
        rig.frame(Vec::new());
        assert_eq!(rig.view.zoom, 90.0);
        assert!(
            (rig.view.x_of(HEADER_WIDTH, 2.0) - 260.0).abs() < 1e-3,
            "the playhead stays put"
        );
        rig.view.step(ViewStep::ZoomOut);
        rig.view.step(ViewStep::ZoomOut);
        rig.frame(Vec::new());
        assert!((rig.view.zoom - 40.0).abs() < 1e-3);
        // Out of view: around the middle of what is shown.
        rig.view.scroll = 30.0;
        let middle = rig.view.time_at(HEADER_WIDTH, 570.0);
        rig.view.step(ViewStep::ZoomIn);
        rig.frame(Vec::new());
        assert!((rig.view.time_at(HEADER_WIDTH, 570.0) - middle).abs() < 1e-4);
        // All the way in, a frame is 40 points wide, also at high frame rates.
        for _ in 0..40 {
            rig.view.step(ViewStep::ZoomIn);
        }
        rig.frame(Vec::new());
        assert_eq!(rig.view.zoom, 2400.0);
        rig.fps = 240.0;
        for _ in 0..40 {
            rig.view.step(ViewStep::ZoomIn);
        }
        rig.frame(Vec::new());
        rig.frame(Vec::new());
        assert_eq!(rig.view.zoom, 9600.0);
        assert_eq!(max_zoom(24.0), 2400.0);
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
        // Zoomed all the way in at 60 frames a second: a label every five
        // frames and a small mark on every frame, never between frames.
        let step = ruler_step(2400.0, 60.0, 90.0);
        assert!((step - 5.0 / 60.0).abs() < 1e-9);
        assert!((minor_step(step, 60.0) - 1.0 / 60.0).abs() < 1e-9);
        let step = ruler_step(2400.0, 30.0, 90.0);
        assert!((step - 2.0 / 30.0).abs() < 1e-9, "two frames at 30 a second");
        assert!(
            (minor_step(step, 30.0) - 1.0 / 30.0).abs() < 1e-9,
            "not 0.4 frames"
        );
        assert_eq!(minor_step(10.0, 60.0), 2.0);
        assert_eq!(timecode(0.0, 60.0), "00:00:00:00");
        assert_eq!(timecode(3661.5, 60.0), "01:01:01:30");
        assert_eq!(
            timecode(10.0, 60_000.0 / 1001.0),
            "00:00:09:59",
            "59.94: frame counts run slightly behind the clock"
        );
    }

    fn click(rig: &mut Rig, at: Pos2, button: egui::PointerButton) -> Vec<TimelineEvent> {
        rig.move_to(at);
        rig.button(at, button, true);
        rig.button(at, button, false)
    }

    /// Presses at `from`, moves to `to` in two steps and lets go there.
    /// Returns what letting go did, with times rounded to a millionth.
    fn drag(rig: &mut Rig, from: Pos2, to: Pos2) -> Vec<TimelineEvent> {
        let primary = egui::PointerButton::Primary;
        rig.move_to(from);
        rig.button(from, primary, true);
        rig.move_to(from + (to - from) / 2.0);
        rig.move_to(to);
        let round = |t: f64| (t * 1e6).round() / 1e6;
        rig.button(to, primary, false)
            .into_iter()
            .map(|event| match event {
                TimelineEvent::Move { clip, by } => TimelineEvent::Move { clip, by: round(by) },
                TimelineEvent::Trim { clip, edge, to } => TimelineEvent::Trim {
                    clip,
                    edge,
                    to: round(to),
                },
                TimelineEvent::RippleTrim { clip, edge, to } => TimelineEvent::RippleTrim {
                    clip,
                    edge,
                    to: round(to),
                },
                TimelineEvent::Roll { clip, to } => TimelineEvent::Roll { clip, to: round(to) },
                TimelineEvent::Slip { clip, by } => TimelineEvent::Slip { clip, by: round(by) },
                TimelineEvent::Slide { clip, by } => TimelineEvent::Slide { clip, by: round(by) },
                other => other,
            })
            .collect()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn every_tool_hits_the_right_clip_in_rows_of_mixed_heights() {
        let primary = egui::PointerButton::Primary;
        // Two audio tracks: the first made 120 tall (y 218 to 338), the
        // second left at 58 (from 338). Each has a clip from x = 200 to 440.
        let mut rig = Rig::new(2, 2);
        rig.view.heights.set(100, AUDIO_TRACK, 120.0);
        let select = |clip| vec![TimelineEvent::Select { clip, add: false }];
        assert_eq!(
            click(&mut rig, pos2(300.0, 320.0), primary),
            select(100),
            "still the first track: it is tall"
        );
        assert_eq!(click(&mut rig, pos2(300.0, 350.0), primary), select(101));
        // The first video track made 100 tall (y 112 to 212); the second sits on it.
        rig.view.heights.set(0, VIDEO_TRACK, 100.0);
        assert_eq!(click(&mut rig, pos2(300.0, 120.0), primary), select(0));
        assert_eq!(click(&mut rig, pos2(300.0, 100.0), primary), select(1));
        assert_eq!(
            click(&mut rig, pos2(300.0, 50.0), primary),
            [TimelineEvent::ClearSelection],
            "above the top track"
        );

        rig.view.tool = Tool::Blade;
        assert_eq!(
            click(&mut rig, pos2(320.0, 330.0), primary),
            [TimelineEvent::Blade { clip: 100, at: 3.0 }]
        );
        // Select: the body moves the clip, an edge trims it.
        rig.view.tool = Tool::Select;
        rig.view.snapping = false;
        let events = drag(&mut rig, pos2(300.0, 330.0), pos2(330.0, 330.0));
        assert_eq!(events, [TimelineEvent::Move { clip: 100, by: 0.5 }]);
        let events = drag(&mut rig, pos2(202.0, 330.0), pos2(262.0, 330.0));
        assert_eq!(
            events,
            [TimelineEvent::Trim {
                clip: 100,
                edge: ClipEdge::Start,
                to: 2.0
            }],
            "an edge pressed and dragged inward is still a trim"
        );
        rig.view.tool = Tool::Ripple;
        let events = drag(&mut rig, pos2(438.0, 330.0), pos2(378.0, 330.0));
        assert_eq!(
            events,
            [TimelineEvent::RippleTrim {
                clip: 100,
                edge: ClipEdge::End,
                to: 4.0
            }]
        );
        rig.view.tool = Tool::Roll;
        let events = drag(&mut rig, pos2(438.0, 330.0), pos2(468.0, 330.0));
        assert_eq!(events, [TimelineEvent::Roll { clip: 100, to: 5.5 }]);
        rig.view.tool = Tool::Slip;
        let events = drag(&mut rig, pos2(300.0, 350.0), pos2(330.0, 350.0));
        assert_eq!(events, [TimelineEvent::Slip { clip: 101, by: -0.5 }]);
        rig.view.tool = Tool::Slide;
        let events = drag(&mut rig, pos2(300.0, 120.0), pos2(270.0, 120.0));
        assert_eq!(events, [TimelineEvent::Slide { clip: 0, by: -0.5 }]);

        // The right-click menu opens for the clip under the pointer.
        click(&mut rig, pos2(320.0, 330.0), egui::PointerButton::Secondary);
        assert_eq!(rig.view.menu_at, Some((3.0, Some((100, 1.0, 5.0)))));
    }

    #[test]
    fn alt_and_the_wheel_resize_every_track_of_the_area_under_the_pointer() {
        let mut rig = Rig::new(2, 8);
        let alt = egui::Modifiers::ALT;
        rig.wheel(OVER_AUDIO, vec2(0.0, 6.0), alt);
        let factor = (6.0f32 / 200.0).exp();
        for id in 100..108 {
            let height = rig.view.heights.height(id, AUDIO_TRACK);
            assert!((height - AUDIO_TRACK * factor).abs() < 1e-3, "{id}: {height}");
        }
        for id in [0, 1] {
            assert_eq!(
                rig.view.heights.height(id, VIDEO_TRACK),
                VIDEO_TRACK,
                "video untouched"
            );
        }
        assert_eq!((rig.view.zoom, rig.view.scroll), (60.0, 0.0), "time untouched");
        // The pointer (y = 300) was 24 points into the second track (from
        // 276): the same spot of that track is still under it.
        let into = 58.0 * factor + 24.0 * factor;
        assert!(
            (rig.view.audio_scroll - (218.0 + into - 300.0)).abs() < 1e-2,
            "{}",
            rig.view.audio_scroll
        );
        rig.wheel(OVER_VIDEO, vec2(0.0, -6.0), alt);
        let height = rig.view.heights.height(0, VIDEO_TRACK);
        assert!(
            (height - VIDEO_TRACK / factor).abs() < 1e-3,
            "wheel down: shorter"
        );
        // Heights stop at the limits.
        for _ in 0..120 {
            rig.wheel(OVER_AUDIO, vec2(0.0, 7.5), alt);
            rig.wheel(OVER_VIDEO, vec2(0.0, -7.5), alt);
        }
        assert!(near(rig.view.heights.height(100, AUDIO_TRACK), MAX_TRACK));
        assert!(near(rig.view.heights.height(0, VIDEO_TRACK), MIN_TRACK));
    }

    #[test]
    fn dragging_the_edge_of_a_tracks_name_resizes_that_track_only() {
        let mut rig = Rig::new(2, 2);
        // The first audio track's name ends at y = 276: down makes it taller.
        let events = drag(&mut rig, pos2(70.0, 276.0), pos2(70.0, 316.0));
        assert!(events.is_empty(), "a height is not an edit: {events:?}");
        assert!(near(rig.view.heights.height(100, AUDIO_TRACK), 98.0));
        assert_eq!(rig.view.heights.height(101, AUDIO_TRACK), AUDIO_TRACK);
        // The first video track's name begins at y = 158: up makes it taller.
        drag(&mut rig, pos2(70.0, 158.0), pos2(70.0, 128.0));
        assert!(near(rig.view.heights.height(0, VIDEO_TRACK), 84.0));
        assert_eq!(rig.view.heights.height(1, VIDEO_TRACK), VIDEO_TRACK);
        // Far past the limit, and off the timeline: it stops at the shortest.
        drag(&mut rig, pos2(70.0, 316.0), pos2(70.0, -500.0));
        assert!(near(rig.view.heights.height(100, AUDIO_TRACK), MIN_TRACK));
        // Double-click the edge (now at y = 246): the usual height again.
        let primary = egui::PointerButton::Primary;
        click(&mut rig, pos2(70.0, 246.0), primary);
        click(&mut rig, pos2(70.0, 246.0), primary);
        assert!(!rig.view.heights.is_changed(100));
        assert!(rig.view.heights.is_changed(0), "the other track keeps its height");
    }

    #[test]
    fn commands_resize_the_area_under_the_pointer_or_both() {
        let mut rig = Rig::new(2, 2);
        rig.view.step(ViewStep::TallerTracks);
        rig.frame(Vec::new());
        assert_eq!(rig.view.heights.height(0, VIDEO_TRACK), VIDEO_TRACK * 1.25);
        assert_eq!(rig.view.heights.height(101, AUDIO_TRACK), AUDIO_TRACK * 1.25);
        rig.move_to(OVER_AUDIO);
        rig.view.step(ViewStep::ShorterTracks);
        rig.view.step(ViewStep::ShorterTracks);
        rig.frame(Vec::new());
        let audio = rig.view.heights.height(101, AUDIO_TRACK);
        assert!((audio - AUDIO_TRACK / 1.25).abs() < 1e-3, "{audio}");
        assert_eq!(
            rig.view.heights.height(0, VIDEO_TRACK),
            VIDEO_TRACK * 1.25,
            "the pointer is over the audio area"
        );
        rig.view.step(ViewStep::UsualTrackHeights);
        rig.frame(Vec::new());
        assert!(!rig.view.heights.is_changed(101));
        assert!(rig.view.heights.is_changed(0));
    }

    #[test]
    fn scrolling_reaches_the_last_track_whatever_the_heights() {
        // Eight audio tracks, the first 300 tall, in an area 168 tall.
        let mut rig = Rig::new(1, 8);
        rig.view.heights.set(100, AUDIO_TRACK, 300.0);
        rig.view.audio_scroll = 10_000.0;
        rig.frame(Vec::new());
        assert!(near(rig.view.audio_scroll, 300.0 + 7.0 * 58.0 - 168.0));
        // Shorter tracks leave less to scroll: the view follows.
        rig.view.heights.reset(100);
        rig.frame(Vec::new());
        assert_eq!(rig.view.audio_scroll, 8.0 * 58.0 - 168.0);
        // The rough cut view has its own usual height for video rows.
        rig.view.rough_cut = true;
        rig.view.heights.set(0, ROUGH_CUT_TRACK, 2.0 * ROUGH_CUT_TRACK);
        rig.view.video_scroll = 10_000.0;
        rig.frame(Vec::new());
        assert_eq!(rig.view.video_scroll, 2.0 * ROUGH_CUT_TRACK - 168.0);
    }
}
