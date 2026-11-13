//! Turns the open timeline into what the shared timeline widget draws, and
//! keeps the selection.

use crate::import;
use crate::reading::Pace;
use crate::waveforms::Look;
use project::{ClipId, LabelColor, Project, Sequence};
use std::collections::HashMap;
use std::time::Instant;
use ui_kit::timeline::{CacheRange, CacheState, LoadKind, LoadState, Loading, cache_bar};
use ui_kit::{ClipShows, TimelineClip, TimelineMarker, TimelineTrack};

/// What the widget needs for one frame.
pub struct Drawn {
    pub video: Vec<TimelineTrack>,
    pub audio: Vec<TimelineTrack>,
    pub markers: Vec<TimelineMarker>,
    /// For the cache bar: the stretches where something is still being
    /// prepared or waiting for it. See [`CACHE_BAR_ABOUT`].
    pub cache: Vec<CacheRange>,
}

/// What the cache bar stands for on the Edit page today, for its tooltip.
/// Nothing is marked as prepared: pictures for playback are decoded as they
/// are needed, so saying "ready" anywhere would promise smooth playback
/// that nothing guarantees yet. Frames kept in memory, proxies and
/// pre-rendered stretches will be marked here when they are built.
pub const CACHE_BAR_ABOUT: &str = "Right now this follows the reading of waveforms and clip pictures: marked stretches are still being read. Pictures for playback are decoded as they are needed, so nothing is marked as prepared yet.";

/// The stretch of a clip (on the timeline from `start`, `length` seconds
/// long) that is not ready to draw yet, when the share `ready` of it is and
/// its reading is in `state`. Nothing for clips that are done or failed.
fn unready(start: f64, length: f64, ready: f64, state: Option<LoadState>) -> Option<CacheRange> {
    let state = match state? {
        LoadState::Reading { .. } => CacheState::Preparing,
        LoadState::Waiting => CacheState::Waiting,
        LoadState::Failed => return None,
    };
    (ready < 1.0).then_some(CacheRange {
        start: start + length * ready,
        end: start + length,
        state,
    })
}

/// Adds up how much of a track's clips is ready to draw, by clip length,
/// for the line by the track's name.
#[derive(Debug, Default, Clone, Copy)]
struct TrackProgress {
    length: f64,
    ready: f64,
    reading: bool,
    waiting: bool,
}

impl TrackProgress {
    /// A clip `length` seconds long of whose reading the share `ready` is
    /// done, and what that reading is doing (`None`: nothing more to come).
    fn add(&mut self, length: f64, ready: f64, state: Option<LoadState>) {
        let ready = match state {
            Some(LoadState::Reading { .. }) => {
                self.reading = true;
                ready
            }
            Some(LoadState::Waiting) => {
                self.waiting = true;
                ready
            }
            // Ready, or failed: nothing more will happen for this clip.
            Some(LoadState::Failed) | None => 1.0,
        };
        self.length += length;
        self.ready += length * ready;
    }

    /// What the track's line says: how much is ready while anything is
    /// still being read, waiting when everything left is in the queue.
    fn loading(&self, what: LoadKind) -> Option<Loading> {
        let state = if self.reading {
            LoadState::Reading {
                done: (self.ready / self.length.max(1e-9)) as f32,
                seconds_left: None,
            }
        } else if self.waiting {
            LoadState::Waiting
        } else {
            return None;
        };
        Some(Loading { what, state })
    }
}

/// The share of a clip whose waveform can be drawn: the clip shows
/// Points along a ramped clip: where in its file it is (for its pictures
/// and waveform) and how fast it plays (the line drawn on it). Nothing for
/// a clip at one speed. A few dozen points, worked out with floats: a
/// drawing, not an edit.
fn ramp_drawing(c: &project::Clip) -> (Option<ui_kit::SourceMap>, Option<ui_kit::SpeedCurve>) {
    if c.ramp.is_empty() {
        return (None, None);
    }
    const POINTS: usize = 64;
    let length = c.duration.as_secs_f64();
    let start = c.start.as_secs_f64();
    let shares = (0..=POINTS).map(|i| i as f64 / POINTS as f64);
    let map: Vec<(f64, f64)> = shares
        .clone()
        .map(|share| {
            let into = share * length;
            (into, c.source_at_f64(start + into))
        })
        .collect();
    let curve: Vec<(f32, f32)> = shares
        .map(|share| (share as f32, c.ramp.speed_at_f64(share * length) as f32))
        .collect();
    (
        Some(ui_kit::SourceMap(map.into())),
        Some(ui_kit::SpeedCurve(curve.into())),
    )
}

/// `source_len` seconds of its file from `source_in`, and `read_secs` of the
/// file's sound have been read.
fn share_read(read_secs: f64, source_in: f64, source_len: f64) -> f64 {
    ((read_secs - source_in) / source_len.max(1e-9)).clamp(0.0, 1.0)
}

/// Tells each track's time left from how fast its share ready has grown.
#[derive(Debug, Default)]
pub struct TrackPaces {
    paces: HashMap<u64, Pace>,
}

impl TrackPaces {
    /// Fills in the time left of every track that is being read.
    pub fn time(&mut self, tracks: &mut [TimelineTrack], now: Instant) {
        for track in tracks {
            match &mut track.loading {
                Some(Loading {
                    state: state @ LoadState::Reading { .. },
                    ..
                }) => {
                    let LoadState::Reading { done, .. } = *state else {
                        continue;
                    };
                    *state = self.paces.entry(track.id).or_default().reading(done, now);
                }
                _ => {
                    self.paces.remove(&track.id);
                }
            }
        }
    }
}

/// Builds the widget's data. Clips in `selected`, and clips linked to them,
/// show as selected; clips of `missing` media show as missing. `waveform`
/// gives what is known of a sound stream's peaks, `pictures` a video
/// stream's pictures when ready and otherwise how their reading is going.
pub fn drawn(
    project: &Project,
    seq: &Sequence,
    selected: &[ClipId],
    missing: &[project::MediaId],
    mut waveform: impl FnMut(&project::MediaRef, u32) -> Look,
    mut pictures: impl FnMut(&project::MediaRef, u32) -> (Option<ui_kit::ClipPictures>, Option<LoadState>),
) -> Drawn {
    let links: Vec<_> = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| selected.contains(&c.id))
        .filter_map(|c| c.link)
        .collect();
    let mut cache = Vec::new();
    let rate = seq.settings.frame_rate;
    let mut track = |t: &project::Track| {
        let what = match t.kind {
            project::TrackKind::Audio => LoadKind::Sound,
            project::TrackKind::Video => LoadKind::Pictures,
        };
        let mut progress = TrackProgress::default();
        let clips = t
            .clips
            .iter()
            .map(|c| {
                let media = project.media.get(&c.media).filter(|_| c.is_file());
                let nested = c.nested().map(|id| project.sequence(id));
                let is_missing = match c.kind {
                    project::ClipKind::File => media.is_none_or(|m| missing.contains(&m.id)),
                    project::ClipKind::Timeline(_) => nested.is_some_and(|s| s.is_none()),
                    project::ClipKind::Adjustment => false,
                };
                let (source_in, speed) = (c.source_in.as_secs_f64(), c.speed.as_f64());
                let length = c.duration.as_secs_f64();
                // What there is to draw inside the clip, how much of its
                // reading is done, and how the reading of the rest goes.
                let (mut peaks, mut strip, mut ready, mut state) = (None, None, 1.0, None);
                match (t.kind, media) {
                    (project::TrackKind::Audio, Some(m)) => {
                        let look = waveform(m, c.stream);
                        ready = share_read(look.read_secs, source_in, c.source_len().as_secs_f64());
                        // A clip whose own part of the file has been read is done.
                        state = look.state.filter(|s| ready < 1.0 || *s == LoadState::Failed);
                        peaks = look.peaks;
                    }
                    (project::TrackKind::Video, Some(m)) if !is_missing => {
                        (strip, state) = pictures(m, c.stream);
                        // Pictures show when all of them are read; until
                        // then the share read is how far the track is.
                        ready = match state {
                            Some(LoadState::Reading { done, .. }) => f64::from(done),
                            Some(_) => 0.0,
                            None => 1.0,
                        };
                    }
                    _ => {}
                }
                // A missing file's clip already says so.
                let state = state.filter(|s| !(is_missing && *s == LoadState::Failed));
                progress.add(length, ready, state);
                // A waveform shows as far as it is read; pictures only once all are.
                let shown = match t.kind {
                    project::TrackKind::Audio => ready,
                    project::TrackKind::Video => 0.0,
                };
                cache.extend(unready(c.start.as_secs_f64(), length, shown, state));
                let name = match (c.kind, nested.flatten()) {
                    (project::ClipKind::Adjustment, _) => project::timelines::ADJUSTMENT_NAME.to_owned(),
                    (project::ClipKind::Timeline(_), Some(seq)) => seq.name.clone(),
                    _ => media
                        .and_then(|m| m.path.file_name())
                        .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned()),
                };
                let name = if c.ramp.is_empty() && c.speed == project::Speed::NORMAL {
                    name
                } else {
                    format!("{name} · {}", c.speed_summary())
                };
                let (map, speed_curve) = ramp_drawing(c);
                TimelineClip {
                    id: c.id.0,
                    start: c.start.as_secs_f64(),
                    duration: length,
                    info: match (c.kind, nested.flatten()) {
                        (project::ClipKind::Adjustment, _) => {
                            "An adjustment layer: its colour and opacity change everything below it".into()
                        }
                        (project::ClipKind::Timeline(_), Some(seq)) => format!(
                            "The timeline {} · {} · double-click to open it",
                            seq.name,
                            import::settings_text(&seq.settings)
                        ),
                        _ => media.map_or_else(String::new, import::summary),
                    },
                    name,
                    selected: selected.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)),
                    missing: is_missing,
                    link: c.link.map(|l| l.0),
                    waveform: peaks.map(|p| ui_kit::ClipWaveform {
                        peaks: p.values,
                        per_second: p.per_second as f32,
                        source_in,
                        speed,
                        map: map.clone(),
                    }),
                    pictures: strip.map(|p| ui_kit::ClipPictures {
                        source_in,
                        speed,
                        map,
                        ..p
                    }),
                    loading: state.map(|state| Loading { what, state }),
                    sound: drawn_sound(&c.sound),
                    keyframes: match (t.kind, selected.contains(&c.id)) {
                        (project::TrackKind::Video, true) => keyframe_marks(c, rate),
                        _ => Vec::new(),
                    },
                    speed_curve,
                    shows: match c.kind {
                        project::ClipKind::File => ClipShows::File,
                        project::ClipKind::Timeline(_) => ClipShows::Timeline,
                        project::ClipKind::Adjustment => ClipShows::Adjustment,
                    },
                    line: None,
                }
            })
            .collect();
        TimelineTrack {
            id: t.id.0,
            name: t.name.clone(),
            muted: t.muted,
            solo: t.solo,
            hidden: t.hidden,
            gain_db: t.gain_db,
            clips,
            loading: progress.loading(what),
            line: None,
            role: None,
        }
    };
    let (video, audio) = (
        seq.video_tracks.iter().map(&mut track).collect(),
        seq.audio_tracks.iter().map(&mut track).collect(),
    );
    Drawn {
        video,
        audio,
        cache: cache_bar::flatten(&cache),
        markers: seq
            .labels
            .iter()
            .map(|l| TimelineMarker {
                id: l.id.0,
                at: l.at.as_secs_f64(),
                name: l.name.clone(),
                colour: label_colour(l.color),
            })
            .collect(),
    }
}

/// Draws `sounds` on their clips instead of what the project says: sound
/// being set and not yet an undo step.
pub fn show_sounds(drawn: &mut Drawn, sounds: &[(ClipId, project::Sound)]) {
    if sounds.is_empty() {
        return;
    }
    for clip in drawn.audio.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        if let Some((_, sound)) = sounds.iter().find(|(id, _)| id.0 == clip.id) {
            clip.sound = drawn_sound(sound);
        }
    }
}

/// A clip's sound as the timeline draws it.
fn drawn_sound(sound: &project::Sound) -> ui_kit::ClipSound {
    ui_kit::ClipSound {
        gain: engine::mix::volume(sound.gain_mdb),
        fade_in: sound.fade_in.as_secs_f64(),
        fade_out: sound.fade_out.as_secs_f64(),
    }
}

/// The rough cut view: each video clip shows its sound inside it (the first
/// sound clip linked to it), and sound clips that belong to a video clip
/// leave the audio area. Tracks left empty there are not shown.
pub fn fold_sound(drawn: &mut Drawn) {
    let video_links: Vec<u64> = drawn
        .video
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| c.link)
        .collect();
    for clip in drawn.video.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        let Some(link) = clip.link else { continue };
        let sound = drawn
            .audio
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|a| a.link == Some(link));
        clip.waveform = sound.and_then(|a| a.waveform.clone());
        clip.sound = sound.map_or_else(Default::default, |a| a.sound);
        // Its pictures' reading shows first; then its sound's.
        if clip.loading.is_none() {
            clip.loading = sound.and_then(|a| a.loading);
        }
    }
    for track in &mut drawn.audio {
        track
            .clips
            .retain(|c| c.link.is_none_or(|l| !video_links.contains(&l)));
    }
    drawn.audio.retain(|t| !t.clips.is_empty());
}

/// The colour each timeline-label colour is drawn in.
pub fn label_colour(c: LabelColor) -> egui::Color32 {
    use egui::Color32 as C;
    match c {
        LabelColor::Blue => C::from_rgb(0x4F, 0x8C, 0xE0),
        LabelColor::Green => C::from_rgb(0x5D, 0xBB, 0x7A),
        LabelColor::Pink => C::from_rgb(0xE0, 0x6F, 0xB4),
        LabelColor::Yellow => C::from_rgb(0xE6, 0xC3, 0x5C),
        LabelColor::Orange => C::from_rgb(0xE8, 0x8A, 0x3C),
        LabelColor::Purple => C::from_rgb(0x9A, 0x6C, 0xD8),
        LabelColor::Red => C::from_rgb(0xE5, 0x48, 0x4D),
        LabelColor::Grey => C::from_rgb(0x8A, 0x8F, 0x96),
    }
}

/// Where a clip's look has keyframes: a mark at the start of each
/// timeline frame that holds one, inside the clip, saying which numbers are
/// keyed there and how they go on to the next.
pub fn keyframe_marks(clip: &project::Clip, rate: project::Rate) -> Vec<ui_kit::KeyframeMark> {
    use project::{Ease, LookField};
    clip.keyframe_frames(&LookField::ALL, rate)
        .into_iter()
        .map(|frame| {
            let at = rate.frame_start(frame);
            let near = clip.frame_in_file(at, rate);
            let fields: Vec<LookField> = clip.keyframes.fields_in(near).collect();
            // In the order the inspector shows them, each group once.
            let keyed = LookField::GROUPS
                .iter()
                .filter(|group| group.iter().any(|field| fields.contains(field)))
                .filter_map(|group| group.first().map(|field| field.name()))
                .collect();
            let mut eases = fields
                .iter()
                .flat_map(|field| clip.keyframes.of(*field))
                .filter(|key| near.0 <= key.at && key.at < near.1)
                .map(|key| key.ease);
            let first = eases.next();
            let ease = first
                .filter(|ease| eases.all(|other| other == *ease))
                .and_then(|ease| Ease::ALL.iter().position(|e| *e == ease));
            ui_kit::KeyframeMark {
                at: at.as_secs_f64(),
                keyed,
                ease,
            }
        })
        .collect()
}

/// The ways a value goes on from a keyframe, as keyframe marks' menus
/// offer them (in `Ease::ALL` order).
pub const EASES: [&str; 3] = [
    project::Ease::ALL[0].label(),
    project::Ease::ALL[1].label(),
    project::Ease::ALL[2].label(),
];

/// The undo step for keyframe marks of `clip` (frame starts, in seconds on
/// the timeline) that were dragged or picked something for in their menu.
/// `None` for a drag of less than a frame or an ease that doesn't exist.
pub fn keyframes_command(
    seq: &Sequence,
    clip: u64,
    marks: &[f64],
    change: ui_kit::timeline::MarksChange,
) -> Option<project::Command> {
    use project::animate::KeyframesChange;
    use ui_kit::timeline::MarksChange;
    let rate = seq.settings.frame_rate;
    let time = |seconds: f64| {
        let step = rate.snap(crate::editor::seconds(seconds.abs()));
        if seconds < 0.0 {
            project::Time::ZERO - step
        } else {
            step
        }
    };
    let change = match change {
        MarksChange::Move(by) => {
            KeyframesChange::Move(Some(time(by)).filter(|by| *by != project::Time::ZERO)?)
        }
        MarksChange::Remove => KeyframesChange::Remove,
        MarksChange::Ease(i) => KeyframesChange::Ease(*project::Ease::ALL.get(i)?),
    };
    Some(project::Command::ChangeKeyframes {
        sequence: seq.id,
        clip: ClipId(clip),
        frames: marks.iter().map(|at| time(*at)).collect(),
        change,
    })
}

/// `project` with the keyframe marks being dragged on `timeline` where
/// they are now, for the viewer to show; `None` when none are dragged (or
/// they can't go there).
pub fn keyframes_moved(timeline: &ui_kit::TimelineView, project: &Project) -> Option<Project> {
    let (clip, marks, by) = timeline.keyframes_being_moved()?;
    let command = keyframes_command(
        project.active()?,
        clip,
        marks,
        ui_kit::timeline::MarksChange::Move(by),
    )?;
    command.apply(project).ok()
}

/// Clicking a clip: alone, or added to (or taken out of) the selection.
pub fn select(selected: &mut Vec<ClipId>, clip: ClipId, add: bool) {
    if !add {
        selected.clear();
        selected.push(clip);
    } else if let Some(i) = selected.iter().position(|c| *c == clip) {
        selected.remove(i);
    } else {
        selected.push(clip);
    }
}

/// What the status line says after a track called `name` was added. In the
/// rough cut view an audio track only shows once it has sound of its own,
/// so that is said too.
pub fn added_track_says(name: &str, kind: project::TrackKind, rough_cut: bool) -> String {
    match kind {
        project::TrackKind::Video => format!("{name} added on top: its clips cover the tracks under it"),
        project::TrackKind::Audio if rough_cut => {
            format!("{name} added. The rough cut view shows it once it has sound that belongs to no picture")
        }
        project::TrackKind::Audio => format!("{name} added under the other audio tracks"),
    }
}

/// A track's name for the status line, saying so when the track was only
/// just made (it is not among `before`'s).
fn named(track: &project::Track, before: &Sequence) -> String {
    if before.track(track.id).is_some() {
        track.name.clone()
    } else {
        format!("{} (a new track)", track.name)
    }
}

/// "A3", "Mic and A3" or "Game, Mic and A3".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
    }
}

/// What the status line says about sound that went somewhere else than
/// expected, on the audio tracks named in `tracks`. Nothing when there is none.
fn sound_says(tracks: &[String]) -> String {
    if tracks.is_empty() {
        return String::new();
    }
    format!(
        "Its sound is on {}: other sound was in the way, and nothing was cut",
        listed(tracks)
    )
}

/// What the status line says after `clip` was dragged to another track or
/// past the last one (`before` and `after` are the timeline around the
/// move): the track that was made for it, and where its linked sound went
/// when that had to leave its track. Empty when nothing needs saying.
pub fn moved_says(before: &Sequence, after: &Sequence, clip: ClipId) -> String {
    let track_of = |seq: &Sequence, id: ClipId| {
        seq.tracks()
            .find(|t| t.clips.iter().any(|c| c.id == id))
            .map(|t| t.id)
    };
    let mut says = Vec::new();
    if let Some(track) = track_of(after, clip).and_then(|id| after.track(id))
        && before.track(track.id).is_none()
    {
        says.push(format!("{} was made for it", track.name));
    }
    let link = after
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .and_then(|c| c.link);
    let moved: Vec<String> = after
        .audio_tracks
        .iter()
        .filter(|t| {
            t.clips.iter().any(|c| {
                c.id != clip && link.is_some() && c.link == link && track_of(before, c.id) != Some(t.id)
            })
        })
        .map(|t| named(t, before))
        .collect();
    if !moved.is_empty() {
        says.push(sound_says(&moved));
    }
    says.join(". ")
}

/// What the status line says after the file `media` was laid over the
/// timeline at `at`: where its sound went, when that is not the first audio
/// tracks. Empty when it landed as an import would put it.
pub fn laid_says(before: &Sequence, after: &Sequence, media: project::MediaId, at: project::Time) -> String {
    let was_there = |id: ClipId| before.tracks().any(|t| t.clips.iter().any(|c| c.id == id));
    // Its sound clips, top to bottom: one per stream, in the file's order.
    let landed: Vec<&project::Track> = after
        .audio_tracks
        .iter()
        .filter(|t| {
            t.clips
                .iter()
                .any(|c| c.media == media && c.start == at && !was_there(c.id))
        })
        .collect();
    let elsewhere: Vec<String> = landed
        .iter()
        .enumerate()
        .filter(|(i, track)| after.audio_tracks.get(*i).map(|t| t.id) != Some(track.id))
        .map(|(_, track)| named(track, before))
        .collect();
    sound_says(&elsewhere)
}

/// The command for what was asked of a track in its header (remove, hide
/// or show, rename); `None` for anything else.
pub fn track_command(
    sequence: project::SequenceId,
    event: ui_kit::TimelineEvent,
) -> Option<project::Command> {
    use project::{Command, TrackId};
    use ui_kit::TimelineEvent as E;
    Some(match event {
        E::RemoveTrack { track } => Command::RemoveTrack {
            sequence,
            track: TrackId(track),
        },
        E::HideTrack { track, hidden } => Command::SetTrackHidden {
            sequence,
            track: TrackId(track),
            hidden,
        },
        E::RenameTrack { track, name } => Command::RenameTrack {
            sequence,
            track: TrackId(track),
            name,
        },
        _ => return None,
    })
}

/// A volume in decibels from the timeline (its level line) in the
/// project's thousandths, the bottom of the range being silence.
pub fn gain_mdb(db: f32) -> i32 {
    if db <= ui_kit::timeline::level::SILENT_DB {
        project::Sound::SILENT
    } else {
        ((db * 1000.0).round() as i32).clamp(project::Sound::SILENT, project::Sound::LOUDEST)
    }
}

/// The undo step for a clip's level line let go at `gain_db`: that clip's
/// volume only, its pan and fades as they are. `None` when the clip is gone
/// or its volume is already that.
pub fn clip_volume_command(seq: &Sequence, clip: ClipId, gain_db: f32) -> Option<project::Command> {
    let found = seq
        .audio_tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| c.id == clip)?;
    let sound = project::Sound {
        gain_mdb: gain_mdb(gain_db),
        ..found.sound
    };
    (sound != found.sound).then(|| project::Command::SetSound {
        sequence: seq.id,
        sounds: vec![(clip, sound)],
    })
}

/// The ids of every track of the timeline, for forgetting the heights of
/// tracks that were removed.
pub fn track_ids(seq: &Sequence) -> Vec<u64> {
    seq.tracks().map(|t| t.id.0).collect()
}

/// The first clip of the highest video track that has any: the picture on
/// top at the start of the timeline. What `--select-first-clip` selects.
pub fn first_clip(seq: &Sequence) -> Option<ClipId> {
    seq.video_tracks
        .iter()
        .rev()
        .find_map(|track| track.clips.iter().min_by_key(|clip| clip.start))
        .map(|clip| clip.id)
}

/// Every clip of the timeline, on every track: what Select all selects.
pub fn all_clips(seq: &Sequence) -> Vec<ClipId> {
    seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect()
}

/// Drops clips that no longer exist (after a delete or an undo).
pub fn keep_existing(selected: &mut Vec<ClipId>, seq: &Sequence) {
    selected.retain(|id| seq.tracks().any(|t| t.clips.iter().any(|c| c.id == *id)));
}

#[cfg(test)]
mod tests {
    #[test]
    fn keyframe_marks_are_the_frames_that_hold_one_inside_the_clip() {
        let project = crate::inspector::tests::project_with_a_clip();
        let sequence = project.active_sequence;
        let clip = project.active().unwrap().video_tracks[0].clips[0].clone();
        let mut keyed = project;
        for (seconds, fields) in [
            (1, vec![project::LookField::Opacity]),
            (3, vec![project::LookField::Scale]),
            (30, vec![project::LookField::Tint]),
        ] {
            keyed = project::Command::Keyframe {
                sequence,
                clip: clip.id,
                at: project::Time::seconds(seconds),
                fields,
                change: project::animate::KeyframeChange::Add,
            }
            .apply(&keyed)
            .unwrap();
        }
        let clip = &keyed.active().unwrap().video_tracks[0].clips[0];
        let marks = keyframe_marks(clip, project::Rate::FPS_60);
        let at: Vec<f64> = marks.iter().map(|m| m.at).collect();
        // The one asked for at 30 s went on the clip's last frame (10 s long).
        assert_eq!(at, vec![1.0, 3.0, 599.0 / 60.0]);
        assert_eq!(marks[0].keyed, ["opacity"]);
        assert_eq!(marks[1].keyed, ["size"]);
        assert_eq!(marks[1].ease, Some(0), "linear");
    }

    #[test]
    fn a_mark_says_every_number_keyed_on_its_frame_once() {
        let project = crate::inspector::tests::project_with_a_clip();
        let sequence = project.active_sequence;
        let clip = project.active().unwrap().video_tracks[0].clips[0].id;
        let mut keyed = project::Command::Keyframe {
            sequence,
            clip,
            at: project::Time::seconds(2),
            fields: vec![
                project::LookField::Opacity,
                project::LookField::X,
                project::LookField::Y,
            ],
            change: project::animate::KeyframeChange::Add,
        }
        .apply(&project)
        .unwrap();
        keyed = project::Command::Keyframe {
            sequence,
            clip,
            at: project::Time::seconds(2),
            fields: vec![project::LookField::Opacity],
            change: project::animate::KeyframeChange::Ease(project::Ease::Hold),
        }
        .apply(&keyed)
        .unwrap();
        let clip = &keyed.active().unwrap().video_tracks[0].clips[0];
        let marks = keyframe_marks(clip, project::Rate::FPS_60);
        assert_eq!(marks[0].keyed, ["position", "opacity"]);
        assert_eq!(marks[0].ease, None, "position goes on evenly, opacity holds");
    }

    #[test]
    fn a_drag_of_marks_is_one_step_in_whole_frames() {
        use ui_kit::timeline::MarksChange;
        let project = crate::inspector::tests::project_with_a_clip();
        let seq = project.active().unwrap();
        let clip = seq.video_tracks[0].clips[0].id;
        let rate = seq.settings.frame_rate;
        let frame = rate.frame_start(1).as_secs_f64();
        let command = keyframes_command(seq, clip.0, &[1.0], MarksChange::Move(-3.1 * frame)).unwrap();
        let project::Command::ChangeKeyframes { frames, change, .. } = command else {
            panic!("{command:?}");
        };
        assert_eq!(frames, [project::Time::seconds(1)]);
        assert_eq!(
            change,
            project::animate::KeyframesChange::Move(project::Time::ZERO - rate.frame_start(3))
        );
        assert!(keyframes_command(seq, clip.0, &[1.0], MarksChange::Move(0.2 * frame)).is_none());
        assert!(keyframes_command(seq, clip.0, &[1.0], MarksChange::Ease(9)).is_none());
    }

    use super::*;

    #[test]
    fn selecting_alone_adding_and_taking_out() {
        let mut s = Vec::new();
        select(&mut s, ClipId(1), false);
        select(&mut s, ClipId(2), true);
        assert_eq!(s, [ClipId(1), ClipId(2)]);
        select(&mut s, ClipId(1), true);
        assert_eq!(s, [ClipId(2)]);
        select(&mut s, ClipId(3), false);
        assert_eq!(s, [ClipId(3)]);
    }

    fn clip(id: u64, link: Option<u64>, peaks: Option<f32>) -> ui_kit::TimelineClip {
        ui_kit::TimelineClip {
            id,
            start: 0.0,
            duration: 1.0,
            name: String::new(),
            info: String::new(),
            selected: false,
            missing: false,
            link,
            waveform: peaks.map(|p| ui_kit::ClipWaveform {
                peaks: vec![p].into(),
                per_second: 1.0,
                source_in: 0.0,
                speed: 1.0,
                map: None,
            }),
            pictures: None,
            loading: None,
            sound: Default::default(),
            keyframes: Default::default(),
            speed_curve: None,
            shows: Default::default(),
            line: None,
        }
    }

    fn track(id: u64, clips: Vec<ui_kit::TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: String::new(),
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

    const READING: LoadState = LoadState::Reading {
        done: 0.5,
        seconds_left: None,
    };

    #[test]
    fn a_clip_is_ready_once_its_own_part_of_the_file_is_read() {
        // A clip showing seconds 10 to 20 of its file.
        assert_eq!(share_read(0.0, 10.0, 10.0), 0.0);
        assert_eq!(share_read(15.0, 10.0, 10.0), 0.5);
        assert_eq!(share_read(25.0, 10.0, 10.0), 1.0);
        assert_eq!(share_read(f64::INFINITY, 10.0, 10.0), 1.0, "all read");
    }

    #[test]
    fn the_cache_bar_marks_what_is_not_ready_yet() {
        // A clip from 10 s to 30 s whose first quarter is read.
        assert_eq!(
            unready(10.0, 20.0, 0.25, Some(READING)),
            Some(CacheRange {
                start: 15.0,
                end: 30.0,
                state: CacheState::Preparing
            })
        );
        assert_eq!(
            unready(10.0, 20.0, 0.0, Some(LoadState::Waiting)).map(|r| (r.start, r.state)),
            Some((10.0, CacheState::Waiting))
        );
        assert_eq!(unready(10.0, 20.0, 1.0, None), None, "ready: nothing to mark");
        assert_eq!(
            unready(10.0, 20.0, 0.0, Some(LoadState::Failed)),
            None,
            "a failed reading is not on its way"
        );
    }

    #[test]
    fn a_track_tells_how_much_of_its_clips_is_ready() {
        let mut p = TrackProgress::default();
        assert_eq!(
            p.loading(LoadKind::Sound),
            None,
            "an empty track has nothing to read"
        );
        // 30 s ready, 10 s half read, 20 s waiting, 40 s that failed.
        p.add(30.0, 1.0, None);
        p.add(10.0, 0.5, Some(READING));
        p.add(20.0, 0.0, Some(LoadState::Waiting));
        p.add(40.0, 0.0, Some(LoadState::Failed));
        let Some(Loading {
            what: LoadKind::Sound,
            state: LoadState::Reading { done, .. },
        }) = p.loading(LoadKind::Sound)
        else {
            panic!("reading: {:?}", p.loading(LoadKind::Sound));
        };
        assert!((done - 0.75).abs() < 1e-6, "75 of 100 seconds: {done}");

        let mut queued = TrackProgress::default();
        queued.add(10.0, 0.0, Some(LoadState::Waiting));
        assert_eq!(
            queued.loading(LoadKind::Pictures).map(|l| l.state),
            Some(LoadState::Waiting)
        );
        let mut done = TrackProgress::default();
        done.add(10.0, 1.0, None);
        done.add(10.0, 0.0, Some(LoadState::Failed));
        assert_eq!(done.loading(LoadKind::Sound), None, "nothing more to come");
    }

    #[test]
    fn a_tracks_time_left_comes_from_its_pace() {
        let reading = |done| {
            Some(Loading {
                what: LoadKind::Sound,
                state: LoadState::Reading {
                    done,
                    seconds_left: None,
                },
            })
        };
        let mut tracks = [track(1, Vec::new()), track(2, Vec::new())];
        let mut paces = TrackPaces::default();
        let start = Instant::now();
        tracks[0].loading = reading(0.25);
        paces.time(&mut tracks, start);
        tracks[0].loading = reading(0.5);
        paces.time(&mut tracks, start + std::time::Duration::from_secs(4));
        assert_eq!(
            tracks[0].loading.map(|l| l.state),
            Some(LoadState::Reading {
                done: 0.5,
                seconds_left: Some(8.0)
            }),
            "a quarter in 4 s: the half that is left takes 8 s"
        );
        assert_eq!(tracks[1].loading, None);
        // Done: the pace is forgotten, so a later reading starts afresh.
        tracks[0].loading = None;
        paces.time(&mut tracks, start + std::time::Duration::from_secs(5));
        assert!(paces.paces.is_empty());
    }

    #[test]
    fn every_track_is_drawn_and_a_hidden_one_says_so() {
        use project::{Command, SequenceSettings, TrackKind};
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let p = Command::AddTrack {
            sequence,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let v2 = p.active().unwrap().video_tracks[1].id;
        let p = Command::SetTrackHidden {
            sequence,
            track: v2,
            hidden: true,
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let d = drawn(
            &p,
            seq,
            &[],
            &[],
            |_, _| Look {
                peaks: None,
                read_secs: 0.0,
                state: None,
            },
            |_, _| (None, None),
        );
        let video: Vec<(&str, bool)> = d.video.iter().map(|t| (t.name.as_str(), t.hidden)).collect();
        assert_eq!(video, [("V1", false), ("V2", true)], "the lowest layer first");
        assert_eq!(d.audio.len(), 1);
        assert_eq!(
            track_ids(seq),
            [seq.video_tracks[0].id.0, v2.0, seq.audio_tracks[0].id.0]
        );
    }

    #[test]
    fn the_first_clip_is_the_earliest_on_the_highest_track_with_any() {
        use project::{Command, SequenceSettings, TrackKind};
        let p = crate::inspector::tests::project_with_a_clip();
        let sequence = p.active_sequence;
        let on_v1 = p.active().unwrap().video_tracks[0].clips[0].clone();
        assert_eq!(first_clip(p.active().unwrap()), Some(on_v1.id));
        // An empty track on top changes nothing.
        let p = Command::AddTrack {
            sequence,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_clip(p.active().unwrap()), Some(on_v1.id));
        // Two clips on it, the later one placed first: the earlier is the first.
        let v2 = p.active().unwrap().video_tracks[1].id;
        let mut p = p;
        for start in [6, 2] {
            let mut clip = on_v1.clone();
            clip.id = p.allocate_id();
            clip.start = project::Time::seconds(start);
            clip.duration = project::Time::seconds(2);
            clip.link = None;
            p = Command::PlaceClip {
                sequence,
                track: v2,
                clip,
            }
            .apply(&p)
            .unwrap();
        }
        let seq = p.active().unwrap();
        let first = first_clip(seq).unwrap();
        let found = seq.video_tracks[1].clips.iter().find(|c| c.id == first);
        assert_eq!(found.map(|c| c.start), Some(project::Time::seconds(2)));
        // Nothing on the timeline, nothing to select.
        let empty = Project::new("Test", SequenceSettings::GAME_CLIP);
        assert_eq!(first_clip(empty.active().unwrap()), None);
    }

    #[test]
    fn select_all_takes_every_clip_on_every_track() {
        let p = crate::inspector::tests::project_with_a_clip();
        let seq = p.active().unwrap();
        let all = all_clips(seq);
        let count: usize = seq.tracks().map(|t| t.clips.len()).sum();
        assert_eq!(all.len(), count);
        assert!(!all.is_empty());
        let empty = Project::new("Test", project::SequenceSettings::GAME_CLIP);
        assert!(all_clips(empty.active().unwrap()).is_empty());
    }

    #[test]
    fn what_is_asked_of_a_track_becomes_its_command() {
        use project::{Command, SequenceSettings, TrackKind};
        use ui_kit::TimelineEvent as E;
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let add = Command::AddTrack {
            sequence,
            kind: TrackKind::Video,
            name: "V2".into(),
        };
        let p = add.apply(&p).unwrap();
        let v2 = p.active().unwrap().video_tracks[1].id.0;
        let ask = |p: &Project, event: E| track_command(sequence, event).unwrap().apply(p);

        let p = ask(
            &p,
            E::RenameTrack {
                track: v2,
                name: "Face cam".into(),
            },
        )
        .unwrap();
        let p = ask(
            &p,
            E::HideTrack {
                track: v2,
                hidden: true,
            },
        )
        .unwrap();
        let top = &p.active().unwrap().video_tracks[1];
        assert_eq!((top.name.as_str(), top.hidden), ("Face cam", true));
        let p = ask(&p, E::RemoveTrack { track: v2 }).unwrap();
        assert_eq!(p.active().unwrap().video_tracks.len(), 1);
        let v1 = p.active().unwrap().video_tracks[0].id.0;
        let err = ask(&p, E::RemoveTrack { track: v1 }).unwrap_err();
        assert!(err.what().contains("the only video track"), "{}", err.what());
        assert_eq!(track_command(sequence, E::ClearSelection), None);
    }

    /// A recording with a picture and `sound` sound streams, `seconds` long.
    fn recording(sound: u32, seconds: i64) -> project::MediaRef {
        project::MediaRef {
            id: project::MediaId(0),
            path: "match.mkv".into(),
            fingerprint: project::Fingerprint {
                size: 1,
                sample_hash: "y".into(),
            },
            kind: project::MediaKind::Video,
            duration: project::Time::seconds(seconds),
            info: project::MediaInfo {
                container: "matroska,webm".into(),
                video: Some(project::VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: project::Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: (1..=sound)
                    .map(|index| project::AudioStream {
                        index,
                        codec: "aac".into(),
                        channels: 2,
                        sample_rate: 48_000,
                        name: format!("Stream {index}"),
                    })
                    .collect(),
                stated_length: None,
            },
        }
    }

    #[test]
    fn the_status_line_says_where_sound_went_when_it_had_to_find_room() {
        use project::{Command, SequenceSettings, Time, TrackKind};
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let import = |p: &Project, media| Command::Import { sequence, media }.apply(p).unwrap();
        // 0 to 30 s with two sound streams, then 30 to 40 s with one; V2 on top.
        let p = import(&import(&p, recording(2, 30)), recording(1, 10));
        let p = Command::AddTrack {
            sequence,
            kind: TrackKind::Video,
            name: "V2".into(),
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let (v2, second) = (seq.video_tracks[1].id, seq.video_tracks[0].clips[1].clone());
        let apply = |p: &Project, command: Command| command.apply(p).unwrap();

        // Dragged up over the first recording: its sound needs a new track.
        let moved = |track, by| Command::MoveBetweenTracks {
            sequence,
            clip: second.id,
            track,
            by: Time::seconds(by),
        };
        let over = apply(&p, moved(Some(v2), -20));
        assert_eq!(
            moved_says(seq, over.active().unwrap(), second.id),
            "Its sound is on A3 (a new track): other sound was in the way, and nothing was cut"
        );
        // Straight up: nothing to say.
        let up = apply(&p, moved(Some(v2), 0));
        assert_eq!(moved_says(seq, up.active().unwrap(), second.id), "");
        // Past the top track, over the first recording.
        let top = apply(&p, moved(None, -20));
        assert_eq!(
            moved_says(seq, top.active().unwrap(), second.id),
            "V3 was made for it. Its sound is on A3 (a new track): other sound was in the way, and nothing was cut"
        );
        // Back down from where its sound has a track of its own that is
        // free there too: the sound stays, nothing to say.
        let back = apply(&over, moved(Some(seq.video_tracks[0].id), 20));
        assert_eq!(
            moved_says(over.active().unwrap(), back.active().unwrap(), second.id),
            ""
        );

        // The short file laid on V2 over the first recording, then again
        // later on V2: its sound shares the track that was made.
        let lay = |p: &Project, at| {
            let command = Command::LayMedia {
                sequence,
                media: second.media,
                at: Time::seconds(at),
                track: Some(v2),
            };
            let after = command.apply(p).unwrap();
            let says = laid_says(
                p.active().unwrap(),
                after.active().unwrap(),
                second.media,
                Time::seconds(at),
            );
            (after, says)
        };
        let (laid, says) = lay(&p, 5);
        assert_eq!(
            says,
            "Its sound is on A3 (a new track): other sound was in the way, and nothing was cut"
        );
        let (_, says) = lay(&laid, 18);
        assert_eq!(
            says,
            "Its sound is on A3: other sound was in the way, and nothing was cut"
        );
        // Where the first audio track is free, it lands as usual.
        let (_, says) = lay(&p, 60);
        assert_eq!(says, "");
        assert_eq!(
            listed(&["Mic".into(), "A3".into(), "A4".into()]),
            "Mic, A3 and A4"
        );
    }

    #[test]
    fn adding_a_track_says_where_it_went() {
        use project::TrackKind;
        assert!(added_track_says("V2", TrackKind::Video, false).starts_with("V2 added on top"));
        assert!(added_track_says("A3", TrackKind::Audio, false).contains("under the other audio tracks"));
        assert!(
            added_track_says("A3", TrackKind::Audio, true).contains("rough cut view shows it once"),
            "or it would seem that nothing happened"
        );
    }

    #[test]
    fn the_rough_cut_folds_linked_sound_into_its_picture() {
        let mut d = Drawn {
            video: vec![track(1, vec![clip(10, Some(1), None)])],
            audio: vec![
                track(
                    2,
                    vec![ui_kit::TimelineClip {
                        loading: Some(Loading {
                            what: LoadKind::Sound,
                            state: READING,
                        }),
                        ..clip(20, Some(1), Some(0.5))
                    }],
                ),
                track(3, vec![clip(30, Some(1), Some(0.9)), clip(31, None, Some(0.2))]),
            ],
            markers: Vec::new(),
            cache: Vec::new(),
        };
        fold_sound(&mut d);
        let w = d.video[0].clips[0].waveform.as_ref().unwrap();
        assert_eq!(w.peaks[0], 0.5, "the first sound track's");
        assert_eq!(
            d.video[0].clips[0].loading.map(|l| l.what),
            Some(LoadKind::Sound),
            "and how the reading of that sound is going"
        );
        assert_eq!(d.audio.len(), 1, "the emptied track is gone");
        assert_eq!(
            d.audio[0].clips.iter().map(|c| c.id).collect::<Vec<_>>(),
            [31],
            "unlinked sound stays"
        );
    }

    #[test]
    fn a_level_line_let_go_sets_that_clips_volume_only() {
        use project::{Command, SequenceSettings, Sound, Time};
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        // A recording with two sound streams: two linked sound clips.
        let p = Command::Import {
            sequence,
            media: recording(2, 10),
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        let (first, second) = (seq.audio_tracks[0].clips[0].id, seq.audio_tracks[1].clips[0].id);
        // It already fades in and sits to the left: that stays.
        let shaped = Sound {
            pan: -400_000,
            fade_in: Time::seconds(1),
            ..Sound::PLAIN
        };
        let p = Command::SetSound {
            sequence,
            sounds: vec![(first, shaped)],
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();

        let command = clip_volume_command(seq, first, -6.5).unwrap();
        let after = command.apply(&p).unwrap();
        let clip = |id| {
            after
                .active()
                .unwrap()
                .audio_tracks
                .iter()
                .flat_map(|t| &t.clips)
                .find(|c| c.id == id)
                .unwrap()
                .sound
        };
        assert_eq!(
            clip(first),
            Sound {
                gain_mdb: -6_500,
                ..shaped
            }
        );
        assert_eq!(clip(second), Sound::PLAIN, "the other stream is left alone");

        // The bottom is silence; the same volume again is no step; a clip that is gone neither.
        assert_eq!(gain_mdb(-60.0), Sound::SILENT);
        assert_eq!(gain_mdb(12.0), Sound::LOUDEST);
        assert_eq!(gain_mdb(-3.04), -3_040);
        assert_eq!(clip_volume_command(seq, first, 0.0), None);
        assert_eq!(clip_volume_command(seq, ClipId(999_999), -3.0), None);
    }
}
