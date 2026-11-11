//! Turns the open timeline into what the shared timeline widget draws, and
//! keeps the selection.

use crate::import;
use crate::reading::Pace;
use crate::waveforms::Look;
use project::{ClipId, LabelColor, Project, Sequence};
use std::collections::HashMap;
use std::time::Instant;
use ui_kit::timeline::{LoadKind, LoadState, Loading};
use ui_kit::{TimelineClip, TimelineMarker, TimelineTrack};

/// What the widget needs for one frame.
pub struct Drawn {
    pub video: Vec<TimelineTrack>,
    pub audio: Vec<TimelineTrack>,
    pub markers: Vec<TimelineMarker>,
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
                let media = project.media.get(&c.media);
                let is_missing = media.is_none_or(|m| missing.contains(&m.id));
                let (source_in, speed) = (c.source_in.as_secs_f64(), c.speed.as_f64());
                let length = c.duration.as_secs_f64();
                // What there is to draw inside the clip, how much of its
                // reading is done, and how the reading of the rest goes.
                let (mut peaks, mut strip, mut ready, mut state) = (None, None, 1.0, None);
                match (t.kind, media) {
                    (project::TrackKind::Audio, Some(m)) => {
                        let look = waveform(m, c.stream);
                        ready = share_read(look.read_secs, source_in, length * speed);
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
                let name = media
                    .and_then(|m| m.path.file_name())
                    .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned());
                let name = if c.speed == project::Speed::NORMAL {
                    name
                } else {
                    format!("{name} · {}", c.speed.percent())
                };
                TimelineClip {
                    id: c.id.0,
                    start: c.start.as_secs_f64(),
                    duration: length,
                    info: media.map_or_else(String::new, import::summary),
                    name,
                    selected: selected.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)),
                    missing: is_missing,
                    link: c.link.map(|l| l.0),
                    waveform: peaks.map(|p| ui_kit::ClipWaveform {
                        peaks: p.values,
                        per_second: p.per_second as f32,
                        source_in,
                        speed,
                    }),
                    pictures: strip.map(|p| ui_kit::ClipPictures {
                        source_in,
                        speed,
                        ..p
                    }),
                    loading: state.map(|state| Loading { what, state }),
                }
            })
            .collect();
        TimelineTrack {
            id: t.id.0,
            name: t.name.clone(),
            muted: t.muted,
            solo: t.solo,
            gain_db: t.gain_db,
            clips,
            loading: progress.loading(what),
        }
    };
    Drawn {
        video: seq.video_tracks.iter().map(&mut track).collect(),
        audio: seq.audio_tracks.iter().map(&mut track).collect(),
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

/// Drops clips that no longer exist (after a delete or an undo).
pub fn keep_existing(selected: &mut Vec<ClipId>, seq: &Sequence) {
    selected.retain(|id| seq.tracks().any(|t| t.clips.iter().any(|c| c.id == *id)));
}

#[cfg(test)]
mod tests {
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
            }),
            pictures: None,
            loading: None,
        }
    }

    fn track(id: u64, clips: Vec<ui_kit::TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: String::new(),
            clips,
            muted: false,
            solo: false,
            gain_db: 0.0,
            loading: None,
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
}
