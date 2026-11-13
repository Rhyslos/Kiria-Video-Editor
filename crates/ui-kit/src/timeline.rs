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
//! There can be any number of tracks of each kind. A clip dragged up or down
//! lands on the track of its kind under the pointer, and a file dragged in
//! from a list on the video track under the pointer. Dragged past the last
//! track of its kind (above the top video track, under the bottom audio
//! track) a clip asks for a new track there: a lit strip shows where. While
//! a clip is dragged to a place it could not be put, it is drawn in the
//! warning colour. The column of track names has a right-click menu to add,
//! rename, hide and remove tracks; a video track's name has an eye that
//! hides the track ([`headers`]), and a double-click on a name changes it in
//! place.
//!
//! The video area is as tall as the divider says, or as tall as its tracks
//! need when that is more, as long as the audio area keeps some room
//! ([`TimelineView::fit_video`]). So a track that is added shows without
//! the divider being dragged, and a removed one gives its room back.
//!
//! Two ways of looking at the tracks can be switched ([`track_views`]):
//! compact tracks ([`TimelineView::compact`], every track a thin strip for
//! an overview of many) and the divider off ([`TimelineView::divider`]:
//! video and audio tracks as one list that scrolls as one).
//!
//! The top edge of the labels row is the cache bar ([`cache_bar`]): a thin
//! strip that shows which stretches are prepared ahead of time, being
//! prepared or waiting, from ranges the page passes in.
//!
//! An audio track's level button shows a line across each of its clips at
//! the clip's volume ([`level`]): drag it up or down for that clip's
//! volume, double-click it for 0 dB. Waveforms are drawn as loud as the
//! clip and its track are set, and follow the track's fader while it moves.
//!
//! Clips whose waveform or pictures are still being read in the background
//! say so ([`Loading`]): a slim bar with a percentage on the clip, and a
//! short line by the track's name while any of its clips is being read.
//!
//! The widget knows nothing about projects: the page hands it plain data
//! (seconds as `f64`, for drawing only) and gets back [`TimelineEvent`]s.

pub mod cache_bar;
mod empty_end;
mod fades;
mod gaps;
mod headers;
mod keyframes;
pub mod level;
pub mod lines;
mod loading;
mod rows;
mod slip;
mod speed;
pub use gaps::Gap;
pub use slip::SlipRoom;
mod track_views;

pub use cache_bar::{CacheBar, CacheRange, CacheState};
pub use fades::ClipSound;
pub use keyframes::KeyframeMark;
pub use lines::{LineBeingSet, LineKind, LineOwner, LinePoints, LinesShown};
pub use loading::{LoadKind, LoadState, Loading, percent};
pub use rows::{MAX_TRACK, MIN_TRACK, TrackHeights};
pub use track_views::COMPACT_TRACK;

use crate::Theme;
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};
use rows::Rows;
use track_views::OneList;

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
    /// Background reading the clip's waveform or pictures still wait for
    /// (or that failed): shown as a slim bar with a percentage on the clip.
    pub loading: Option<Loading>,
    /// Its sound's volume and fades: the waveform is drawn as loud as the
    /// clip is set, and its fades as lines from the corners.
    pub sound: ClipSound,
    /// Where its look has keyframes, one mark per frame: small diamonds
    /// along its bottom edge, to click, drag and right-click
    /// ([`keyframes`]). The page fills them in for the selected clip only.
    pub keyframes: Vec<KeyframeMark>,
    /// Its speed, when that changes along it (a ramp): drawn as a line.
    pub speed_curve: Option<SpeedCurve>,
    /// What it plays: a file, another timeline (a double-click opens it),
    /// or nothing of its own (an adjustment layer). Each is drawn its way.
    pub shows: ClipShows,
    /// Its own line of the kind shown ([`TimelineView::lines`]), in
    /// seconds on the timeline: drawn inside it ([`lines`]).
    pub line: Option<LinePoints>,
    /// How far its picture can slip (the Slip tool) and its edges be
    /// trimmed outwards: both drags are held to it as they go.
    pub slip_room: Option<SlipRoom>,
    /// Where what it plays ends, in seconds on the timeline, when that is
    /// before the clip ends (a nested timeline that got shorter): the rest
    /// is hatched.
    pub plays_until: Option<f64>,
}

/// What a clip plays, as the timeline draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipShows {
    /// A part of a file.
    #[default]
    File,
    /// Another timeline: a small stack of two frames on it, and a
    /// double-click (or "Open its timeline" in its menu) opens that timeline.
    Timeline,
    /// An adjustment layer: light stripes across it.
    Adjustment,
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
    /// Where in its file it is along it, when its speed changes over time
    /// (`source_in` and `speed` then mean nothing).
    pub map: Option<SourceMap>,
}

impl ClipPictures {
    /// Seconds into the file at `from_start` seconds into the clip.
    fn file_at(&self, from_start: f64) -> f64 {
        file_at(self.source_in, self.speed, self.map.as_ref(), from_start)
    }
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
    /// Where in its file it is along it, when its speed changes over time
    /// (`source_in` and `speed` then mean nothing).
    pub map: Option<SourceMap>,
}

impl ClipWaveform {
    /// Seconds into the file at `from_start` seconds into the clip.
    fn file_at(&self, from_start: f64) -> f64 {
        file_at(self.source_in, self.speed, self.map.as_ref(), from_start)
    }
}

/// Where in its file a clip whose speed changes over time is, along it:
/// points of (seconds from the clip's start, seconds into its file), in
/// order, with the file going evenly between two and on at the end
/// points' pace beyond them. The page makes it from the clip's ramp.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMap(pub std::sync::Arc<[(f64, f64)]>);

impl SourceMap {
    /// Seconds into the file at `from_start` seconds into the clip.
    pub fn at(&self, from_start: f64) -> f64 {
        let points = &self.0;
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return from_start;
        };
        let next = points.partition_point(|(t, _)| *t <= from_start);
        let pair = match next {
            0 => points.get(..2),
            n if n >= points.len() => points.get(points.len().saturating_sub(2)..),
            n => points.get(n - 1..=n),
        };
        match pair {
            Some([(t0, s0), (t1, s1)]) if t1 > t0 => s0 + (from_start - t0) * (s1 - s0) / (t1 - t0),
            _ => first.1 + (from_start - first.0).max(0.0).min(last.0 - first.0),
        }
    }
}

/// Seconds into a file at `from_start` seconds into its clip.
fn file_at(source_in: f64, speed: f64, map: Option<&SourceMap>, from_start: f64) -> f64 {
    match map {
        Some(map) => map.at(from_start),
        None => source_in + from_start * speed,
    }
}

/// How a clip's speed changes along it, drawn as a thin line over its
/// lower part: points of (share of the clip's length, 0 to 1; speed, 1 is
/// normal), in order. Normal speed is the middle of the band; a quarter
/// and four times as fast are its bottom and top.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedCurve(pub std::sync::Arc<[(f32, f32)]>);

/// One track as the timeline shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTrack {
    pub id: u64,
    pub name: String,
    pub clips: Vec<TimelineClip>,
    pub muted: bool,
    pub solo: bool,
    /// A video track that is not seen in the picture: its clips are drawn dimmed.
    pub hidden: bool,
    /// Volume in decibels.
    pub gain_db: f32,
    /// Background reading still going on for clips of this track: shown
    /// as a short line by the track's name, gone when it is done.
    pub loading: Option<Loading>,
    /// Its line of the kind shown ([`TimelineView::lines`]), across its row.
    pub line: Option<LinePoints>,
    /// Its role, an index into [`TimelineData::roles`]: a mark in its
    /// colour by its name, and chosen in the names' right-click menu.
    pub role: Option<usize>,
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
    /// Coloured stretches to mark (seconds), each over the audio tracks it
    /// names (or every track): the clarity check's findings.
    pub spans: &'a [TimelineSpan],
    /// Keep the playhead in view (while playing).
    pub follow: bool,
    /// What is prepared ahead of time along the timeline, for the strip under the ruler.
    pub cache: CacheBar<'a>,
    /// The ways a value can go on from a keyframe, offered by a keyframe
    /// mark's right-click menu (see [`MarksChange::Ease`]).
    pub keyframe_eases: &'a [&'a str],
    /// The roles an audio track can have, with their colours, offered in
    /// the names' right-click menu (empty: none offered).
    pub roles: &'a [(&'a str, Color32)],
}

/// A stretch the page marks in a colour of its own (see
/// [`TimelineData::spans`]): faintly over the tracks, more strongly over
/// the audio tracks it names, and as a bar along the labels row.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSpan {
    pub start: f64,
    pub end: f64,
    pub colour: Color32,
    /// The audio tracks it is about (empty: every track alike).
    pub tracks: Vec<u64>,
    /// Shown when the pointer rests on its bar.
    pub tip: String,
}

/// Something the person did on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
    /// Click or drag in the ruler: move the playhead there.
    Seek(f64),
    /// The edit needle was dropped here.
    MoveNeedle(f64),
    /// A clip was clicked; `add` with Ctrl or Shift held.
    Select { clip: u64, add: bool },
    /// Empty space was clicked.
    ClearSelection,
    /// "Close the gap" from the right-click menu of a gap: move everything
    /// after it back by its length, on every track.
    CloseGap { start: f64, end: f64 },
    /// A clip (with its linked clips) was dragged by `by` seconds and, when
    /// `track` is given, onto that track of its kind. With `new_track` it
    /// was let go past the last track of its kind (above the top video
    /// track, under the bottom audio track): onto a track to be made there.
    Move {
        clip: u64,
        by: f64,
        track: Option<u64>,
        new_track: bool,
    },
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
    /// A clip's level line was dragged (or double-clicked, for 0 dB):
    /// that clip's volume in decibels, [`level::SILENT_DB`] being silence.
    ClipVolume { clip: u64, gain_db: f32 },
    /// A track's mute, solo or volume changed.
    TrackMix {
        track: u64,
        muted: bool,
        solo: bool,
        gain_db: f32,
    },
    /// The region was drawn or adjusted (`None`: cleared).
    SetRegion(Option<(f64, f64)>),
    /// One of the region's cuts from its right-click menu (in the ruler or
    /// on the tracks): the page makes it one undo step.
    RegionCut(RegionCut),
    /// A dragged file was dropped at `at`: over what is there, or (Ctrl
    /// held) inserted, pushing the rest later. `track` is the video track
    /// under the pointer (none over the audio area).
    DropMedia {
        media: u64,
        at: f64,
        insert: bool,
        track: Option<u64>,
    },
    /// "Add a video track" or "Add an audio track" from the menu of the track names.
    AddTrack { audio: bool },
    /// "Remove" from that menu, for a track without clips.
    RemoveTrack { track: u64 },
    /// A track's name was typed.
    RenameTrack { track: u64, name: String },
    /// A video track's eye was clicked: hide it, or show it again.
    HideTrack { track: u64, hidden: bool },
    /// "Speed" from a clip's right-click menu: play it `num / den` times as fast.
    SetSpeed { clip: u64, num: u32, den: u32 },
    /// A timeline label was double-clicked (or "Edit" chosen on it).
    EditLabel(u64),
    /// "Add a label here" from the right-click menu.
    AddLabel(f64),
    /// "Split here" from the right-click menu: one clip (with its linked
    /// clips), or every clip there when `None`.
    SplitAt { at: f64, clip: Option<u64> },
    /// A clip playing another timeline was double-clicked (or "Open its
    /// timeline" chosen on it): open that timeline.
    OpenNested { clip: u64 },
    /// "Fit to the nested timeline" on a nested clip whose timeline ends
    /// before it does: shorten it to end there.
    FitNested { clip: u64 },
    /// "Add an adjustment layer here" from the right-click menu, at `at`
    /// on the video track under the pointer.
    AddAdjustment { at: f64, track: Option<u64> },
    /// "Nest the selected clips" from the right-click menu.
    NestSelected,
    /// "Sync by sound" from the right-click menu (two or more clips are
    /// selected): line them up by what they hear.
    SyncBySound,
    /// A timeline dragged from the timeline tabs was dropped at `at`, its
    /// picture on the video track `track` (none over the audio area).
    DropTimeline {
        timeline: u64,
        at: f64,
        track: Option<u64>,
    },
    /// A volume or pan line's point was added, dragged or removed (or the
    /// line cleared): every point of it now, in seconds on the timeline.
    /// One undo step.
    Line {
        owner: LineOwner,
        kind: LineKind,
        points: Vec<(f64, f32)>,
    },
    /// An audio track's role was chosen in the names' menu: an index into
    /// [`TimelineData::roles`].
    TrackRole { track: u64, role: usize },
    /// Keyframe marks of `clip` (each the start of a frame, in seconds on
    /// the timeline) were dragged, or something was picked for them in
    /// their right-click menu. One undo step for all of them.
    Keyframes {
        clip: u64,
        marks: Vec<f64>,
        change: MarksChange,
    },
}

/// What was done to keyframe marks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarksChange {
    /// Dragged this far, in seconds (whole frames).
    Move(f64),
    /// "Remove these keyframes".
    Remove,
    /// One of [`TimelineData::keyframe_eases`], by its index.
    Ease(usize),
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

/// A timeline being dragged from the timeline tabs onto the timeline, to
/// be placed in it as one piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedTimeline(pub u64);

/// One end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipEdge {
    Start,
    End,
}

/// What a click or a drag in the ruler moves. The other one is moved by
/// dragging its own handle: the playhead's at the top of the ruler, the
/// edit needle's red one at its bottom. A way of working the page keeps in
/// its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ClickMoves {
    /// The playhead (as it always was).
    #[default]
    Playhead,
    /// The edit needle, when there is one (not with the needle merged into
    /// the playhead: then a click moves the playhead).
    EditNeedle,
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

/// Where a dragged clip would land if let go now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Landing {
    Track(u64),
    /// Past the last track of its kind: on a track to be made there.
    NewTrack,
}

/// A clip being dragged: what is moving and by how much so far.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipDrag {
    clip: u64,
    link: Option<u64>,
    /// The track the clip is on.
    track: u64,
    /// Where it would land if let go now: another track of its kind, or a
    /// new one, once a move has gone up or down far enough.
    over: Landing,
    /// Let go now, the move would be refused: a clip is in the way.
    refused: bool,
    kind: DragKind,
    /// Pointer time when the drag began.
    grabbed_at: f64,
    start: f64,
    end: f64,
    /// Current offset in seconds, after snapping.
    delta: f64,
    /// A move of one of several selected clips: all the selected clips
    /// (and those linked to them) move along their own tracks together.
    together: bool,
}

impl ClipDrag {
    fn moves(&self, c: &TimelineClip) -> bool {
        c.id == self.clip || (self.link.is_some() && c.link == self.link) || (self.together && c.selected)
    }

    /// Where the dragged clip is drawn instead of on its own track, when
    /// the drag has taken it off it.
    fn lands_elsewhere(&self) -> Option<Landing> {
        (self.kind == DragKind::Move && self.over != Landing::Track(self.track)).then_some(self.over)
    }

    /// Whether a clip is in the way of this move, among `data`'s clips: on
    /// the track the dragged clip would land on, or on the track of a clip
    /// linked to it. Linked sound is not counted when the clip changes
    /// track: it then finds a free track by itself. Overlaps shorter than
    /// half a frame are not counted either, as places are rounded to frames.
    fn is_in_the_way(&self, data: &TimelineData) -> bool {
        if self.kind != DragKind::Move {
            return false;
        }
        let slack = 0.5 / data.fps.max(1.0);
        let taken = |track: &TimelineTrack, start: f64, end: f64| {
            track
                .clips
                .iter()
                .any(|c| !self.moves(c) && c.start < end - slack && start < c.start + c.duration - slack)
        };
        let changes_track = self.lands_elsewhere().is_some();
        for (tracks, audio) in [(data.video, false), (data.audio, true)] {
            for track in tracks {
                for c in track.clips.iter().filter(|c| self.moves(c)) {
                    let (start, end) = (c.start + self.delta, c.start + c.duration + self.delta);
                    let lands_on = if c.id != self.clip {
                        (!(audio && changes_track)).then_some(track)
                    } else {
                        match self.over {
                            Landing::Track(id) => tracks.iter().find(|t| t.id == id),
                            Landing::NewTrack => None,
                        }
                    };
                    if lands_on.is_some_and(|track| taken(track, start, end)) {
                        return true;
                    }
                }
            }
        }
        false
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
    /// The video area is taller than the divider's place says when its
    /// tracks need more room, as long as the audio area keeps some
    /// ([`AUDIO_KEEPS`]). Off once the divider has been dragged to where
    /// they don't all show; on again when a video track is added or the
    /// divider is dragged back. View state, like `split`.
    pub fit_video: bool,
    /// Each track's height. View state: the page keeps and saves it.
    pub heights: TrackHeights,
    /// The view (rough cut or not) and the tracks drawn last time, to
    /// notice a track that was added.
    known_tracks: Option<(bool, Vec<u64>)>,
    /// A track was added by hand: the next new audio track is scrolled
    /// into view. Audio tracks that come with an import are not, so the
    /// audio area never scrolls away by itself.
    reveal_audio: bool,
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
    /// Audio tracks whose clips show their level line. View state: the
    /// page keeps and saves it.
    level_lines: std::collections::BTreeSet<u64>,
    /// A clip's level line being dragged.
    level_drag: Option<level::LevelDrag>,
    region_drag: Option<(RegionDrag, (f64, f64))>,
    /// Where the right-click menu was opened: time, and the clip under it.
    menu_at: Option<MenuSpot>,
    /// The video track the right-click menu was opened on.
    menu_track: Option<u64>,
    /// The gap the right-click menu was opened on.
    menu_gap: Option<Gap>,
    /// The gap selected by a click on it (the page closes it).
    gap: Option<Gap>,
    /// The keyframe marks the right-click menu was opened on, with their
    /// clip (the menu is then theirs).
    mark_menu: Option<(u64, Vec<f64>)>,
    /// Keyframe marks chosen to act on together (Shift-click).
    chosen_marks: keyframes::Chosen,
    /// A keyframe mark being dragged.
    mark_drag: Option<keyframes::MarkDrag>,
    /// A box being dragged along a clip's marks to choose them.
    mark_box: Option<keyframes::MarkBox>,
    /// Where the menu of the track names was opened.
    names_menu: Option<headers::MenuSpot>,
    /// A track's name being typed in its header.
    rename: Option<headers::Rename>,
    /// A file dragged in from a list is over the timeline: the time under
    /// the pointer and the video row it would land on (none over the audio area).
    file_over: Option<(f64, Option<usize>)>,
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
    /// Compact tracks: every track [`COMPACT_TRACK`] tall, with no pictures,
    /// waveforms or level lines, whatever height it was given. The heights
    /// are kept and come back when it is off. A way of looking: view state.
    pub compact: bool,
    /// The divider between the video and the audio area. Off, both kinds
    /// of track are one list that scrolls as one; `split` and `fit_video`
    /// wait for it to come back.
    pub divider: bool,
    /// How far the one list (the divider off) is scrolled down, in points.
    list_scroll: f32,
    /// What a click or a drag in the ruler moves.
    pub click_moves: ClickMoves,
    /// Which of the project's timelines it shows, by id: the page sets it,
    /// and keeps it with the layout so the project opens on it again.
    pub showing: Option<u64>,
    /// The playhead's handle is pointed at or dragged: it is drawn lit.
    playhead_lit: bool,
    /// An Alt+drag in the ruler or on empty track space: the playhead
    /// follows the pointer until the button is let go.
    scrub: bool,
    /// Which volume or pan lines are drawn with their points, and worked
    /// by the pointer over the audio tracks (none: lines with points are
    /// drawn faintly, and the pointer works clips). The page sets it.
    pub lines: Option<LinesShown>,
    /// A line's point being dragged.
    line_drag: Option<lines::LineDrag>,
    /// Where a line's right-click menu was opened: whose line, and the point.
    line_menu: Option<(LineOwner, Option<usize>)>,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView {
            zoom: 60.0,
            scroll: 0.0,
            video_scroll: 0.0,
            audio_scroll: 0.0,
            split: 0.5,
            fit_video: true,
            heights: TrackHeights::default(),
            known_tracks: None,
            reveal_audio: false,
            height_drag: None,
            hover_half: None,
            needle_drag: None,
            scrollbar_drag: None,
            clip_drag: None,
            fader_drag: None,
            level_lines: Default::default(),
            level_drag: None,
            region_drag: None,
            menu_at: None,
            menu_track: None,
            menu_gap: None,
            gap: None,
            mark_menu: None,
            chosen_marks: Default::default(),
            mark_drag: None,
            mark_box: None,
            names_menu: None,
            rename: None,
            file_over: None,
            pan_drag: None,
            steps: Vec::new(),
            max_zoom: MAX_ZOOM,
            tool: Tool::Select,
            snapping: true,
            rough_cut: false,
            compact: false,
            divider: true,
            list_scroll: 0.0,
            click_moves: ClickMoves::Playhead,
            showing: None,
            playhead_lit: false,
            scrub: false,
            lines: None,
            line_drag: None,
            line_menu: None,
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
/// A dragged clip stays on its own track until the pointer is this far
/// past the row's edge, so a sideways drag never changes track by accident.
const STAYS_ON_ITS_TRACK: f32 = 6.0;
/// A dragged clip asks for a new track once the pointer is this far past
/// the last track of its kind.
const NEW_TRACK_PAST: f32 = 10.0;
/// The lit strip that shows where a new track would be is at least this tall.
const NEW_TRACK_STRIP: f32 = MIN_TRACK;
/// When the tracks fill their area to its far edge, so there is no room
/// past the last one, the pointer this near that edge asks for a new
/// track (the strip is drawn over the edge there).
const NEW_TRACK_EDGE: f32 = 12.0;
/// The share of the body the audio area keeps (or one usual audio track, if
/// that is more) when the video area grows to show its tracks.
pub const AUDIO_KEEPS: f32 = 0.3;

/// The closest zoom for a timeline at `fps`: single frames stay easy to click.
pub fn max_zoom(fps: f64) -> f32 {
    (fps as f32 * FRAME_WIDTH_ZOOMED_IN).max(MAX_ZOOM)
}

/// One of the timeline's two areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// The areas with the divider at `split` (the share of the body above it).
    fn new(rect: Rect, split: f32) -> Layout {
        let video_h = (Layout::body_height(rect) * split.clamp(0.1, 0.9)).round();
        Layout::with(rect, video_h, DIVIDER)
    }

    /// The areas with no divider: the video area `video_h` tall at the top
    /// of the body and the audio area under it, touching.
    fn one_list(rect: Rect, video_h: f32) -> Layout {
        Layout::with(rect, video_h, 0.0)
    }

    /// The height of the body with no divider in it.
    fn list_height(rect: Rect) -> f32 {
        Layout::body_height(rect) + DIVIDER
    }

    fn with(rect: Rect, video_h: f32, divider_h: f32) -> Layout {
        let left = rect.left() + HEADER_WIDTH;
        let ruler = Rect::from_min_max(pos2(left, rect.top()), pos2(rect.right(), rect.top() + RULER));
        let labels = Rect::from_min_max(
            pos2(left, ruler.bottom()),
            pos2(rect.right(), ruler.bottom() + LABELS),
        );
        let scrollbar = Rect::from_min_max(pos2(left, rect.bottom() - SCROLLBAR), rect.right_bottom());
        let body_top = labels.bottom();
        let video = Rect::from_min_max(
            pos2(rect.left(), body_top),
            pos2(rect.right(), body_top + video_h.max(0.0)),
        );
        let divider = Rect::from_min_max(
            pos2(rect.left(), video.bottom()),
            pos2(rect.right(), video.bottom() + divider_h),
        );
        let audio = Rect::from_min_max(
            pos2(rect.left(), divider.bottom()),
            pos2(rect.right(), scrollbar.top().max(divider.bottom())),
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

    /// The height the video and audio areas share, for a timeline in `rect`.
    fn body_height(rect: Rect) -> f32 {
        (rect.height() - RULER - LABELS - SCROLLBAR - DIVIDER).max(20.0)
    }

    /// The share of that height the video area has.
    fn video_share(&self) -> f32 {
        self.video.height() / (self.video.height() + self.audio.height()).max(1.0)
    }

    fn time_left(&self) -> f32 {
        self.ruler.left()
    }

    fn width(&self) -> f32 {
        self.ruler.width()
    }

    /// The cache bar: the top edge of the labels row.
    fn cache_strip(&self) -> Rect {
        Rect::from_min_max(
            self.labels.min,
            pos2(self.labels.right(), self.labels.top() + cache_bar::HEIGHT),
        )
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

/// The right-click menu of a timeline area, for the time and clip that were
/// clicked (`menu_at`). Call it every frame for the area's response, wherever
/// the pointer is: a menu that is not shown for a frame closes.
fn clip_menu(r: &egui::Response, menu: Menu, events: &mut Vec<TimelineEvent>) {
    let Menu {
        at: menu_at,
        marks,
        eases,
        has_region,
        video_track,
        on_nested,
        ends_empty,
        gap,
        selection,
        several,
    } = menu;
    r.context_menu(|ui| {
        if let Some((clip, marks)) = marks {
            marks_menu(ui, clip, marks, eases, events);
            return;
        }
        let Some((at, clip)) = menu_at else { return };
        if let Some(gap) = gap.filter(|_| clip.is_none())
            && ui
                .button("Close the gap")
                .on_hover_text("Move everything after it back by its length, on every track")
                .clicked()
        {
            events.push(TimelineEvent::CloseGap {
                start: gap.start,
                end: gap.end,
            });
            ui.close();
        }
        if ui.button("Add a label here").clicked() {
            events.push(TimelineEvent::AddLabel(at));
            ui.close();
        }
        if let Some((id, start, end)) = clip {
            if on_nested && ui.button("Open its timeline").clicked() {
                events.push(TimelineEvent::OpenNested { clip: id });
                ui.close();
            }
            if ends_empty
                && ui
                    .button("Fit to the nested timeline")
                    .on_hover_text("Its timeline is shorter now: shorten the clip to end where it does.")
                    .clicked()
            {
                events.push(TimelineEvent::FitNested { clip: id });
                ui.close();
            }
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
        if video_track.is_some() && ui.button("Add an adjustment layer here").clicked() {
            events.push(TimelineEvent::AddAdjustment {
                at,
                track: video_track,
            });
            ui.close();
        }
        if selection && ui.button("Nest the selected clips").clicked() {
            events.push(TimelineEvent::NestSelected);
            ui.close();
        }
        if several && ui.button("Sync by sound…").clicked() {
            events.push(TimelineEvent::SyncBySound);
            ui.close();
        }
        if has_region {
            ui.separator();
            region_menu(ui, events);
            if ui.button("Clear the region").clicked() {
                events.push(TimelineEvent::SetRegion(None));
                ui.close();
            }
        }
    });
}

/// What can be done with the region's stretch of every track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionCut {
    /// Take it out and close the gap (everything after moves back).
    Remove,
    /// Take it out and leave the gap.
    Lift,
    /// Keep only it: everything before and after goes.
    Keep,
}

/// The region's cuts as menu lines, each with the command it is in the
/// shortcut table (so search finds them too).
fn region_menu(ui: &mut Ui, events: &mut Vec<TimelineEvent>) {
    for (cut, text, tip) in [
        (
            RegionCut::Remove,
            "Remove the region",
            "Cut it out of every track and close the gap",
        ),
        (
            RegionCut::Lift,
            "Remove the region, leave a gap",
            "Empty it on every track; nothing moves",
        ),
        (
            RegionCut::Keep,
            "Keep only the region",
            "Remove everything before and after it",
        ),
    ] {
        if ui.button(text).on_hover_text(tip).clicked() {
            events.push(TimelineEvent::RegionCut(cut));
            ui.close();
        }
    }
}

/// What the right-click menu of a timeline area is about.
#[derive(Clone, Copy)]
struct Menu<'a> {
    at: Option<MenuSpot>,
    /// Keyframe marks it was opened on (with their clip), and the marks
    /// shown there now: the menu is theirs.
    marks: Option<(u64, &'a [&'a KeyframeMark])>,
    eases: &'a [&'a str],
    has_region: bool,
    /// The video track the menu was opened on (none over the audio area).
    video_track: Option<u64>,
    /// The clip it was opened on plays another timeline.
    on_nested: bool,
    /// That nested clip is longer than what its timeline has left to play.
    ends_empty: bool,
    /// The gap it was opened on.
    gap: Option<Gap>,
    /// Some clip is selected.
    selection: bool,
    /// Two or more clips are selected.
    several: bool,
}

/// The right-click menu of keyframe marks: go there, remove their
/// keyframes, or set how they go on to the next.
fn marks_menu(
    ui: &mut Ui,
    clip: u64,
    marks: &[&KeyframeMark],
    eases: &[&str],
    events: &mut Vec<TimelineEvent>,
) {
    let times: Vec<f64> = marks.iter().map(|m| m.at).collect();
    let mut keyed: Vec<&str> = Vec::new();
    for word in marks.iter().flat_map(|m| m.keyed.iter()) {
        if !keyed.contains(word) {
            keyed.push(word);
        }
    }
    let title = match (marks.len(), keyed.is_empty()) {
        (1, false) => format!("Keyframes: {}", keyed.join(", ")),
        (1, true) => "Keyframes".to_owned(),
        (n, _) => format!("{n} keyframe marks"),
    };
    ui.label(egui::RichText::new(title).weak().size(12.0));
    let change = |change| TimelineEvent::Keyframes {
        clip,
        marks: times.clone(),
        change,
    };
    if let [one] = marks
        && ui.button("Go here").clicked()
    {
        events.push(TimelineEvent::Seek(one.at));
        ui.close();
    }
    if ui.button("Remove these keyframes").clicked() {
        events.push(change(MarksChange::Remove));
        ui.close();
    }
    if eases.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("From here on").weak().size(12.0));
    // Ticked when every mark goes on that way.
    let shared = marks
        .first()
        .and_then(|m| m.ease)
        .filter(|e| marks.iter().all(|m| m.ease == Some(*e)));
    for (i, name) in eases.iter().enumerate() {
        if ui.selectable_label(shared == Some(i), *name).clicked() {
            events.push(change(MarksChange::Ease(i)));
            ui.close();
        }
    }
}

impl TimelineView {
    /// A track is being added by hand: when it is an audio track, it is
    /// scrolled into view once it shows.
    pub fn reveal_added_track(&mut self) {
        self.reveal_audio = true;
    }

    /// The gap selected by a click on it, if any: its start and end in
    /// seconds.
    pub fn selected_gap(&self) -> Option<(f64, f64)> {
        self.gap.map(|g| (g.start, g.end))
    }

    /// Forgets the selected gap (it was closed, or something else chosen).
    pub fn clear_gap(&mut self) {
        self.gap = None;
    }

    /// A clip being slipped (the Slip tool): the clip and how far its file
    /// would move under it if let go now, in seconds of the timeline (the
    /// `by` of [`TimelineEvent::Slip`]), for the page to show the picture.
    pub fn slipping(&self) -> Option<(u64, f64)> {
        self.clip_drag
            .filter(|d| d.kind == DragKind::Slip && d.delta != 0.0)
            .map(|d| (d.clip, -d.delta))
    }

    /// Keyframe marks being dragged: their clip, the marks (seconds on the
    /// timeline) and how far they have moved so far, for the page to show
    /// the picture as it would be.
    pub fn keyframes_being_moved(&self) -> Option<(u64, &[f64], f64)> {
        self.mark_drag
            .as_ref()
            .filter(|d| d.by != 0.0 && !d.refused)
            .map(|d| (d.clip, d.moving.as_slice(), d.by))
    }

    /// The keyframe marks chosen (clicked, Shift-clicked or boxed): their
    /// clip and their frames' starts, in seconds on the timeline. What Copy
    /// copies. `None` when none are.
    pub fn chosen_keyframes(&self) -> Option<(u64, &[f64])> {
        Some((self.chosen_marks.clip, self.chosen_marks.at.as_slice())).filter(|(_, at)| !at.is_empty())
    }

    /// Chooses the marks of `clip` at `at` (seconds on the timeline), as a
    /// page does after pasting keyframes there.
    pub fn choose_keyframes(&mut self, clip: u64, at: Vec<f64>) {
        self.chosen_marks = keyframes::Chosen { clip, at };
    }

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
                // Compact tracks are all one height; the heights given wait.
                ViewStep::UsualTrackHeights if self.compact => {}
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

    /// How tall `track` of `half` is drawn: its own height, or the compact
    /// height for every track while compact tracks are on.
    fn track_height(&self, half: Half, track: u64) -> f32 {
        if self.compact {
            COMPACT_TRACK
        } else {
            self.heights.height(track, self.usual_height(half))
        }
    }

    /// The page's coloured stretches ([`TimelineData::spans`]): a faint wash
    /// over every track, a stronger one over the audio rows each names, and
    /// a bar along the bottom of the labels row with its tip.
    fn draw_spans(&self, ui: &Ui, layout: &Layout, data: &TimelineData, audio_rows: &Rows) {
        if data.spans.is_empty() {
            return;
        }
        let left = layout.time_left();
        let body = Rect::from_min_max(
            pos2(left, layout.labels.bottom()),
            pos2(layout.ruler.right(), layout.scrollbar.top()),
        );
        let wash = ui.painter_at(body);
        let audio = ui.painter_at(Rect::from_min_max(
            pos2(left, layout.audio.top()),
            layout.audio.max,
        ));
        let bar_row = Rect::from_min_max(
            pos2(left, layout.labels.bottom() - 4.0),
            pos2(layout.ruler.right(), layout.labels.bottom()),
        );
        let bars = ui.painter_at(bar_row);
        for (i, span) in data.spans.iter().enumerate() {
            let (x0, x1) = (self.x_of(left, span.start), self.x_of(left, span.end));
            if x1 < body.left() || x0 > body.right() {
                continue;
            }
            let x1 = x1.max(x0 + 2.0);
            let column = Rect::from_min_max(pos2(x0, body.top()), pos2(x1, body.bottom()));
            wash.rect_filled(column, 0.0, span.colour.gamma_multiply(0.08));
            for (track, row) in data.audio.iter().zip(audio_rows.rects()) {
                if span.tracks.contains(&track.id) {
                    let r = Rect::from_min_max(pos2(x0, row.top()), pos2(x1, row.bottom()));
                    audio.rect_filled(r, 0.0, span.colour.gamma_multiply(0.18));
                }
            }
            let bar = Rect::from_min_max(pos2(x0, bar_row.top()), pos2(x1, bar_row.bottom()));
            bars.rect_filled(bar, 1.0, span.colour.gamma_multiply(0.85));
            if !span.tip.is_empty() {
                let r = ui.interact(
                    bar.intersect(bar_row),
                    ui.scope_id().with(("timeline-span", i)),
                    Sense::hover(),
                );
                r.on_hover_text(&span.tip);
            }
        }
    }

    /// The rows of `half`, each track with its own height.
    fn rows(&self, half: Half, layout: &Layout, data: &TimelineData) -> Rows {
        let heights = tracks_of(half, data)
            .iter()
            .map(|t| self.track_height(half, t.id));
        let (video_scroll, audio_scroll) = self.area_scrolls(layout, data);
        match half {
            Half::Video => Rows::new(layout.video, heights, video_scroll, true),
            Half::Audio => Rows::new(layout.audio, heights, audio_scroll, false),
        }
    }

    /// The height every track of `half` takes together.
    fn content_height(&self, half: Half, data: &TimelineData) -> f32 {
        tracks_of(half, data)
            .iter()
            .map(|t| self.track_height(half, t.id))
            .sum()
    }

    /// The video and audio tracks as one list (the divider off) in a body
    /// `body` tall.
    fn one_list(&self, body: f32, data: &TimelineData) -> OneList {
        OneList {
            video: self.content_height(Half::Video, data),
            audio: self.content_height(Half::Audio, data),
            body,
            scroll: self.list_scroll,
        }
    }

    /// The areas of a timeline drawn in `rect`: with the divider, at its
    /// place (or lower, for the video tracks); without it, as far as each
    /// kind's rows of the one list are in view.
    fn layout(&self, rect: Rect, data: &TimelineData) -> Layout {
        if self.divider {
            Layout::new(rect, self.shown_split(Layout::body_height(rect), data))
        } else {
            let list = self.one_list(Layout::list_height(rect), data);
            Layout::one_list(rect, list.video_area())
        }
    }

    /// The scrolls the video and the audio rows are drawn with: each area's
    /// own, or with the divider off the ones that lay both kinds end to end.
    fn area_scrolls(&self, layout: &Layout, data: &TimelineData) -> (f32, f32) {
        if self.divider {
            return (self.video_scroll, self.audio_scroll);
        }
        let body = layout.video.height() + layout.audio.height();
        // From where the areas are now (the scroll may have changed since
        // they were laid out this frame), so the rows are always in place.
        self.one_list(body, data).area_scrolls(layout.video.height())
    }

    /// The tracks of both areas, from the top of the video area to the
    /// bottom of the audio area.
    fn body(layout: &Layout) -> Rect {
        Rect::from_min_max(layout.video.min, layout.audio.max)
    }

    /// The share of a body `body` tall that shows every video track, or as
    /// many as fit while the audio area keeps the room it is due.
    fn share_for_video_tracks(&self, body: f32, data: &TimelineData) -> f32 {
        let audio_keeps = self
            .content_height(Half::Audio, data)
            .min(AUDIO_TRACK.max(body * AUDIO_KEEPS));
        self.content_height(Half::Video, data).min(body - audio_keeps) / body.max(1.0)
    }

    /// The share of the body the video area gets: the divider's place, or
    /// what its tracks need when that is more.
    fn shown_split(&self, body: f32, data: &TimelineData) -> f32 {
        if self.fit_video {
            self.split.max(self.share_for_video_tracks(body, data))
        } else {
            self.split
        }
    }

    /// Notices tracks that were not there the last time the timeline was
    /// drawn: the video area fits its tracks again, and the newest video
    /// track (and audio track, when one was added by hand,
    /// [`TimelineView::reveal_added_track`]) is scrolled into view. Not when the view has just changed
    /// between rough cut and full, which shows other audio tracks.
    fn notice_new_tracks(&mut self, rect: Rect, data: &TimelineData) {
        let now: Vec<u64> = data.video.iter().chain(data.audio).map(|t| t.id).collect();
        let before = self.known_tracks.replace((self.rough_cut, now));
        let Some((rough_cut, known)) = before else { return };
        if rough_cut != self.rough_cut {
            return;
        }
        let newest = |tracks: &[TimelineTrack]| tracks.iter().rposition(|t| !known.contains(&t.id));
        let new_video = newest(data.video);
        if new_video.is_some() {
            self.fit_video = true;
        }
        let layout = self.layout(rect, data);
        let new_audio = newest(data.audio).filter(|_| std::mem::take(&mut self.reveal_audio));
        for (half, new) in [(Half::Video, new_video), (Half::Audio, new_audio)] {
            let area = match half {
                _ if !self.divider => TimelineView::body(&layout),
                Half::Video => layout.video,
                Half::Audio => layout.audio,
            };
            let rows = self.rows(half, &layout, data);
            let Some(row) = new.and_then(|i| rows.rects().get(i)) else {
                continue;
            };
            // How far the row has to come down (or, negative, go up) to be in view.
            let down = (area.top() - row.top()).max(0.0) - (row.bottom() - area.bottom()).max(0.0);
            self.scroll_half(half, down);
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
        // Compact tracks are all one height; the heights given wait.
        if self.compact {
            return;
        }
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
            _ if !self.divider => self.list_scroll += moved,
            Half::Video => self.video_scroll -= moved,
            Half::Audio => self.audio_scroll += moved,
        }
    }

    /// Never scrolled past the first track or further than the last one needs.
    fn keep_scrolls_in_range(&mut self, layout: &Layout, data: &TimelineData) {
        if !self.divider {
            let body = layout.video.height() + layout.audio.height();
            self.list_scroll = self.one_list(body, data).in_range().scroll;
            return;
        }
        let most = self
            .rows(Half::Video, layout, data)
            .max_scroll(layout.video.height());
        self.video_scroll = self.video_scroll.clamp(0.0, most);
        let most = self
            .rows(Half::Audio, layout, data)
            .max_scroll(layout.audio.height());
        self.audio_scroll = self.audio_scroll.clamp(0.0, most);
    }

    /// Whether the clips of audio track `track` show their level line.
    pub fn level_line(&self, track: u64) -> bool {
        self.level_lines.contains(&track)
    }

    /// Shows or hides the level line on the clips of audio track `track`.
    pub fn set_level_line(&mut self, track: u64, on: bool) {
        if on {
            self.level_lines.insert(track);
        } else {
            self.level_lines.remove(&track);
        }
    }

    /// The audio tracks that show their level line, in order, for keeping.
    pub fn level_lines(&self) -> Vec<u64> {
        self.level_lines.iter().copied().collect()
    }

    /// Shows the level line on exactly these tracks (from what was kept).
    pub fn set_level_lines(&mut self, tracks: impl IntoIterator<Item = u64>) {
        self.level_lines = tracks.into_iter().collect();
    }

    /// The clip whose level line is being dragged, and the volume (dB) it
    /// would get if let go now, for other panels to follow the drag.
    pub fn level_being_set(&self) -> Option<(u64, f32)> {
        self.level_drag.map(|d| (d.clip, d.db))
    }

    /// Fits `length` seconds in `width` pixels.
    /// A line's point being dragged: whose line, which, and every point
    /// as it is now (seconds on the timeline), so the page can be heard
    /// following it before the undo step comes.
    pub fn line_being_set(&self) -> Option<lines::LineBeingSet<'_>> {
        self.line_drag.as_ref().map(|d| lines::LineBeingSet {
            owner: d.owner,
            kind: d.kind,
            points: &d.points,
        })
    }

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
        self.notice_new_tracks(rect, data);
        // A selected gap that is no longer there (closed, filled, undone) is forgotten.
        if let Some(gap) = self.gap {
            let still = data
                .video
                .iter()
                .chain(data.audio)
                .find(|t| t.id == gap.track)
                .and_then(|t| gaps::gap_at(t, (gap.start + gap.end) / 2.0));
            if still != Some(gap) {
                self.gap = None;
            }
        }
        let layout = self.layout(rect, data);
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
        // With the divider off the areas follow the list's scroll, which
        // the wheel or a command may just have changed.
        let layout = self.layout(rect, data);
        // Body: video tracks bottom-up above the divider, audio top-down below it.
        let video_rows = self.rows(Half::Video, &layout, data);
        let audio_rows = self.rows(Half::Audio, &layout, data);
        self.file_over = None;
        // While the middle button pans, nothing else reacts to the pointer.
        if !panning {
            self.input(ui, &layout, data, [&video_rows, &audio_rows], &mut events);
            // On top of the clips: with lines shown, the pointer over the
            // audio tracks works their lines.
            self.lines_input(ui, &layout, data, &audio_rows, &mut events);
        }
        let visible = (self.time_at(left, left), self.time_at(left, rect.right()));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.panel);

        let mut renaming_shown = false;
        // Compact rows keep almost all their height for the clip, and show
        // only its colour and name: pictures and waveforms would not read.
        let inset = track_views::clip_inset(self.compact);
        let full = !self.compact;
        for (area, tracks, rows, half) in [
            (layout.video, data.video, &video_rows, Half::Video),
            (layout.audio, data.audio, &audio_rows, Half::Audio),
        ] {
            let audio = half == Half::Audio;
            let clip_painter = ui.painter_at(Rect::from_min_max(pos2(left, area.top()), area.max));
            let header_painter = ui.painter_at(Rect::from_min_max(area.min, pos2(left, area.bottom())));
            // A clip being dragged to another track is drawn there.
            let elsewhere = self
                .clip_drag
                .and_then(|d| d.lands_elsewhere().map(|over| (d, over)));
            for (i, (track, row)) in tracks.iter().zip(rows.rects()).enumerate() {
                // The edge that resizes the track is reachable even when the
                // row itself has just left the area.
                self.track_edge(ui, theme, half, track, *row, area);
                if !row.intersects(area) {
                    continue;
                }
                let band = Rect::from_min_max(pos2(left, row.top()), pos2(area.right(), row.bottom()));
                clip_painter.rect_filled(band.shrink2(vec2(0.0, 1.0)), 0.0, theme.bg);
                let header = Rect::from_min_max(pos2(layout.header_x, row.top()), pos2(left, row.bottom()))
                    .intersect(area);
                match self.rename.as_mut().filter(|r| r.track == track.id) {
                    Some(rename) => {
                        renaming_shown = true;
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
                        &header_painter,
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
                let text_room =
                    Rect::from_min_max(header.min, pos2(first_button.left() - 4.0, header.bottom()));
                let text_painter =
                    header_painter.with_clip_rect(header_painter.clip_rect().intersect(text_room));
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
                    self.track_controls(ui, theme, track, header, &mut events);
                } else {
                    headers::eye(ui, theme, track, header, self.compact, &mut events);
                }
                let colour = if audio {
                    AUDIO_COLOURS[i % AUDIO_COLOURS.len()]
                } else {
                    theme.line_strong
                };
                // Waveforms are as loud as the track is set, following its
                // fader while it is dragged.
                let track_gain = match self.fader_drag {
                    Some((t, db)) if t == track.id => db,
                    _ => track.gain_db,
                };
                let track_gain = 10f32.powf(track_gain / 20.0);
                let level_line = audio && full && self.level_line(track.id);
                // Clips a few pixels wide can't be told apart: neighbours
                // merge into one shape, so zooming far out stays cheap.
                let mut merged: Option<Rect> = None;
                let leaving = elsewhere
                    .filter(|(d, _)| d.track == track.id)
                    .map(|(d, _)| d.clip);
                let arriving = elsewhere
                    .filter(|(_, over)| *over == Landing::Track(track.id))
                    .and_then(|(d, _)| tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == d.clip));
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
                    let x0 = self.x_of(left, start);
                    let x1 = self.x_of(left, start + duration).max(x0 + 1.0);
                    let r = Rect::from_min_max(pos2(x0, row.top() + inset), pos2(x1, row.bottom() - inset));
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
                    let (pictures, waveform) = match full {
                        true => (clip.pictures.as_ref(), clip.waveform.as_ref()),
                        false => (None, None),
                    };
                    let (pic_r, wave_r) = match (pictures, waveform) {
                        (Some(_), Some(_)) => {
                            let split = r.top() + r.height() * 0.62;
                            (
                                Rect::from_min_max(r.min, pos2(r.right(), split)),
                                Rect::from_min_max(pos2(r.left(), split - 17.0), r.max),
                            )
                        }
                        _ => (r, r),
                    };
                    if let Some(pictures) = pictures {
                        self.draw_pictures(&clip_painter, pic_r, clip, start, pictures);
                    }
                    // The clip's volume: the one its level line is being dragged to.
                    let dragged_db = self.level_drag.filter(|d| d.clip == clip.id).map(|d| d.db);
                    let clip_gain = dragged_db.map_or(clip.sound.gain, level::gain_of_db);
                    if let Some(w) = waveform {
                        let wave_colour = if audio { colour } else { AUDIO_COLOURS[0] };
                        let look = WaveLook {
                            colour: wave_colour,
                            muted: track.muted,
                            gain: clip_gain * track_gain,
                        };
                        self.draw_waveform(&clip_painter, wave_r, clip, start, w, look);
                    }
                    if full {
                        fades::draw(&clip_painter, wave_r, &clip.sound, self.zoom);
                    }
                    if full && let Some(curve) = &clip.speed_curve {
                        speed::draw(&clip_painter, r, curve, theme.accent);
                    }
                    if level_line {
                        let db = dragged_db.unwrap_or_else(|| level::db_of_gain(clip.sound.gain));
                        let hovered = self.level_drag.is_none()
                            && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                                r.contains(p)
                                    && clip_painter.clip_rect().contains(p)
                                    && level::grabs(r, db, p.y)
                            });
                        let label_x = dragged_db
                            .and_then(|_| ui.input(|i| i.pointer.hover_pos()))
                            .map(|p| p.x);
                        level::draw(&clip_painter, r, db, hovered || dragged_db.is_some(), label_x);
                    }
                    if audio && full {
                        let shift = start - clip.start;
                        self.draw_line(
                            ui,
                            &clip_painter,
                            LineOwner::Clip(clip.id),
                            clip.line.as_deref(),
                            r,
                            |t| self.x_of(left, t + shift),
                        );
                    }
                    if let Some(until) = clip.plays_until.filter(|u| *u < start + duration) {
                        empty_end::draw(&clip_painter, theme, r, self.x_of(left, until));
                    }
                    draw_clip_marks(&clip_painter, theme, r, clip, pictures.is_some());
                    if let (Some(d), Some(room)) = (slipping.filter(|d| d.clip == clip.id), clip.slip_room) {
                        slip::draw_room(&clip_painter, theme, r, room, d.delta);
                    }
                    if let Some(loading) = &clip.loading {
                        loading::draw_on_clip(&clip_painter, theme, r, loading);
                    }
                    if self.clip_drag.is_some_and(|d| d.refused && d.moves(clip)) {
                        draw_refused(&clip_painter, r);
                    }
                    if full && !clip.keyframes.is_empty() {
                        // On top of everything else in the clip, where it is
                        // shown (also while it is dragged).
                        let shift = start - clip.start;
                        let x_of = |t: f64| self.x_of(left, t + shift);
                        let chosen = self.chosen_marks.of(clip.id, &clip.keyframes, data.fps);
                        let look = keyframes::MarksLook {
                            colour: theme.accent,
                            chosen: &chosen,
                            drag: self.mark_drag.as_ref().filter(|d| d.clip == clip.id),
                            fps: data.fps,
                        };
                        // A box being dragged goes under the marks it chooses.
                        if let Some(chosen_box) = self.mark_box.as_ref().filter(|b| b.clip == clip.id) {
                            keyframes::draw_box(&clip_painter, r, chosen_box, x_of, theme.accent);
                        }
                        keyframes::draw(&clip_painter, r, &clip.keyframes, x_of, look);
                    }
                }
                if let Some(m) = merged {
                    clip_painter.rect_filled(m, 1.0, tiny_fill(theme, colour, audio));
                }
                if let Some(gap) = self.gap.filter(|g| g.track == track.id) {
                    let (x0, x1) = (self.x_of(left, gap.start), self.x_of(left, gap.end));
                    gaps::draw(&clip_painter, *row, x0, x1, theme.accent);
                }
                if audio && full {
                    let row_band = Rect::from_min_max(
                        pos2(left, row.top() + inset),
                        pos2(area.right(), row.bottom() - inset),
                    );
                    self.draw_line(
                        ui,
                        &clip_painter,
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
            // A clip dragged past the last track of this area: a lit strip
            // where the new track would be, with the clip in it.
            if let Some((drag, Landing::NewTrack)) = elsewhere
                && let Some(clip) = tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == drag.clip)
            {
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
                    pos2(layout.header_x + 12.0, strip.center().y),
                    Align2::LEFT_CENTER,
                    "New track",
                    FontId::proportional(11.5),
                    theme.text,
                );
                let (start, duration) = drag.shown(clip);
                let x0 = self.x_of(left, start);
                let x1 = self.x_of(left, start + duration).max(x0 + 1.0);
                let r = Rect::from_min_max(pos2(x0, strip.top() + inset), pos2(x1, strip.bottom() - inset));
                let colour = if audio {
                    AUDIO_COLOURS[0]
                } else {
                    theme.line_strong
                };
                draw_clip_body(&clip_painter, theme, r, colour, audio);
                draw_clip_marks(&clip_painter, theme, r, clip, false);
            }
        }
        if !renaming_shown {
            // The track whose name was being typed has left the view, or is gone.
            self.rename = None;
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
        // Track header column and its edge.
        painter.line_segment(
            [
                pos2(left, layout.labels.bottom()),
                pos2(left, layout.scrollbar.top()),
            ],
            Stroke::new(1.0, theme.line),
        );
        if self.divider {
            painter.rect_filled(layout.divider, 0.0, theme.line);
        } else if let Some(first) = audio_rows.rects().first()
            && TimelineView::body(&layout).y_range().contains(first.top())
        {
            // One list: a hairline where the video tracks end and the audio
            // tracks begin, nothing to drag.
            painter.line_segment(
                [pos2(rect.left(), first.top()), pos2(rect.right(), first.top())],
                Stroke::new(1.0, theme.line_strong),
            );
        }

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
        self.draw_spans(ui, &layout, data, &audio_rows);
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
        cache_bar::draw(
            &label_painter,
            theme,
            layout.cache_strip(),
            &data.cache,
            visible,
            |t| self.x_of(left, t),
        );
        for marker in data.markers {
            let x = self.x_of(left, marker.at);
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
        draw_playhead_handle(
            &over,
            theme,
            ph,
            &layout,
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
                needle_handle(nx, &layout),
                crate::status::EDIT_NEEDLE,
                Stroke::NONE,
            ));
        }
        events
    }

    /// Draws a track's or a clip's line in `rect`: with its points when
    /// it is the kind of line shown, faintly when it has points but isn't,
    /// not at all otherwise. A line being dragged is drawn as it is now.
    fn draw_line(
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
    fn lines_input(
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
            let clip = track
                .clips
                .iter()
                .find(|c| c.start <= t && t < c.start + c.duration)?;
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
                LineOwner::Clip(id) => data
                    .audio
                    .iter()
                    .flat_map(|t| &t.clips)
                    .find(|c| c.id == id)
                    .and_then(|c| c.line.clone()),
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

    /// M (mute), S (solo) and a volume fader in an audio track's header.
    fn track_controls(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        track: &TimelineTrack,
        header: Rect,
        events: &mut Vec<TimelineEvent>,
    ) {
        if header.height() < headers::least_for_buttons(self.compact) {
            return;
        }
        let id = ui.scope_id().with(("track", track.id));
        let button = |i: f32| headers::button_rect(header, i);
        if !self.compact {
            self.level_button(ui, theme, track.id, button(-1.0), id);
        }
        self.mute_and_solo(ui, theme, track, header, id, events);
        // The fader, in a header with room for it under the buttons.
        if !self.compact {
            self.fader(ui, theme, track, header, id, events);
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

    /// The track's volume fader under its buttons: -60 dB at the left,
    /// +12 dB at the right; double-click resets to 0. Only in a header with
    /// room for it.
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
            let peak = (w.peaks[a..b].iter().fold(0f32, |m, v| m.max(*v)) * gain).min(1.0);
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
            // One list: both kinds shift together.
            _ if !self.divider => self.list_scroll -= pixels,
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
        if self.scrub && !ui.input(|i| i.pointer.primary_down()) {
            self.scrub = false;
        }
        // The ruler takes the pointer first, so that the handles lying on it
        // (the edit needle's, the region's edges) come after it and are on
        // top: a press on a handle drags the handle, not the playhead.
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

        // The playhead's handle at the top of the ruler: dragging it moves
        // the playhead whatever a click in the ruler moves. Not with Shift
        // held, so a region can be begun at the playhead.
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
                events.push(TimelineEvent::Seek(
                    self.time_at(left, p.x).clamp(0.0, data.length.max(0.0)),
                ));
            }
            if let Some(t) = self.alt_click(ui, &r, left, data) {
                events.push(TimelineEvent::Seek(t));
            }
            if r.dragged() || r.drag_stopped() || r.clicked() {
                return;
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

        // Alt+click anywhere in the ruler or the labels row: the playhead
        // goes there, whatever a click moves.
        let ruler_alt = self.alt_click(ui, &ruler, left, data);
        if let Some(t) = ruler_alt {
            events.push(TimelineEvent::Seek(t));
        }

        // Labels: click one to go to it, double-click to rename or recolour it.
        if ruler_alt.is_none()
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
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
            if single && !ui.input(|i| i.modifiers.alt) {
                events.push(TimelineEvent::Seek(marker.at));
                return;
            }
        }

        // Ruler: click or drag to move the playhead; Shift-drag draws the region.
        // The cache bar explains its colours when pointed at.
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
                let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
                self.region_drag = Some((RegionDrag::New(t), (t, t)));
            } else if alt {
                self.scrub = true;
            }
        }
        if ruler_alt.is_some() {
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
                let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
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
                let t = self.time_at(left, p.x).clamp(0.0, data.length.max(0.0));
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
            events.push(TimelineEvent::Seek(
                self.time_at(left, p.x).clamp(0.0, data.length.max(0.0)),
            ));
        }

        // Divider (none in the one list).
        if self.divider {
            self.divider_input(ui, layout, data, id);
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

        // Track names: right-click for the menu of tracks, double-click a name to change it.
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

        // Clips: click selects (or cuts, with the blade); drag moves or trims.
        for (area, tracks, rows, from_bottom) in [
            (layout.video, data.video, rows[0], true),
            (layout.audio, data.audio, rows[1], false),
        ] {
            let time_area = Rect::from_min_max(pos2(left, area.top()), area.max);
            let r = ui.interact(time_area, id.with(("area", from_bottom)), Sense::click_and_drag());
            // While a file is dragged here from a list, the list has the
            // pointer: the area is not pointed at, but the file is over it.
            let file_over = r.dnd_hover_payload::<DraggedMedia>().is_some()
                || r.dnd_hover_payload::<DraggedTimeline>().is_some();
            let file_at = ui.input(|i| i.pointer.interact_pos()).filter(|_| file_over);
            let menu_marks = self.menu_marks(tracks, data.fps);
            let menu_clip = self
                .menu_at
                .and_then(|(_, clip)| clip)
                .and_then(|(id, ..)| tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == id));
            let on_nested = menu_clip.is_some_and(|c| c.shows == ClipShows::Timeline);
            let ends_empty = menu_clip.is_some_and(|c| c.plays_until.is_some());
            let selected = data
                .video
                .iter()
                .chain(data.audio)
                .flat_map(|t| &t.clips)
                .filter(|c| c.selected)
                .take(2)
                .count();
            let menu = Menu {
                at: self.menu_at,
                marks: menu_marks.as_ref().map(|(clip, marks)| (*clip, marks.as_slice())),
                eases: data.keyframe_eases,
                has_region: data.region.is_some(),
                video_track: self.menu_track.filter(|_| from_bottom),
                on_nested,
                ends_empty,
                gap: self.menu_gap,
                selection: selected > 0,
                several: selected > 1,
            };
            let Some(p) = r.interact_pointer_pos().or(r.hover_pos()).or(file_at) else {
                // The pointer is elsewhere, perhaps on the menu itself: an
                // open menu is still shown, or it would close under it.
                clip_menu(&r, menu, events);
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
            // The video track a file dropped here lands on: the one under
            // the pointer, or the nearest above or below the rows.
            let drop_row = rows.nearest(p.y).filter(|_| from_bottom);
            // A file dragged over the timeline: show where it would land.
            if file_over {
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
                continue;
            }
            if let Some(dragged) = r.dnd_release_payload::<DraggedMedia>() {
                let insert = ui.input(|i| i.modifiers.command);
                events.push(TimelineEvent::DropMedia {
                    media: dragged.0,
                    at: t.max(0.0),
                    insert,
                    track: drop_row.and_then(|row| tracks.get(row)).map(|t| t.id),
                });
                continue;
            }
            let hit_track = rows.at(p.y).and_then(|row| tracks.get(row));
            let hit = hit_track.and_then(|track| {
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

            // Alt+drag on empty track space scrubs: the playhead follows the
            // pointer. (On a clip, Alt+drag moves or trims it without snapping.)
            if r.drag_started_by(egui::PointerButton::Primary)
                && hit.is_none()
                && ui.input(|i| i.modifiers.alt)
            {
                self.scrub = true;
            }
            if self.scrub
                && r.dragged()
                && let Some(now) = r.interact_pointer_pos()
            {
                events.push(TimelineEvent::Seek(self.frame_under(left, now.x, data)));
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }

            // A keyframe mark on the clip under the pointer: it comes before
            // the clip itself.
            let on_mark = match (hit, rows.at(p.y).and_then(|row| row_rects.get(row))) {
                (Some(c), Some(row))
                    if !self.compact && !c.keyframes.is_empty() && self.clip_drag.is_none() =>
                {
                    let drawn = Rect::from_min_max(
                        pos2(self.x_of(left, c.start), row.top() + 3.0),
                        pos2(self.x_of(left, c.start + c.duration), row.bottom() - 3.0),
                    );
                    keyframes::hit(drawn, &c.keyframes, |t| self.x_of(left, t), p).map(|mark| (c, mark))
                }
                _ => None,
            };

            // The level line of an audio clip, when its track shows it: it
            // comes before moving the clip, its edges before the line.
            let on_line = match (hit, hit_track, rows.at(p.y).and_then(|row| row_rects.get(row))) {
                (Some(c), Some(track), Some(row))
                    if !from_bottom
                        && !self.compact
                        && self.level_line(track.id)
                        && zone == Some(DragKind::Move)
                        && self.clip_drag.is_none() =>
                {
                    let x0 = self.x_of(left, c.start);
                    let x1 = self.x_of(left, c.start + c.duration).max(x0 + 1.0);
                    let drawn = Rect::from_min_max(pos2(x0, row.top() + 3.0), pos2(x1, row.bottom() - 3.0));
                    let db = level::db_of_gain(c.sound.gain);
                    level::grabs(drawn, db, p.y).then_some((c, drawn, db))
                }
                _ => None,
            };
            // The strip along the bottom of a clip with marks, away from
            // them: a drag there draws a box that chooses marks.
            let on_strip = match (hit, rows.at(p.y).and_then(|row| row_rects.get(row))) {
                (Some(c), Some(row))
                    if on_mark.is_none()
                        && !self.compact
                        && !c.keyframes.is_empty()
                        && zone == Some(DragKind::Move)
                        && self.clip_drag.is_none() =>
                {
                    let drawn = Rect::from_min_max(
                        pos2(self.x_of(left, c.start), row.top() + 3.0),
                        pos2(self.x_of(left, c.start + c.duration), row.bottom() - 3.0),
                    );
                    keyframes::in_strip(drawn, p).then_some(c)
                }
                _ => None,
            };
            let still = self.clip_drag.is_none()
                && self.level_drag.is_none()
                && self.mark_drag.is_none()
                && self.mark_box.is_none();
            if r.hovered()
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
            if r.dragged()
                && let Some(drag) = self.clip_drag
            {
                let raw = t - drag.grabbed_at;
                let delta = if self.snapping && !ui.input(|i| i.modifiers.alt) {
                    self.snapped(raw, &drag, data, left)
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
                drag.refused = drag.is_in_the_way(data);
                self.clip_drag = Some(drag);
                ui.ctx().set_cursor_icon(match drag.kind {
                    DragKind::Move if drag.refused => CursorIcon::NotAllowed,
                    DragKind::Move => CursorIcon::Grabbing,
                    DragKind::Slip | DragKind::Slide => CursorIcon::ResizeColumn,
                    _ => CursorIcon::ResizeHorizontal,
                });
            }
            if r.dragged()
                && let Some(drag) = self.mark_drag.clone()
                && let Some(now) = r.interact_pointer_pos()
                && let Some(c) = tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == drag.clip)
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
            clip_menu(&r, menu, events);
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            if let Some(at) = self.alt_click(ui, &r, left, data) {
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
                    (_, Some(c)) => TimelineEvent::Select { clip: c.id, add },
                    (_, None) => TimelineEvent::ClearSelection,
                });
            }
        }
    }

    /// The marks the right-click menu was opened on, when they are on a
    /// clip of `tracks` and still there.
    fn menu_marks<'d>(&self, tracks: &'d [TimelineTrack], fps: f64) -> Option<(u64, Vec<&'d KeyframeMark>)> {
        let (clip, times) = self.mark_menu.as_ref()?;
        let c = tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == *clip)?;
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

    /// The start of the frame under screen x, inside the timeline: where
    /// an Alt+click or an Alt+drag puts the playhead.
    fn frame_under(&self, left: f32, x: f32, data: &TimelineData) -> f64 {
        let t = self.time_at(left, x).clamp(0.0, data.length.max(0.0));
        if data.fps <= 0.0 {
            return t;
        }
        // A hair over, so a time on a frame's start stays on that frame.
        (t * data.fps + 1e-6).floor() / data.fps
    }

    /// Where an Alt+click on `r` puts the playhead: a click as egui tells
    /// it (pressed and let go without dragging) with Alt held. Alt+drag
    /// keeps its own meaning (no snapping, or scrubbing).
    fn alt_click(&self, ui: &Ui, r: &egui::Response, left: f32, data: &TimelineData) -> Option<f64> {
        if !r.clicked_by(egui::PointerButton::Primary) || !ui.input(|i| i.modifiers.alt) {
            return None;
        }
        let p = r.interact_pointer_pos()?;
        Some(self.frame_under(left, p.x, data))
    }

    /// Whether a click in the ruler moves the edit needle: so set, and the
    /// timeline has a needle of its own.
    fn needle_moves(&self, data: &TimelineData) -> bool {
        self.click_moves == ClickMoves::EditNeedle && data.needle.is_some()
    }

    /// The divider between the video and the audio area: drag it to share
    /// the body between them.
    fn divider_input(&mut self, ui: &Ui, layout: &Layout, data: &TimelineData, id: egui::Id) {
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

/// How wide [`toolbar`] is (at most): nine buttons, a gap, and the
/// spacing between them.
pub const TOOLBAR_WIDTH: f32 = 9.0 * TOOL_BUTTON + 10.0 + 9.0 * TOOLBAR_SPACING;
const TOOL_BUTTON: f32 = 30.0;
/// The toolbar's own spacing between buttons (it sets it, so the width holds).
const TOOLBAR_SPACING: f32 = 4.0;

/// The row of tools above the timeline: Select, Blade, Ripple, Roll, Slip,
/// Slide, then Snapping, Fit and the rough cut view. `tips` gives each
/// button's tooltip (with its shortcut), in that order.
pub fn toolbar(ui: &mut Ui, theme: &Theme, view: &TimelineView, tips: [&str; 9]) -> Option<ToolbarClick> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = TOOLBAR_SPACING;
        let size = vec2(TOOL_BUTTON, 26.0);
        // While the lines are worked the pointer works them, not a tool.
        let tool = |t: Tool| view.tool == t && view.lines.is_none();
        let items = [
            (ToolbarClick::Tool(Tool::Select), tool(Tool::Select), 0),
            (ToolbarClick::Tool(Tool::Blade), tool(Tool::Blade), 1),
            (ToolbarClick::Tool(Tool::Ripple), tool(Tool::Ripple), 2),
            (ToolbarClick::Tool(Tool::Roll), tool(Tool::Roll), 3),
            (ToolbarClick::Tool(Tool::Slip), tool(Tool::Slip), 4),
            (ToolbarClick::Tool(Tool::Slide), tool(Tool::Slide), 5),
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

/// While the volume and pan lines are worked (`TimelineView::lines`): a
/// lit pill after the [`toolbar`] saying so, with a cross to leave them
/// (`tip` names the key, which leaves them too, as Escape does). Returns
/// whether it was clicked.
pub fn lines_chip(ui: &mut Ui, theme: &Theme, tip: &str) -> bool {
    let text = "Lines on   \u{00D7}";
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.5), theme.accent);
    let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 26.0), Sense::click());
    let fill = if r.hovered() {
        theme.accent.gamma_multiply(0.35)
    } else {
        theme.accent.gamma_multiply(0.22)
    };
    ui.painter().rect_filled(rect, 13.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, theme.accent);
    r.on_hover_text(format!(
        "The pointer works the volume and pan lines of the audio tracks, not clips. Click to go back to the clips ({tip}, or Escape)."
    ))
    .clicked()
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
fn past_the_last(row: &Rect, area: Rect, video: bool, y: f32) -> bool {
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

/// Where the lit strip for a new track goes: past the last row of `rows`
/// (above the top video track, under the bottom audio track), as tall as a
/// track usually is where `area` has that much room, and over the edge of
/// the area where it has none.
fn new_track_strip(area: Rect, rows: &Rows, usual: f32, half: Half) -> Rect {
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
/// adjustment layer, and a small stack of two frames in the top right
/// corner of a nested timeline.
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
            loading: None,
            sound: ClipSound::default(),
            keyframes: Vec::new(),
            speed_curve: None,
            shows: ClipShows::File,
            line: None,
            slip_room: None,
            plays_until: None,
        }
    }

    fn track(id: u64, clips: Vec<TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: format!("T{id}"),
            clips,
            muted: false,
            solo: false,
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
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
        cache: Vec<CacheRange>,
        fps: f64,
        modifiers: egui::Modifiers,
        needle: Option<f64>,
        region: Option<(f64, f64)>,
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
                cache: Vec::new(),
                fps: 60.0,
                modifiers: egui::Modifiers::NONE,
                needle: None,
                region: None,
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
                            needle: self.needle,
                            region: self.region,
                            markers: &[],
                            highlights: &[],
                            spans: &[],
                            follow: false,
                            cache: CacheBar {
                                ranges: &self.cache,
                                about: "",
                            },
                            keyframe_eases: &["Linear", "Hold", "Smooth"],
                            roles: &[],
                        };
                        out = self.view.show(ui, &Theme::ASH, &data, size);
                    });
            });
            // Nothing draws these frames.
            output.drop_without_applying_deltas();
            out
        }

        /// How tall the video area is drawn (the body is 336 tall).
        fn video_height(&self) -> f32 {
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
                spans: &[],
                follow: false,
                cache: CacheBar {
                    ranges: &[],
                    about: "",
                },
                keyframe_eases: &["Linear", "Hold", "Smooth"],
                roles: &[],
            };
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
            let share = self.view.shown_split(Layout::body_height(rect), &data);
            Layout::new(rect, share).video.height()
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
            events.iter().any(
                |e| matches!(e, TimelineEvent::Move { clip: 0, by, track: None, .. } if (by - 1.0).abs() < 1e-6)
            ),
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
    fn handles_on_the_ruler_drag_themselves_not_the_playhead() {
        let primary = egui::PointerButton::Primary;
        // Presses at `grab`, drags a second to the right and lets go.
        let drag = |rig: &mut Rig, grab: Pos2| -> Vec<TimelineEvent> {
            let mut events = rig.move_to(grab);
            events.extend(rig.button(grab, primary, true));
            events.extend(rig.move_to(grab + vec2(30.0, 0.0)));
            events.extend(rig.move_to(grab + vec2(60.0, 0.0)));
            events.extend(rig.button(grab + vec2(60.0, 0.0), primary, false));
            events
        };
        let seeks = |events: &[TimelineEvent]| events.iter().any(|e| matches!(e, TimelineEvent::Seek(_)));

        // The edit needle at 5 s is at x = 440; its handle hangs from the ruler's top to y = 26.
        let mut rig = Rig::new(1, 1);
        rig.needle = Some(5.0);
        let events = drag(&mut rig, pos2(441.0, 20.0));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::MoveNeedle(t) if (t - 6.0).abs() < 0.05)),
            "{events:?}"
        );
        assert!(!seeks(&events), "the playhead stays: {events:?}");

        // The end of a region from 2 s to 4 s is at x = 380.
        let mut rig = Rig::new(1, 1);
        rig.region = Some((2.0, 4.0));
        let events = drag(&mut rig, pos2(381.0, 22.0));
        assert!(
            events.iter().any(|e| matches!(
                e,
                TimelineEvent::SetRegion(Some((a, b))) if (a - 2.0).abs() < 1e-6 && (b - 5.0).abs() < 0.05
            )),
            "{events:?}"
        );
        assert!(!seeks(&events), "the playhead stays: {events:?}");

        // Beside the handles the ruler still moves the playhead.
        let mut rig = Rig::new(1, 1);
        rig.needle = Some(5.0);
        let events = drag(&mut rig, pos2(300.0, 10.0));
        assert!(seeks(&events), "{events:?}");
    }

    #[test]
    fn the_cache_bar_takes_no_room_and_leaves_the_ruler_working() {
        let mut rig = Rig::new(1, 1);
        rig.cache = vec![
            CacheRange {
                start: 0.0,
                end: 4.0,
                state: CacheState::Ready,
            },
            CacheRange {
                start: 4.0,
                end: 9.0,
                state: CacheState::Preparing,
            },
        ];
        // The strip is the top of the labels row (y 26 to 30): a click on
        // it still moves the playhead, as anywhere in the ruler.
        let primary = egui::PointerButton::Primary;
        let on_strip = pos2(640.0, 28.0);
        let events: Vec<_> = click(&mut rig, on_strip, primary);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::Seek(t) if (t - 500.0 / 60.0).abs() < 1e-6)),
            "{events:?}"
        );
        // And the tracks sit where they sat without it.
        assert_eq!(
            click(&mut rig, pos2(300.0, 190.0), primary),
            [TimelineEvent::Select { clip: 0, add: false }]
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
            hidden: false,
            gain_db: 0.0,
            loading: None,
            line: None,
            role: None,
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
                TimelineEvent::Move {
                    clip,
                    by,
                    track,
                    new_track,
                } => TimelineEvent::Move {
                    clip,
                    by: round(by),
                    track,
                    new_track,
                },
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

    #[test]
    fn with_lines_shown_a_click_adds_a_point_and_a_drag_moves_it() {
        let mut rig = Rig::new(1, 1);
        rig.view.lines = Some(LinesShown {
            kind: LineKind::Volume,
            on_clips: false,
        });
        // The audio row runs from y 218 for 58 points; 60 points a second.
        // 0 dB sits two thirds of the way up the line's travel.
        let row = Rect::from_min_max(pos2(0.0, 218.0), pos2(1.0, 276.0));
        let unity = level::y_of(row, 0.0);
        let middle = 218.0 + 29.0;
        let x_at = |t: f32| HEADER_WIDTH + t * 60.0;
        let events = click(&mut rig, pos2(x_at(2.0), unity), egui::PointerButton::Primary);
        let Some(TimelineEvent::Line { owner, kind, points }) = events.first() else {
            panic!("a point is added: {events:?}");
        };
        assert_eq!((*owner, *kind), (LineOwner::Track(100), LineKind::Volume));
        assert_eq!(points.len(), 1);
        assert!(
            (points[0].0 - 2.0).abs() < 1e-9 && points[0].1.abs() < 1.0,
            "{points:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, TimelineEvent::Select { .. })),
            "the clip under it stays"
        );
        // The page hands the line back; dragging its point moves it.
        rig.audio[0].line = Some(vec![(2.0, 0.0)].into());
        let events = drag(&mut rig, pos2(x_at(2.0), unity), pos2(x_at(3.0), 225.0));
        let Some(TimelineEvent::Line { points, .. }) = events.first() else {
            panic!("the drag makes one step: {events:?}");
        };
        assert_eq!(points.len(), 1);
        assert!(
            (points[0].0 - 3.0).abs() < 1e-9 && points[0].1 > 6.0,
            "{points:?}"
        );
        assert!(rig.view.line_being_set().is_none(), "let go");
        // On the clips: only inside a clip, and it is the clip's line.
        rig.view.lines = Some(LinesShown {
            kind: LineKind::Pan,
            on_clips: true,
        });
        assert!(click(&mut rig, pos2(x_at(10.0), middle), egui::PointerButton::Primary).is_empty());
        let events = click(&mut rig, pos2(x_at(4.0), middle), egui::PointerButton::Primary);
        assert!(
            matches!(events.first(), Some(TimelineEvent::Line { owner: LineOwner::Clip(100), kind: LineKind::Pan, points }) if points == &vec![(4.0, 0.0)]),
            "{events:?}"
        );
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A rig whose video clip (1 s to 5 s, x = 200 to 440) is selected and
    /// has keyframe marks at `marks`, each keying the numbers given; and
    /// the height where its marks are taken.
    fn rig_with_marks(marks: &[(f64, &[&'static str])]) -> (Rig, f32) {
        let primary = egui::PointerButton::Primary;
        let mut rig = Rig::new(1, 1);
        rig.video[0].clips[0].selected = true;
        rig.video[0].clips[0].keyframes = marks
            .iter()
            .map(|(at, keyed)| KeyframeMark {
                at: *at,
                keyed: keyed.to_vec(),
                ease: None,
            })
            .collect();
        // Somewhere near the bottom edge of the clip, the first mark is taken.
        let x = x_at(marks[0].0) + 1.0;
        for y in (60..212).rev() {
            let events = click(&mut rig, pos2(x, y as f32), primary);
            if events == vec![TimelineEvent::Seek(marks[0].0)] {
                return (rig, y as f32);
            }
        }
        panic!("the mark is under the pointer near the clip's bottom edge");
    }

    /// The x of time `t` in the rig (60 points a second from x = 140).
    fn x_at(t: f64) -> f32 {
        140.0 + t as f32 * 60.0
    }

    #[test]
    fn a_click_on_a_keyframe_mark_moves_the_playhead_there() {
        let primary = egui::PointerButton::Primary;
        let (mut rig, y) = rig_with_marks(&[(3.0, &["opacity"])]);
        // Higher up in the clip, or beside the mark, a click selects as before.
        let select = vec![TimelineEvent::Select { clip: 0, add: false }];
        assert_eq!(click(&mut rig, pos2(321.0, y - 20.0), primary), select);
        assert_eq!(click(&mut rig, pos2(360.0, y), primary), select);
        // Without marks, the same place selects too.
        rig.video[0].clips[0].keyframes.clear();
        assert_eq!(click(&mut rig, pos2(321.0, y), primary), select);
    }

    fn moved_marks(events: &[TimelineEvent]) -> Vec<(Vec<f64>, f64)> {
        let round = |t: f64| (t * 1e6).round() / 1e6;
        events
            .iter()
            .filter_map(|e| match e {
                TimelineEvent::Keyframes {
                    clip: 0,
                    marks,
                    change: MarksChange::Move(by),
                } => Some((marks.clone(), round(*by))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_dragged_mark_moves_by_whole_frames_and_snaps_to_the_playhead() {
        let (mut rig, y) = rig_with_marks(&[(3.0, &["opacity"]), (4.0, &["size"])]);
        // Near the playhead (2 s): it lands on it.
        let events = drag(&mut rig, pos2(x_at(3.0), y), pos2(x_at(2.0) + 3.0, y));
        assert_eq!(moved_marks(&events), [(vec![3.0], -1.0)]);
        assert!(
            !events.iter().any(|e| matches!(e, TimelineEvent::Move { .. })),
            "the clip stays: {events:?}"
        );
        // With Alt, no snapping: whole frames only (3 points is 3 frames).
        rig.modifiers = egui::Modifiers::ALT;
        let events = drag(&mut rig, pos2(x_at(3.0), y), pos2(x_at(2.0) + 3.0, y));
        assert_eq!(moved_marks(&events), [(vec![3.0], -0.95)]);
        rig.modifiers = egui::Modifiers::NONE;
        // Never off the clip: dragged far left it stops on the first frame.
        let events = drag(&mut rig, pos2(x_at(3.0), y), pos2(150.0, y));
        assert_eq!(moved_marks(&events), [(vec![3.0], -2.0)]);
        // Let go where it was taken: nothing.
        let events = drag(&mut rig, pos2(x_at(3.0), y), pos2(x_at(3.0), y));
        assert!(moved_marks(&events).is_empty());
    }

    #[test]
    fn a_box_dragged_along_the_marks_chooses_them_and_the_clip_stays() {
        let marks: &[(f64, &[&'static str])] = &[(2.0, &["opacity"]), (3.0, &["size"]), (4.5, &["turn"])];
        let (mut rig, y) = rig_with_marks(marks);
        // From empty space between the first two marks to past the second.
        let events = drag(&mut rig, pos2(x_at(2.5), y), pos2(x_at(3.5), y));
        assert!(events.is_empty(), "nothing moves or is selected: {events:?}");
        assert_eq!(rig.view.chosen_keyframes(), Some((0, &[3.0][..])));
        // With Shift it adds to them; dragged leftwards too.
        rig.modifiers = egui::Modifiers::SHIFT;
        drag(&mut rig, pos2(x_at(4.8), y), pos2(x_at(4.2), y));
        rig.modifiers = egui::Modifiers::NONE;
        assert_eq!(rig.view.chosen_keyframes(), Some((0, &[3.0, 4.5][..])));
        // Without, it chooses afresh.
        drag(&mut rig, pos2(x_at(1.5), y), pos2(x_at(2.2), y));
        assert_eq!(rig.view.chosen_keyframes(), Some((0, &[2.0][..])));
        // Higher up the clip a drag moves the clip as before.
        let events = drag(&mut rig, pos2(x_at(2.5), y - 25.0), pos2(x_at(3.5), y - 25.0));
        assert!(
            events.iter().any(|e| matches!(e, TimelineEvent::Move { .. })),
            "{events:?}"
        );
        // A click elsewhere chooses nothing.
        click(&mut rig, pos2(x_at(2.5), y - 25.0), egui::PointerButton::Primary);
        assert_eq!(rig.view.chosen_keyframes(), None);
    }

    #[test]
    fn chosen_marks_move_together_and_a_clash_is_shown_while_dragging() {
        let primary = egui::PointerButton::Primary;
        let (mut rig, y) = rig_with_marks(&[(3.0, &["opacity"]), (4.0, &["opacity", "size"])]);
        // Shift-click chooses the second one too, without moving the playhead.
        rig.modifiers = egui::Modifiers::SHIFT;
        let events = click(&mut rig, pos2(x_at(4.0), y), primary);
        assert!(events.is_empty(), "{events:?}");
        rig.modifiers = egui::Modifiers::NONE;
        let events = drag(&mut rig, pos2(x_at(4.0), y), pos2(x_at(4.5), y));
        assert_eq!(moved_marks(&events), [(vec![4.0, 3.0], 0.5)]);
        // The first one alone (a click chooses it alone), onto the second:
        // opacity twice in a frame is shown refused.
        click(&mut rig, pos2(x_at(3.0), y), primary);
        rig.move_to(pos2(x_at(3.0), y));
        rig.button(pos2(x_at(3.0), y), primary, true);
        rig.move_to(pos2(x_at(3.5), y));
        rig.move_to(pos2(x_at(4.0), y));
        assert!(rig.view.mark_drag.as_ref().is_some_and(|d| d.refused));
        assert!(rig.view.keyframes_being_moved().is_none(), "nothing to show");
        rig.move_to(pos2(x_at(3.5), y));
        assert!(rig.view.mark_drag.as_ref().is_some_and(|d| !d.refused));
        assert_eq!(
            rig.view
                .keyframes_being_moved()
                .map(|(c, m, by)| (c, m.to_vec(), by)),
            Some((0, vec![3.0], 0.5))
        );
        rig.button(pos2(x_at(3.5), y), primary, false);
    }

    #[test]
    fn a_marks_menu_removes_eases_and_goes_there() {
        let (mut rig, y) = rig_with_marks(&[(3.0, &["position", "opacity"])]);
        let at = pos2(x_at(3.0), y);
        let mut asked = Vec::new();
        for down in (0..30).map(|step| 4.0 + step as f32 * 5.0) {
            rig.frame(vec![key(egui::Key::Escape)]);
            rig.frame(vec![]);
            click(&mut rig, at, egui::PointerButton::Secondary);
            rig.frame(vec![]);
            assert!(egui::Popup::is_any_open(&rig.ctx), "the menu opened");
            for event in click(&mut rig, at + vec2(30.0, down), egui::PointerButton::Primary) {
                if !asked.contains(&event) {
                    asked.push(event);
                }
            }
        }
        let change = |change| TimelineEvent::Keyframes {
            clip: 0,
            marks: vec![3.0],
            change,
        };
        assert_eq!(
            asked,
            [
                TimelineEvent::Seek(3.0),
                change(MarksChange::Remove),
                change(MarksChange::Ease(0)),
                change(MarksChange::Ease(1)),
                change(MarksChange::Ease(2)),
            ]
        );
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
        assert_eq!(
            events,
            [TimelineEvent::Move {
                clip: 100,
                by: 0.5,
                track: None,
                new_track: false
            }]
        );
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
    fn the_right_click_menu_stays_open_while_the_pointer_moves_to_it() {
        let mut rig = Rig::new(1, 2);
        click(&mut rig, pos2(320.0, 330.0), egui::PointerButton::Secondary);
        rig.frame(vec![]);
        assert!(egui::Popup::is_any_open(&rig.ctx), "open after the click");
        // Onto the menu, which covers the timeline there, and on down it.
        for y in [340.0, 348.0, 356.0] {
            rig.frame(vec![egui::Event::PointerMoved(pos2(336.0, y))]);
            rig.frame(vec![]);
            assert!(
                egui::Popup::is_any_open(&rig.ctx),
                "still open with the pointer at y = {y}"
            );
        }
        // And with the pointer outside the window altogether.
        rig.frame(vec![egui::Event::PointerGone]);
        rig.frame(vec![]);
        assert!(
            egui::Popup::is_any_open(&rig.ctx),
            "still open with the pointer gone"
        );
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
    fn audio_tracks_that_come_with_an_import_never_scroll_the_area() {
        // Three audio tracks fill an area 168 tall; a fourth comes in.
        let mut rig = Rig::new(1, 3);
        rig.frame(Vec::new());
        rig.audio.push(track(103, Vec::new()));
        rig.frame(Vec::new());
        assert_eq!(rig.view.audio_scroll, 0.0, "an import's track waits below");
        // Added by hand, it is brought into view.
        rig.view.reveal_added_track();
        rig.audio.push(track(104, Vec::new()));
        rig.frame(Vec::new());
        assert!(rig.view.audio_scroll > 0.0);
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
        // The rough cut view has its own usual height for video rows. (With
        // the divider left where it was put: the area does not grow.)
        rig.view.rough_cut = true;
        rig.view.fit_video = false;
        rig.view.heights.set(0, ROUGH_CUT_TRACK, 2.0 * ROUGH_CUT_TRACK);
        rig.view.video_scroll = 10_000.0;
        rig.frame(Vec::new());
        assert_eq!(rig.view.video_scroll, 2.0 * ROUGH_CUT_TRACK - 168.0);
    }

    fn moved(clip: u64, by: f64, track: Option<u64>) -> Vec<TimelineEvent> {
        vec![TimelineEvent::Move {
            clip,
            by,
            track,
            new_track: false,
        }]
    }

    /// A clip let go past the last track of its kind.
    fn moved_to_a_new_track(clip: u64, by: f64) -> Vec<TimelineEvent> {
        vec![TimelineEvent::Move {
            clip,
            by,
            track: None,
            new_track: true,
        }]
    }

    #[test]
    fn a_clip_dragged_up_or_down_lands_on_the_track_under_the_pointer() {
        // Two video tracks: track 0 from y = 158 to 212, track 1 on it from
        // 104 to 158. Two audio tracks: 100 from 218 to 276, 101 to 334.
        // Every clip runs from x = 200 to 440.
        let mut rig = Rig::new(2, 2);
        rig.view.snapping = false;
        let on_v1 = pos2(300.0, 190.0);
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 130.0)),
            moved(0, 0.5, Some(1)),
            "up a track and half a second later"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(300.0, 130.0)),
            moved(0, 0.0, Some(1)),
            "straight up is a move too"
        );
        assert_eq!(
            drag(&mut rig, pos2(300.0, 130.0), pos2(270.0, 190.0)),
            moved(1, -0.5, Some(0)),
            "and down"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 154.0)),
            moved(0, 0.5, None),
            "a few points past its row's edge it stays on its track"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 98.0)),
            moved(0, 0.5, Some(1)),
            "a few points above the top track: still the top track"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 300.0)),
            moved(0, 0.5, None),
            "a video clip never lands on an audio track"
        );
        assert!(
            drag(&mut rig, on_v1, pos2(300.0, 300.0)).is_empty(),
            "and dragged nowhere, nothing happens"
        );
        // Sound clips move between audio tracks the same way.
        assert_eq!(
            drag(&mut rig, pos2(300.0, 240.0), pos2(360.0, 300.0)),
            moved(100, 1.0, Some(101))
        );
        assert_eq!(
            drag(&mut rig, pos2(300.0, 300.0), pos2(300.0, 100.0)),
            moved(101, 0.0, Some(100)),
            "past the divider: the first audio track"
        );
    }

    #[test]
    fn a_dragged_clip_follows_the_pointer_to_the_track_under_it_and_back() {
        let mut rig = Rig::new(2, 1);
        rig.view.snapping = false;
        let primary = egui::PointerButton::Primary;
        let grab = pos2(300.0, 190.0);
        rig.move_to(grab);
        rig.button(grab, primary, true);
        rig.move_to(pos2(310.0, 185.0));
        let over = |rig: &Rig| {
            rig.view.clip_drag.map(|d| {
                let landing = |landing| match landing {
                    Landing::Track(track) => track,
                    Landing::NewTrack => u64::MAX,
                };
                (d.track, landing(d.over), d.lands_elsewhere().map(landing))
            })
        };
        assert_eq!(over(&rig), Some((0, 0, None)));
        rig.move_to(pos2(330.0, 130.0));
        assert_eq!(over(&rig), Some((0, 1, Some(1))), "shown on the track above");
        let drag = rig.view.clip_drag.unwrap();
        assert!((drag.delta - 0.5).abs() < 1e-6, "and still moved sideways");
        rig.move_to(pos2(330.0, 180.0));
        assert_eq!(over(&rig), Some((0, 0, None)), "back on its own");
        let events = rig.button(pos2(330.0, 180.0), primary, false);
        assert!(
            matches!(
                events[..],
                [TimelineEvent::Move {
                    clip: 0,
                    track: None,
                    ..
                }]
            ),
            "{events:?}"
        );
        assert_eq!(rig.view.clip_drag, None);
    }

    #[test]
    fn only_a_move_changes_track() {
        let mut rig = Rig::new(2, 1);
        rig.view.snapping = false;
        // An edge dragged up and inward still trims on its own track.
        assert_eq!(
            drag(&mut rig, pos2(438.0, 190.0), pos2(378.0, 130.0)),
            [TimelineEvent::Trim {
                clip: 0,
                edge: ClipEdge::End,
                to: 4.0
            }]
        );
        rig.view.tool = Tool::Slide;
        assert_eq!(
            drag(&mut rig, pos2(300.0, 190.0), pos2(330.0, 130.0)),
            [TimelineEvent::Slide { clip: 0, by: 0.5 }]
        );
        rig.view.tool = Tool::Slip;
        assert_eq!(
            drag(&mut rig, pos2(300.0, 190.0), pos2(330.0, 130.0)),
            [TimelineEvent::Slip { clip: 0, by: -0.5 }]
        );
        // The ripple tool moves a clip by its body, as the select tool does.
        rig.view.tool = Tool::Ripple;
        assert_eq!(
            drag(&mut rig, pos2(300.0, 190.0), pos2(330.0, 130.0)),
            moved(0, 0.5, Some(1))
        );
    }

    #[test]
    fn a_clip_dragged_past_the_last_track_of_its_kind_asks_for_a_new_one() {
        // Two video tracks: track 0 from y = 158 to 212, track 1 on it from
        // 104 to 158, and room above it. Two audio tracks: 100 from 218 to
        // 276, 101 to 334, and room under it.
        let mut rig = Rig::new(2, 2);
        rig.view.snapping = false;
        let (on_v1, on_v2) = (pos2(300.0, 190.0), pos2(300.0, 130.0));
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 80.0)),
            moved_to_a_new_track(0, 0.5),
            "above the top track"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(300.0, 10.0)),
            moved_to_a_new_track(0, 0.0),
            "also from over the ruler, and straight up"
        );
        assert_eq!(
            drag(&mut rig, pos2(300.0, 240.0), pos2(360.0, 350.0)),
            moved_to_a_new_track(100, 1.0),
            "a sound clip under the bottom audio track"
        );
        assert_eq!(
            drag(&mut rig, pos2(300.0, 240.0), pos2(360.0, 340.0)),
            moved(100, 1.0, Some(101)),
            "just under it: still the bottom track"
        );
        assert_eq!(
            drag(&mut rig, on_v1, pos2(330.0, 395.0)),
            moved(0, 0.5, None),
            "a video clip dragged down past everything stays a video clip on its track"
        );
        // Each top track has one clip: on its own up there already, a new
        // track would only leave an empty one behind.
        assert_eq!(drag(&mut rig, on_v2, pos2(330.0, 60.0)), moved(1, 0.5, None));
        rig.video[1].clips.push(clip(9, 8.0, 2.0));
        assert_eq!(
            drag(&mut rig, on_v2, pos2(330.0, 60.0)),
            moved_to_a_new_track(1, 0.5),
            "with company on its track it can leave"
        );
        // The tool that trims never changes track, so never asks for one.
        assert_eq!(
            drag(&mut rig, pos2(438.0, 190.0), pos2(378.0, 60.0)),
            [TimelineEvent::Trim {
                clip: 0,
                edge: ClipEdge::End,
                to: 4.0
            }]
        );
    }

    #[test]
    fn the_new_track_is_shown_while_the_clip_is_dragged_there_and_not_past_tracks_out_of_view() {
        let mut rig = Rig::new(2, 1);
        rig.view.snapping = false;
        let primary = egui::PointerButton::Primary;
        let grab = pos2(300.0, 190.0);
        rig.move_to(grab);
        rig.button(grab, primary, true);
        rig.move_to(pos2(310.0, 150.0));
        rig.move_to(pos2(330.0, 80.0));
        let drag_now = rig.view.clip_drag.unwrap();
        assert_eq!(
            drag_now.lands_elsewhere(),
            Some(Landing::NewTrack),
            "the strip is lit"
        );
        assert!(!drag_now.refused, "a new track has room for it");
        rig.move_to(pos2(330.0, 130.0));
        assert_eq!(
            rig.view.clip_drag.unwrap().lands_elsewhere(),
            Some(Landing::Track(1)),
            "back down on the top track"
        );
        rig.button(pos2(330.0, 130.0), primary, false);

        // Four video tracks in an area that shows three: the top one is out
        // of view above, so a clip dragged up goes to it, not past it.
        let mut rig = Rig::new(4, 1);
        rig.view.snapping = false;
        rig.view.fit_video = false;
        assert_eq!(
            drag(&mut rig, grab, pos2(330.0, 10.0)),
            moved(0, 0.5, Some(3)),
            "the track at the top of what is shown"
        );
    }

    /// Tracks that fill their area leave no room past the last: the band
    /// along the area's edge asks for a new track then, as the strip drawn
    /// over that edge shows. With room, only past the last track does.
    #[test]
    fn a_new_track_is_asked_for_at_the_edge_when_the_tracks_fill_the_area() {
        let area = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 300.0));
        // Audio filling the area to its bottom.
        let full = Rect::from_min_max(pos2(0.0, 242.0), pos2(500.0, 300.0));
        assert!(!past_the_last(&full, area, false, 280.0), "on the last track");
        assert!(past_the_last(&full, area, false, 292.0), "at the edge");
        assert!(past_the_last(&full, area, false, 310.0), "past it");
        // With room under it: only well past it.
        let room = Rect::from_min_max(pos2(0.0, 150.0), pos2(500.0, 208.0));
        assert!(!past_the_last(&room, area, false, 212.0));
        assert!(past_the_last(&room, area, false, 220.0));
        // Scrolled so the last track's bottom is out of view: never.
        let hidden = Rect::from_min_max(pos2(0.0, 260.0), pos2(500.0, 330.0));
        assert!(!past_the_last(&hidden, area, false, 299.0));
        // Video fills from the top the same way.
        let top = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 150.0));
        assert!(past_the_last(&top, area, true, 105.0));
        assert!(!past_the_last(&top, area, true, 120.0));
    }

    #[test]
    fn the_strip_for_a_new_track_lies_past_the_last_row_or_over_the_edge() {
        let area = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 300.0));
        let ys = |r: Rect| (r.top(), r.bottom());
        // Two video rows of 50 from the bottom: room above them.
        let video = Rows::new(area, [50.0, 50.0], 0.0, true);
        assert_eq!(
            ys(new_track_strip(area, &video, 54.0, Half::Video)),
            (146.0, 200.0)
        );
        // Rows up to the top of the area: the strip lies over its edge.
        let full = Rows::new(area, [100.0, 100.0], 0.0, true);
        assert_eq!(
            ys(new_track_strip(area, &full, 54.0, Half::Video)),
            (100.0, 100.0 + NEW_TRACK_STRIP)
        );
        // Little room: as much as there is.
        let nearly = Rows::new(area, [100.0, 70.0], 0.0, true);
        assert_eq!(
            ys(new_track_strip(area, &nearly, 54.0, Half::Video)),
            (100.0, 130.0)
        );
        // Audio rows grow down: the strip is under them.
        let audio = Rows::new(area, [58.0], 0.0, false);
        assert_eq!(
            ys(new_track_strip(area, &audio, 58.0, Half::Audio)),
            (158.0, 216.0)
        );
        let full = Rows::new(area, [100.0, 100.0], 0.0, false);
        assert_eq!(
            ys(new_track_strip(area, &full, 58.0, Half::Audio)),
            (300.0 - NEW_TRACK_STRIP, 300.0)
        );
    }

    /// Presses at `from`, drags to `to` and says whether the clip is shown
    /// as refused there, then lets go.
    fn refused_at(rig: &mut Rig, from: Pos2, to: Pos2) -> bool {
        let primary = egui::PointerButton::Primary;
        rig.move_to(from);
        rig.button(from, primary, true);
        rig.move_to(from + (to - from) / 2.0);
        rig.move_to(to);
        let refused = rig.view.clip_drag.is_some_and(|d| d.refused);
        rig.button(to, primary, false);
        refused
    }

    #[test]
    fn a_clip_dragged_where_it_cannot_go_is_shown_as_refused() {
        // Two video tracks and one audio track, each with a clip from 1 s
        // to 5 s (x = 200 to 440); 60 points are a second.
        let mut rig = Rig::new(2, 1);
        rig.view.snapping = false;
        let (on_v1, on_v2) = (pos2(300.0, 190.0), pos2(300.0, 130.0));
        assert!(
            refused_at(&mut rig, on_v1, on_v2),
            "straight up: onto the clip that is there"
        );
        assert!(
            !refused_at(&mut rig, on_v1, pos2(600.0, 130.0)),
            "up and five seconds later: room"
        );
        assert!(
            refused_at(&mut rig, on_v1, pos2(500.0, 130.0)),
            "its start still under the other's end"
        );
        assert!(
            !refused_at(&mut rig, on_v1, pos2(360.0, 190.0)),
            "along its own track"
        );
        assert!(
            !refused_at(&mut rig, on_v1, pos2(300.0, 60.0)),
            "a new track has room"
        );
        // Let go there, the move is still asked for: the page says why not.
        assert_eq!(drag(&mut rig, on_v1, on_v2), moved(0, 0.0, Some(1)));

        // The clip on V1 and the sound clip belong together, and other
        // sound lies from 6 s to 10 s on the same audio track.
        rig.video[0].clips[0].link = Some(7);
        rig.audio[0].clips[0].link = Some(7);
        rig.audio[0].clips.push(clip(150, 6.0, 4.0));
        assert!(
            refused_at(&mut rig, on_v1, pos2(600.0, 190.0)),
            "along its track: its sound would land on other sound"
        );
        assert!(
            !refused_at(&mut rig, on_v1, pos2(600.0, 130.0)),
            "to another track: its sound finds a free one"
        );
        // A touch is not an overlap: from 5 s on, right after the other clip.
        rig.video[0].clips[0].link = None;
        rig.view.snapping = true;
        assert!(
            !refused_at(&mut rig, on_v1, pos2(538.0, 130.0)),
            "snapped to its end"
        );
    }

    #[test]
    fn the_video_area_is_as_tall_as_its_tracks_need_and_gives_the_room_back() {
        // A body 336 tall, the divider in the middle: 168 for video, which
        // shows three tracks of 54.
        let mut rig = Rig::new(2, 3);
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), 168.0);
        let add = |rig: &mut Rig| {
            let id = rig.video.len() as u64;
            rig.video.push(track(id, Vec::new()));
            rig.frame(Vec::new());
        };
        add(&mut rig);
        assert_eq!(rig.video_height(), 168.0, "a third track fits already");
        add(&mut rig);
        assert_eq!(
            rig.video_height(),
            216.0,
            "four tracks: the area grew to show them"
        );
        assert_eq!(rig.view.split, 0.5, "the divider's own place is as it was");
        assert_eq!(rig.view.video_scroll, 0.0);
        // Six tracks need 324: the audio area keeps three tenths of the body.
        add(&mut rig);
        add(&mut rig);
        assert_eq!(rig.video_height(), 235.0);
        assert!(
            near(rig.view.video_scroll, 324.0 - 235.0),
            "and the new track is scrolled into view: {}",
            rig.view.video_scroll
        );
        // Tracks are removed again (or their adding undone).
        rig.video.truncate(4);
        rig.frame(Vec::new());
        assert_eq!((rig.video_height(), rig.view.video_scroll), (216.0, 0.0));
        rig.video.truncate(2);
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), 168.0, "the room is given back");
        // Taller tracks need more room too, and with a single audio track
        // the audio area keeps only what that track needs.
        rig.audio.truncate(1);
        rig.view.heights.set(0, VIDEO_TRACK, 200.0);
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), 254.0);
        rig.view.heights.set(0, VIDEO_TRACK, 400.0);
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), 336.0 - AUDIO_TRACK);
    }

    #[test]
    fn the_divider_can_still_be_dragged_anywhere_and_a_new_track_shows_again() {
        // Four video tracks: the area is 216 tall, the divider at y = 260.
        let mut rig = Rig::new(4, 3);
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), 216.0);
        // Up by 100: fewer tracks show than there are, as asked.
        drag(&mut rig, pos2(500.0, 263.0), pos2(500.0, 163.0));
        assert!(near(rig.video_height(), 116.0), "{}", rig.video_height());
        assert!(!rig.view.fit_video);
        rig.frame(Vec::new());
        assert!(near(rig.video_height(), 116.0), "and it stays there");
        // A track is added: the area shows its tracks again.
        rig.video.push(track(4, Vec::new()));
        rig.frame(Vec::new());
        assert!(rig.view.fit_video);
        assert_eq!(rig.video_height(), 235.0);
        // Dragged down past what the tracks need, the divider is simply there.
        rig.video.truncate(2);
        rig.frame(Vec::new());
        let at = 44.0 + rig.video_height() + 3.0;
        drag(&mut rig, pos2(500.0, at), pos2(500.0, at + 60.0));
        assert!(rig.view.fit_video);
        let tall = rig.video_height();
        assert!(tall > 168.0);
        rig.video.push(track(2, Vec::new()));
        rig.frame(Vec::new());
        assert_eq!(rig.video_height(), tall, "room enough: nothing moves");
    }

    #[test]
    fn a_new_audio_track_is_scrolled_into_view_but_not_when_the_view_changes() {
        // Three audio tracks of 58 in an area 168 tall.
        let mut rig = Rig::new(1, 3);
        rig.frame(Vec::new());
        assert_eq!(rig.view.audio_scroll, 0.0);
        // Added by hand (an import's tracks are not scrolled to).
        rig.view.reveal_added_track();
        rig.audio.push(track(103, Vec::new()));
        rig.frame(Vec::new());
        assert!(
            near(rig.view.audio_scroll, 4.0 * 58.0 - 168.0),
            "its row is in view"
        );
        // Switching to the rough cut view and back shows other tracks:
        // that is not a track being added.
        rig.view.audio_scroll = 0.0;
        rig.view.rough_cut = true;
        rig.audio.push(track(104, Vec::new()));
        rig.frame(Vec::new());
        assert_eq!(rig.view.audio_scroll, 0.0);
    }

    /// Lets go of a file dragged from a list with the pointer at `at`.
    fn drop_file(rig: &mut Rig, at: Pos2) -> Vec<TimelineEvent> {
        let primary = egui::PointerButton::Primary;
        // The drag began elsewhere, on the list.
        let list = pos2(60.0, 395.0);
        rig.move_to(list);
        rig.button(list, primary, true);
        egui::DragAndDrop::set_payload(&rig.ctx, DraggedMedia(7));
        rig.move_to(at);
        rig.move_to(at);
        rig.button(at, primary, false)
    }

    #[test]
    fn a_dropped_timeline_lands_where_a_file_would() {
        let mut rig = Rig::new(2, 2);
        let primary = egui::PointerButton::Primary;
        let (tabs, at) = (pos2(60.0, 395.0), pos2(500.0, 130.0));
        rig.move_to(tabs);
        rig.button(tabs, primary, true);
        egui::DragAndDrop::set_payload(&rig.ctx, DraggedTimeline(3));
        rig.move_to(at);
        assert_eq!(rig.view.file_over.map(|(t, _)| t), Some(6.0), "marked while over");
        rig.move_to(at);
        assert_eq!(
            rig.button(at, primary, false),
            vec![TimelineEvent::DropTimeline {
                timeline: 3,
                at: 6.0,
                track: Some(1),
            }]
        );
        // A file still drops as a file afterwards.
        assert!(matches!(
            drop_file(&mut rig, at).as_slice(),
            [TimelineEvent::DropMedia { media: 7, .. }]
        ));
    }

    #[test]
    fn a_double_click_on_a_nested_timelines_clip_opens_it() {
        let mut rig = Rig::new(2, 2);
        // The clip on V1 runs from 1 s to 5 s: x = 300 is in it.
        let on_clip = pos2(300.0, 190.0);
        let double = |rig: &mut Rig| {
            let primary = egui::PointerButton::Primary;
            rig.move_to(on_clip);
            let mut events = Vec::new();
            for _ in 0..2 {
                events.extend(rig.button(on_clip, primary, true));
                events.extend(rig.button(on_clip, primary, false));
            }
            events
        };
        let opened = |events: &[TimelineEvent]| events.contains(&TimelineEvent::OpenNested { clip: 0 });
        assert!(!opened(&double(&mut rig)), "a file's clip opens nothing");
        // A fresh timeline, so these clicks are not taken as a third and fourth.
        let mut rig = Rig::new(2, 2);
        rig.video[0].clips[0].shows = ClipShows::Timeline;
        let events = double(&mut rig);
        assert!(opened(&events), "{events:?}");
    }

    #[test]
    fn a_dropped_file_lands_on_the_video_track_under_the_pointer() {
        let mut rig = Rig::new(2, 2);
        let dropped = |at: f64, insert, track| {
            vec![TimelineEvent::DropMedia {
                media: 7,
                at,
                insert,
                track,
            }]
        };
        // x = 500 is 6 s in.
        assert_eq!(
            drop_file(&mut rig, pos2(500.0, 130.0)),
            dropped(6.0, false, Some(1))
        );
        assert_eq!(
            drop_file(&mut rig, pos2(500.0, 190.0)),
            dropped(6.0, false, Some(0))
        );
        assert_eq!(
            drop_file(&mut rig, pos2(500.0, 60.0)),
            dropped(6.0, false, Some(1)),
            "above the top track: on the top track"
        );
        assert_eq!(
            drop_file(&mut rig, pos2(500.0, 300.0)),
            dropped(6.0, false, None),
            "over the audio area: no video track is chosen"
        );
        rig.modifiers = egui::Modifiers::COMMAND;
        assert_eq!(
            drop_file(&mut rig, pos2(500.0, 130.0)),
            dropped(6.0, true, Some(1)),
            "Ctrl still inserts"
        );
        // While the file is dragged, the place it would land is marked.
        rig.modifiers = egui::Modifiers::NONE;
        let primary = egui::PointerButton::Primary;
        rig.move_to(pos2(60.0, 395.0));
        rig.button(pos2(60.0, 395.0), primary, true);
        egui::DragAndDrop::set_payload(&rig.ctx, DraggedMedia(7));
        let marked = |rig: &Rig| {
            rig.view
                .file_over
                .map(|(at, row)| ((at * 1e6).round() / 1e6, row))
        };
        assert_eq!(marked(&rig), None);
        rig.move_to(pos2(500.0, 130.0));
        assert_eq!(
            marked(&rig),
            Some((6.0, Some(1))),
            "the time, and the upper video row"
        );
        rig.move_to(pos2(560.0, 300.0));
        assert_eq!(
            marked(&rig),
            Some((7.0, None)),
            "over the audio area: the time only"
        );
        rig.move_to(pos2(560.0, 10.0));
        assert_eq!(marked(&rig), None, "over the ruler it would not land");
        rig.button(pos2(560.0, 10.0), primary, false);
    }

    #[test]
    fn the_eye_hides_and_shows_a_video_track() {
        let mut rig = Rig::new(2, 1);
        let primary = egui::PointerButton::Primary;
        // The eye of track 1 (y 104 to 158): x 114 to 134, y 110 to 126.
        let eye = pos2(124.0, 118.0);
        assert_eq!(
            click(&mut rig, eye, primary),
            [TimelineEvent::HideTrack {
                track: 1,
                hidden: true
            }]
        );
        rig.video[1].hidden = true;
        // Hidden tracks are drawn (dimmed) and their clips still answer.
        assert_eq!(
            click(&mut rig, pos2(300.0, 130.0), primary),
            [TimelineEvent::Select { clip: 1, add: false }]
        );
        assert_eq!(
            click(&mut rig, eye, primary),
            [TimelineEvent::HideTrack {
                track: 1,
                hidden: false
            }]
        );
        // Audio tracks have mute and solo there instead.
        let events = click(&mut rig, pos2(124.0, 232.0), primary);
        assert!(
            matches!(
                events[..],
                [TimelineEvent::TrackMix {
                    track: 100,
                    solo: true,
                    ..
                }]
            ),
            "{events:?}"
        );
    }

    /// Opens the menu of the track names at `at` and clicks in it `down`
    /// points under the place that was clicked. Returns what that asked for, and
    /// whether it began typing a track's name.
    fn pick_from_names_menu(rig: &mut Rig, at: Pos2, down: f32) -> (Vec<TimelineEvent>, bool) {
        // Starting without a menu, whatever was open.
        rig.frame(vec![key(egui::Key::Escape)]);
        rig.frame(vec![]);
        assert!(!egui::Popup::is_any_open(&rig.ctx), "Escape closes the menu");
        click(rig, at, egui::PointerButton::Secondary);
        rig.frame(vec![]);
        assert!(egui::Popup::is_any_open(&rig.ctx), "the menu opened");
        let events = click(rig, at + vec2(30.0, down), egui::PointerButton::Primary);
        let renames = rig.view.rename.take().is_some();
        (events, renames)
    }

    /// Everything the menu of the track names can ask for when opened at
    /// `at`, top to bottom, and whether one of its lines began a rename.
    fn names_menu(rig: &mut Rig, at: Pos2) -> (Vec<TimelineEvent>, bool) {
        let (mut all, mut renames) = (Vec::new(), false);
        // Near the window's bottom edge the menu opens upward to fit, so
        // the tries begin above the place that was clicked.
        for step in -30..40 {
            let (events, renamed) = pick_from_names_menu(rig, at, 4.0 + step as f32 * 5.0);
            for event in events {
                if !all.contains(&event) {
                    all.push(event);
                }
            }
            renames |= renamed;
        }
        (all, renames)
    }

    #[test]
    fn the_menu_of_the_track_names_adds_hides_renames_and_removes() {
        let mut rig = Rig::new(2, 2);
        // The upper video track has no clips: it can go.
        rig.video[1].clips.clear();
        click(&mut rig, pos2(40.0, 130.0), egui::PointerButton::Secondary);
        assert_eq!(rig.view.names_menu, Some((Half::Video, Some(1))));
        let (events, renames) = names_menu(&mut rig, pos2(40.0, 130.0));
        assert!(renames, "Rename… starts typing the name");
        assert_eq!(
            events,
            [
                TimelineEvent::HideTrack {
                    track: 1,
                    hidden: true
                },
                TimelineEvent::RemoveTrack { track: 1 },
                TimelineEvent::AddTrack { audio: false },
                TimelineEvent::AddTrack { audio: true },
            ]
        );
        // A track with clips stays: the line is there but does nothing.
        let (events, _) = names_menu(&mut rig, pos2(40.0, 190.0));
        assert_eq!(
            events,
            [
                TimelineEvent::HideTrack {
                    track: 0,
                    hidden: true
                },
                TimelineEvent::AddTrack { audio: false },
                TimelineEvent::AddTrack { audio: true },
            ]
        );
        // An audio track: nothing to hide, and its own kind is offered first.
        rig.audio[1].clips.clear();
        let (events, renames) = names_menu(&mut rig, pos2(40.0, 300.0));
        assert!(renames);
        assert_eq!(
            events,
            [
                TimelineEvent::RemoveTrack { track: 101 },
                TimelineEvent::AddTrack { audio: true },
                TimelineEvent::AddTrack { audio: false },
            ]
        );
        // On the empty part of the column: only adding.
        click(&mut rig, pos2(40.0, 60.0), egui::PointerButton::Secondary);
        assert_eq!(rig.view.names_menu, Some((Half::Video, None)));
        let (events, renames) = names_menu(&mut rig, pos2(40.0, 60.0));
        assert!(!renames);
        assert_eq!(
            events,
            [
                TimelineEvent::AddTrack { audio: false },
                TimelineEvent::AddTrack { audio: true },
            ]
        );
    }

    #[test]
    fn the_last_track_of_a_kind_and_a_track_with_clips_stay() {
        let empty = track(1, Vec::new());
        let used = track(2, vec![clip(1, 0.0, 1.0)]);
        assert_eq!(headers::why_it_stays(&empty, 2), None);
        assert!(
            headers::why_it_stays(&empty, 1)
                .unwrap()
                .contains("at least one track")
        );
        assert!(
            headers::why_it_stays(&used, 2)
                .unwrap()
                .contains("still has clips")
        );
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn double_click(rig: &mut Rig, at: Pos2) -> Vec<TimelineEvent> {
        let primary = egui::PointerButton::Primary;
        let mut events = click(rig, at, primary);
        events.extend(rig.button(at, primary, true));
        events.extend(rig.button(at, primary, false));
        events
    }

    #[test]
    fn a_double_click_on_a_tracks_name_renames_it() {
        let mut rig = Rig::new(2, 1);
        assert!(double_click(&mut rig, pos2(40.0, 130.0)).is_empty());
        assert_eq!(rig.view.rename.as_ref().map(|r| r.track), Some(1));
        // The field takes the keyboard with the old name selected: typing replaces it.
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert!(rig.ctx.egui_wants_keyboard_input(), "typing goes to the name");
        assert!(rig.frame(vec![egui::Event::Text("Face cam ".into())]).is_empty());
        let events = rig.frame(vec![key(egui::Key::Enter)]);
        assert_eq!(
            events,
            [TimelineEvent::RenameTrack {
                track: 1,
                name: "Face cam".into()
            }],
            "without the space at its end"
        );
        assert_eq!(rig.view.rename, None);

        // Escape leaves the name as it was.
        double_click(&mut rig, pos2(40.0, 190.0));
        rig.frame(vec![]);
        rig.frame(vec![]);
        rig.frame(vec![egui::Event::Text("Oops".into())]);
        assert!(rig.frame(vec![key(egui::Key::Escape)]).is_empty());
        assert_eq!(rig.view.rename, None);

        // The same name again is no change, and a click elsewhere ends the typing.
        double_click(&mut rig, pos2(40.0, 190.0));
        rig.frame(vec![]);
        rig.frame(vec![]);
        let events = click(&mut rig, OVER_RULER, egui::PointerButton::Primary);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, TimelineEvent::RenameTrack { .. })),
            "{events:?}"
        );
        rig.frame(vec![]);
        assert_eq!(rig.view.rename, None);
    }

    #[test]
    fn the_level_button_shows_a_line_that_drags_the_clips_volume() {
        let mut rig = Rig::new(1, 1);
        let primary = egui::PointerButton::Primary;
        // The audio clip: x 200 to 440, drawn y 221 to 273; its line moves
        // from y 269 (Off) to 225 (+12 dB), 0 dB two thirds up, at 239.7.
        let on_line = pos2(300.0, 240.0);
        // Eleven points down, a quarter of the way.
        let lowered = level::settled_db(level::UNITY - 11.0 / 44.0);
        assert!((-9.0..-7.0).contains(&lowered), "{lowered}");
        // Without the line, a drag there moves the clip.
        let events = drag(&mut rig, on_line, on_line + vec2(60.0, 0.0));
        assert!(
            matches!(events[..], [TimelineEvent::Move { clip: 100, .. }]),
            "{events:?}"
        );

        // The level button, left of M and S, turns it on: view state, no event.
        assert!(click(&mut rig, pos2(76.0, 232.0), primary).is_empty());
        assert!(rig.view.level_line(100));
        assert_eq!(rig.view.level_lines(), [100]);

        // Dragging the line a quarter of the way down, shown while dragging.
        rig.move_to(on_line);
        rig.button(on_line, primary, true);
        let mut started = rig.move_to(on_line + vec2(0.0, 6.0));
        started.extend(rig.move_to(on_line + vec2(30.0, 11.0)));
        assert_eq!(
            started,
            [TimelineEvent::Select {
                clip: 100,
                add: false
            }],
            "it is selected"
        );
        assert_eq!(rig.view.level_being_set(), Some((100, lowered)));
        let events = rig.button(on_line + vec2(30.0, 11.0), primary, false);
        assert_eq!(
            events,
            [TimelineEvent::ClipVolume {
                clip: 100,
                gain_db: lowered
            }],
            "one change, on letting go; the clip did not move"
        );
        assert_eq!(rig.view.level_being_set(), None);

        // The project now says -18 dB: the line is drawn lower, at 258.6.
        rig.audio[0].clips[0].sound.gain = level::gain_of_db(-18.0);

        // Away from the line the clip still moves, and its edges still trim.
        let events = drag(&mut rig, pos2(300.0, 230.0), pos2(360.0, 230.0));
        assert!(
            matches!(events[..], [TimelineEvent::Move { clip: 100, .. }]),
            "{events:?}"
        );
        let events = drag(&mut rig, pos2(437.0, 258.0), pos2(500.0, 258.0));
        assert!(
            matches!(events[..], [TimelineEvent::Trim { clip: 100, .. }]),
            "{events:?}"
        );

        // A drag that comes back where it began changes nothing.
        let events = drag(&mut rig, pos2(300.0, 258.0), pos2(300.0, 258.0 + 8.0));
        assert!(matches!(
            events[..],
            [TimelineEvent::ClipVolume { clip: 100, .. }]
        ));
        rig.move_to(pos2(300.0, 258.0));
        rig.button(pos2(300.0, 258.0), primary, true);
        rig.move_to(pos2(300.0, 250.0));
        rig.move_to(pos2(300.0, 258.0));
        assert!(rig.button(pos2(300.0, 258.0), primary, false).is_empty());

        // A double-click on the line puts it back to 0 dB.
        let mut fresh = Rig::new(1, 1);
        fresh.view.set_level_line(100, true);
        fresh.audio[0].clips[0].sound.gain = level::gain_of_db(-18.0);
        let events = double_click(&mut fresh, pos2(300.0, 258.0));
        assert!(
            events.contains(&TimelineEvent::ClipVolume {
                clip: 100,
                gain_db: 0.0
            }),
            "{events:?}"
        );

        // Off again: the line is gone and the clip moves.
        click(&mut rig, pos2(76.0, 232.0), primary);
        assert!(!rig.view.level_line(100));
        let events = drag(&mut rig, pos2(300.0, 258.0), pos2(360.0, 258.0));
        assert!(
            matches!(events[..], [TimelineEvent::Move { clip: 100, .. }]),
            "{events:?}"
        );
    }

    impl Rig {
        /// Where the rows of `half` are drawn now, in the 1000×400 timeline.
        fn rows_of(&self, half: Half) -> Vec<Rect> {
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
                spans: &[],
                follow: false,
                cache: CacheBar {
                    ranges: &[],
                    about: "",
                },
                keyframe_eases: &["Linear", "Hold", "Smooth"],
                roles: &[],
            };
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
            let layout = self.view.layout(rect, &data);
            self.view.rows(half, &layout, &data).rects().to_vec()
        }
    }

    #[test]
    fn compact_tracks_are_thin_strips_and_the_heights_given_come_back() {
        let mut rig = Rig::new(3, 3);
        rig.view.heights.set(0, VIDEO_TRACK, 120.0);
        rig.view.heights.set(101, AUDIO_TRACK, 90.0);
        let saved = rig.view.heights.to_save();
        rig.view.compact = true;
        rig.frame(Vec::new());
        for half in [Half::Video, Half::Audio] {
            let heights: Vec<f32> = rig.rows_of(half).iter().map(Rect::height).collect();
            assert_eq!(heights, [COMPACT_TRACK; 3], "{half:?}");
        }
        // Alt+wheel and the height commands leave the heights as they were.
        rig.wheel(OVER_VIDEO, vec2(0.0, 60.0), egui::Modifiers::ALT);
        rig.view.step(ViewStep::TallerTracks);
        rig.view.step(ViewStep::UsualTrackHeights);
        rig.frame(Vec::new());
        assert_eq!(rig.view.heights.to_save(), saved, "nothing was stored or lost");
        rig.view.compact = false;
        rig.frame(Vec::new());
        assert_eq!(rig.rows_of(Half::Video)[0].height(), 120.0, "back as they were");
        assert_eq!(rig.rows_of(Half::Audio)[1].height(), 90.0);
    }

    #[test]
    fn twenty_compact_tracks_of_each_kind_show_together() {
        // A timeline 400 tall shows 336 of body: in a taller one (as on a
        // large screen) the same sums show all forty.
        let mut rig = Rig::new(20, 20);
        rig.view.compact = true;
        rig.view.divider = false;
        rig.frame(Vec::new());
        let video = rig.rows_of(Half::Video);
        let audio = rig.rows_of(Half::Audio);
        assert_eq!(
            video[19].top(),
            44.0,
            "the top video track at the top of the body"
        );
        assert_eq!(video[0].bottom(), audio[0].top(), "audio right under V1");
        assert_eq!(audio[19].bottom() - video[19].top(), 40.0 * COMPACT_TRACK);
    }

    #[test]
    fn clips_on_compact_tracks_are_still_picked_moved_and_trimmed() {
        // Three tracks of each kind, the divider in the middle (y = 212):
        // V1 is 194 to 212, V2 176 to 194; A1 218 to 236. Clips run from
        // x = 200 to 440.
        let mut rig = Rig::new(3, 3);
        rig.view.compact = true;
        let primary = egui::PointerButton::Primary;
        let events = click(&mut rig, pos2(300.0, 203.0), primary);
        assert_eq!(events, [TimelineEvent::Select { clip: 0, add: false }]);
        let events = drag(&mut rig, pos2(300.0, 203.0), pos2(360.0, 203.0));
        assert_eq!(events, moved(0, 1.0, None));
        let events = drag(&mut rig, pos2(300.0, 203.0), pos2(300.0, 185.0));
        assert_eq!(events, moved(0, 0.0, Some(1)), "up onto V2");
        let events = drag(&mut rig, pos2(438.0, 227.0), pos2(498.0, 227.0));
        assert_eq!(
            events,
            [TimelineEvent::Trim {
                clip: 100,
                edge: ClipEdge::End,
                to: 6.0
            }]
        );
    }

    #[test]
    fn with_the_divider_off_video_and_audio_tracks_scroll_as_one_list() {
        // Eight tracks of each kind: 432 of video and 464 of audio in a
        // body 342 tall (no divider takes room).
        let mut rig = Rig::new(8, 8);
        rig.view.split = 0.4;
        rig.view.divider = false;
        rig.frame(Vec::new());
        assert_eq!(
            rig.rows_of(Half::Video)[7].top(),
            44.0,
            "the list starts at the top"
        );
        assert_eq!(rig.rows_of(Half::Video)[0].bottom(), 476.0);
        assert_eq!(rig.rows_of(Half::Audio)[0].top(), 476.0, "audio follows V1");

        // The wheel over either kind scrolls both.
        rig.wheel(OVER_VIDEO, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert_eq!(rig.rows_of(Half::Audio)[0].top(), 470.0);
        rig.wheel(OVER_AUDIO, vec2(0.0, -6.0), egui::Modifiers::NONE);
        assert_eq!(rig.rows_of(Half::Audio)[0].top(), 464.0);
        assert_eq!(rig.rows_of(Half::Video)[0].bottom(), 464.0);
        assert_eq!(
            (rig.view.video_scroll, rig.view.audio_scroll),
            (0.0, 0.0),
            "each area's own scroll waits for the divider"
        );
        // To the end: the last audio track at the bottom, no further.
        rig.view.list_scroll = 10_000.0;
        rig.frame(Vec::new());
        assert_eq!(rig.rows_of(Half::Audio)[7].bottom(), 386.0);
        // The last audio clip (A8, 328 to 386) is picked, though every
        // video track has left the view.
        let events = click(&mut rig, pos2(300.0, 357.0), egui::PointerButton::Primary);
        assert_eq!(
            events,
            [TimelineEvent::Select {
                clip: 107,
                add: false
            }]
        );
        // Back at the top, a video clip dragged down lands on the video track under the pointer.
        rig.view.list_scroll = 0.0;
        rig.frame(Vec::new());
        // (V3 is 314 to 368, V2 368 to 422.)
        let events = drag(&mut rig, pos2(300.0, 340.0), pos2(360.0, 380.0));
        assert!(
            matches!(
                events[..],
                [TimelineEvent::Move {
                    clip: 2,
                    track: Some(1),
                    ..
                }]
            ),
            "{events:?}"
        );

        // Nothing to drag where the divider was.
        let events = drag(&mut rig, pos2(640.0, 188.0), pos2(640.0, 260.0));
        assert!(events.is_empty() || matches!(events[..], [TimelineEvent::ClearSelection]));
        assert_eq!(rig.view.split, 0.4);

        // Back on: the divider where it was, each area scrolled as before.
        rig.view.divider = true;
        rig.frame(Vec::new());
        assert_eq!(rig.view.split, 0.4);
        assert!(rig.rows_of(Half::Audio)[0].top() > rig.rows_of(Half::Video)[0].bottom());
    }

    /// Presses at `grab`, drags a second (60 points) to the right in two
    /// steps and lets go: every event on the way.
    fn drag_along(rig: &mut Rig, grab: Pos2) -> Vec<TimelineEvent> {
        let primary = egui::PointerButton::Primary;
        let mut events = rig.move_to(grab);
        events.extend(rig.button(grab, primary, true));
        events.extend(rig.move_to(grab + vec2(30.0, 0.0)));
        events.extend(rig.move_to(grab + vec2(60.0, 0.0)));
        events.extend(rig.button(grab + vec2(60.0, 0.0), primary, false));
        events
    }

    fn seeks(events: &[TimelineEvent]) -> Vec<f64> {
        events
            .iter()
            .filter_map(|e| match e {
                TimelineEvent::Seek(t) => Some((t * 1000.0).round() / 1000.0),
                _ => None,
            })
            .collect()
    }

    fn needle_moves(events: &[TimelineEvent]) -> Vec<f64> {
        events
            .iter()
            .filter_map(|e| match e {
                TimelineEvent::MoveNeedle(t) => Some((t * 1000.0).round() / 1000.0),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn set_to_the_edit_needle_a_click_or_drag_in_the_ruler_moves_the_needle() {
        let primary = egui::PointerButton::Primary;
        let mut rig = Rig::new(1, 1);
        rig.view.click_moves = ClickMoves::EditNeedle;
        rig.needle = Some(5.0);
        // A click at x = 500: six seconds.
        let events = click(&mut rig, pos2(500.0, 10.0), primary);
        assert_eq!(needle_moves(&events), [6.0]);
        assert!(seeks(&events).is_empty(), "the playhead stays: {events:?}");
        // In the labels row too.
        let events = click(&mut rig, pos2(500.0, 36.0), primary);
        assert_eq!(needle_moves(&events), [6.0]);
        // A drag takes the needle along and puts it down once, where it is let go.
        let events = drag_along(&mut rig, pos2(500.0, 10.0));
        assert_eq!(needle_moves(&events), [7.0], "{events:?}");
        assert!(seeks(&events).is_empty());
        // The needle's own handle still drags it.
        let events = drag_along(&mut rig, pos2(441.0, 20.0));
        assert_eq!(needle_moves(&events).len(), 1);
        // Shift+drag still marks a region.
        rig.modifiers = egui::Modifiers::SHIFT;
        let events = drag_along(&mut rig, pos2(500.0, 10.0));
        rig.modifiers = egui::Modifiers::NONE;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::SetRegion(Some(_)))),
            "{events:?}"
        );
        assert!(needle_moves(&events).is_empty());
        // Clips and empty track space are as before.
        let events = click(&mut rig, pos2(300.0, 190.0), primary);
        assert_eq!(events, [TimelineEvent::Select { clip: 0, add: false }]);
        let events = click(&mut rig, pos2(700.0, 190.0), primary);
        assert_eq!(events, [TimelineEvent::ClearSelection]);
    }

    #[test]
    fn the_playhead_is_dragged_by_its_handle_whatever_a_click_moves() {
        // The playhead at 2 s is at x = 260; its handle is at the top of the ruler.
        for moves in [ClickMoves::Playhead, ClickMoves::EditNeedle] {
            let mut rig = Rig::new(1, 1);
            rig.needle = Some(5.0);
            rig.view.click_moves = moves;
            let events = drag_along(&mut rig, pos2(260.0, 6.0));
            assert_eq!(seeks(&events).last(), Some(&3.0), "{moves:?}: {events:?}");
            assert!(needle_moves(&events).is_empty(), "{moves:?}");
            // A click on the handle moves nothing.
            let events = click(&mut rig, pos2(260.0, 6.0), egui::PointerButton::Primary);
            assert!(events.is_empty(), "{moves:?}: {events:?}");
        }
        // With Shift a region can still be begun at the playhead.
        let mut rig = Rig::new(1, 1);
        rig.modifiers = egui::Modifiers::SHIFT;
        let events = drag_along(&mut rig, pos2(260.0, 6.0));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::SetRegion(Some(_)))),
            "{events:?}"
        );
    }

    #[test]
    fn with_no_needle_of_its_own_a_click_moves_the_playhead() {
        // "Edit at the playhead" merges the needle into it: the page passes none.
        let mut rig = Rig::new(1, 1);
        rig.view.click_moves = ClickMoves::EditNeedle;
        let events = click(&mut rig, pos2(500.0, 10.0), egui::PointerButton::Primary);
        assert_eq!(seeks(&events), [6.0]);
        assert!(needle_moves(&events).is_empty());
    }

    #[test]
    fn alt_click_in_the_ruler_moves_the_playhead_whatever_a_click_moves() {
        let primary = egui::PointerButton::Primary;
        for moves in [ClickMoves::Playhead, ClickMoves::EditNeedle] {
            let mut rig = Rig::new(1, 1);
            rig.view.click_moves = moves;
            rig.needle = Some(5.0);
            rig.modifiers = egui::Modifiers::ALT;
            // x = 500 is six seconds, in the ruler and in the labels row.
            for y in [10.0, 36.0] {
                let events = click(&mut rig, pos2(500.0, y), primary);
                assert_eq!(seeks(&events), [6.0], "{moves:?} at y {y}: {events:?}");
                assert!(needle_moves(&events).is_empty(), "{moves:?}: the needle stays");
            }
            // On the playhead's own handle too.
            let events = click(&mut rig, pos2(260.0, 6.0), primary);
            assert_eq!(seeks(&events), [2.0], "{moves:?}: {events:?}");
        }
    }

    #[test]
    fn alt_click_puts_the_playhead_on_the_frame_under_the_pointer() {
        let mut rig = Rig::new(1, 1);
        rig.fps = 25.0;
        rig.modifiers = egui::Modifiers::ALT;
        // x = 507 is 6.1167 s, in frame 152 at 25 frames a second.
        for y in [10.0, 190.0, 300.0] {
            let events = click(&mut rig, pos2(507.0, y), egui::PointerButton::Primary);
            assert_eq!(seeks(&events), [6.08], "at y {y}: {events:?}");
        }
    }

    #[test]
    fn alt_click_on_a_clip_moves_the_playhead_and_keeps_the_selection() {
        let primary = egui::PointerButton::Primary;
        let mut rig = Rig::new(1, 1);
        rig.video[0].clips[0].selected = true;
        rig.modifiers = egui::Modifiers::ALT;
        // On the clip (x = 300 is 2 2/3 s), on the audio clip and on empty space.
        let events = click(&mut rig, pos2(300.0, 190.0), primary);
        assert_eq!(events, [TimelineEvent::Seek(160.0 / 60.0)]);
        let events = click(&mut rig, pos2(300.0, 300.0), primary);
        assert_eq!(events, [TimelineEvent::Seek(160.0 / 60.0)]);
        let events = click(&mut rig, pos2(700.0, 190.0), primary);
        assert_eq!(
            events,
            [TimelineEvent::Seek(560.0 / 60.0)],
            "nothing is unselected"
        );
    }

    #[test]
    fn alt_click_with_any_tool_never_cuts_or_trims() {
        let primary = egui::PointerButton::Primary;
        for tool in [Tool::Blade, Tool::Ripple, Tool::Roll, Tool::Slip, Tool::Slide] {
            let mut rig = Rig::new(1, 1);
            rig.view.tool = tool;
            rig.modifiers = egui::Modifiers::ALT;
            // In the middle of the clip and on its end edge.
            for x in [300.0, 439.0] {
                let events = click(&mut rig, pos2(x, 190.0), primary);
                assert_eq!(seeks(&events).len(), 1, "{tool:?} at {x}: {events:?}");
                assert_eq!(
                    events.len(),
                    1,
                    "{tool:?} at {x}: only the playhead moves: {events:?}"
                );
            }
        }
    }

    #[test]
    fn alt_drag_still_moves_a_clip_and_scrubs_elsewhere() {
        // On a clip: the clip moves (without snapping); the playhead stays.
        let mut rig = Rig::new(1, 1);
        rig.modifiers = egui::Modifiers::ALT;
        let events = drag_along(&mut rig, pos2(300.0, 190.0));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TimelineEvent::Move { clip: 0, by, .. } if (by - 1.0).abs() < 1e-6)),
            "{events:?}"
        );
        assert!(seeks(&events).is_empty(), "{events:?}");
        // On empty track space and in the ruler (also set to the needle):
        // the playhead follows the pointer, on whole frames.
        rig.view.click_moves = ClickMoves::EditNeedle;
        rig.needle = Some(5.0);
        for grab in [pos2(700.0, 190.0), pos2(700.0, 300.0), pos2(700.0, 10.0)] {
            let events = drag_along(&mut rig, grab);
            assert_eq!(seeks(&events).last(), Some(&10.333), "from {grab:?}: {events:?}");
            assert!(needle_moves(&events).is_empty(), "{events:?}");
            assert!(
                !events.iter().any(|e| matches!(e, TimelineEvent::ClearSelection)),
                "{events:?}"
            );
        }
        // Let go, the next plain drag in empty space doesn't scrub.
        rig.modifiers = egui::Modifiers::NONE;
        let events = drag_along(&mut rig, pos2(700.0, 190.0));
        assert!(seeks(&events).is_empty(), "{events:?}");
    }
}
