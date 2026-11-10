//! What a project is made of.
//!
//! All collections are persistent (`imbl`): cloning a `Project` is cheap and
//! shares everything unchanged, so undo history, autosave and background
//! rendering can each hold their own snapshot.

use crate::ids::{ClipId, LabelId, LinkId, MediaId, SequenceId, TrackId};
use crate::time::{Rate, Time};
use diag::AppError;
use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A whole project: its media, its timelines and which timeline is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub media: OrdMap<MediaId, MediaRef>,
    pub sequences: Vector<Sequence>,
    pub active_sequence: SequenceId,
    /// The next unused id. Ids are never reused.
    next_id: u64,
}

/// Picture size, frame rate and audio rate of a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rate,
    pub sample_rate: u32,
}

impl SequenceSettings {
    /// 1920×1080 at 60 fps, 48 kHz: the "Game clip" preset.
    pub const GAME_CLIP: SequenceSettings = SequenceSettings {
        width: 1920,
        height: 1080,
        frame_rate: Rate::FPS_60,
        sample_rate: 48_000,
    };
}

/// A source file the project uses. The file itself is never changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRef {
    pub id: MediaId,
    pub path: PathBuf,
    /// Identifies the file's contents, so a moved or renamed file can be found again.
    pub fingerprint: Fingerprint,
    pub kind: MediaKind,
    pub duration: Time,
}

/// Size plus a hash of the start and end of a file: quick to compute on huge
/// recordings, and enough to recognise the same file in a new place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub sample_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

/// One timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub id: SequenceId,
    pub name: String,
    pub settings: SequenceSettings,
    pub video_tracks: Vector<Track>,
    pub audio_tracks: Vector<Track>,
    pub labels: Vector<TimelineLabel>,
    pub region: Option<Region>,
    /// Where cuts, splits and pastes happen; separate from the playhead.
    pub edit_needle: Time,
}

impl Sequence {
    /// From the timeline's start to the end of its last clip.
    pub fn length(&self) -> Time {
        self.tracks()
            .filter_map(|t| t.clips.iter().map(Clip::end).max())
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// Video tracks, then audio tracks.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.video_tracks.iter().chain(self.audio_tracks.iter())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

/// A row of clips. Clips are kept in start order and never overlap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub clips: Vector<Clip>,
    pub muted: bool,
    pub solo: bool,
    pub locked: bool,
    pub hidden: bool,
    /// Track volume in decibels; 0 leaves it unchanged.
    pub gain_db: f32,
}

/// A piece of a media file placed on a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub media: MediaId,
    /// Which stream of the file: the video stream, or one of several audio streams.
    pub stream: u32,
    /// Where the clip starts on the timeline.
    pub start: Time,
    /// Where in the source file the clip starts.
    pub source_in: Time,
    pub duration: Time,
    /// Clips with the same link move and split together.
    pub link: Option<LinkId>,
}

impl Clip {
    pub fn end(&self) -> Time {
        self.start + self.duration
    }
}

/// A named, coloured marker on the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineLabel {
    pub id: LabelId,
    pub name: String,
    pub at: Time,
    pub color: LabelColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelColor {
    Blue,
    Green,
    Pink,
    Yellow,
    Orange,
    Purple,
    Red,
    Grey,
}

/// A stretch of the timeline marked in the ruler: used to loop playback,
/// pre-render, and as an export range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub start: Time,
    pub end: Time,
}

impl Region {
    /// A region from `start` to `end`, in either order. `None` when they are equal.
    pub fn between(a: Time, b: Time) -> Option<Region> {
        (a != b).then(|| Region {
            start: a.min(b),
            end: a.max(b),
        })
    }

    pub fn length(&self) -> Time {
        self.end - self.start
    }
}

impl Project {
    /// A new project with one timeline holding one video and one audio track.
    pub fn new(name: impl Into<String>, settings: SequenceSettings) -> Project {
        let mut project = Project {
            name: name.into(),
            media: OrdMap::new(),
            sequences: Vector::new(),
            active_sequence: SequenceId(0),
            next_id: 1,
        };
        let sequence = project.new_sequence("Main sequence", settings);
        project.active_sequence = sequence.id;
        project.sequences.push_back(sequence);
        project
    }

    /// Hands out an id that has never been used in this project.
    pub fn allocate_id<T: From<u64>>(&mut self) -> T {
        let id = self.next_id;
        self.next_id += 1;
        T::from(id)
    }

    /// A new, empty timeline with one video and one audio track (not yet added to the project).
    pub fn new_sequence(&mut self, name: &str, settings: SequenceSettings) -> Sequence {
        let video = self.new_track(TrackKind::Video, "V1");
        let audio = self.new_track(TrackKind::Audio, "A1");
        Sequence {
            id: self.allocate_id(),
            name: name.to_owned(),
            settings,
            video_tracks: Vector::unit(video),
            audio_tracks: Vector::unit(audio),
            labels: Vector::new(),
            region: None,
            edit_needle: Time::ZERO,
        }
    }

    /// A new, empty track (not yet added to a timeline).
    pub fn new_track(&mut self, kind: TrackKind, name: &str) -> Track {
        Track {
            id: self.allocate_id(),
            kind,
            name: name.to_owned(),
            clips: Vector::new(),
            muted: false,
            solo: false,
            locked: false,
            hidden: false,
            gain_db: 0.0,
        }
    }

    /// The timeline that is open.
    pub fn active(&self) -> Option<&Sequence> {
        self.sequences.iter().find(|s| s.id == self.active_sequence)
    }

    /// Everything that breaks the model's rules, in plain words. Empty means valid.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.active().is_none() {
            problems.push(format!(
                "the open timeline {} does not exist",
                self.active_sequence
            ));
        }
        for (id, media) in &self.media {
            if media.id != *id {
                problems.push(format!("{id} is stored under the wrong id ({})", media.id));
            }
        }
        for sequence in &self.sequences {
            if sequence.settings.width == 0 || sequence.settings.height == 0 {
                problems.push(format!("{} has no picture size", sequence.name));
            }
            if let Some(region) = sequence.region
                && region.start >= region.end
            {
                problems.push(format!(
                    "{} has a region that ends before it starts",
                    sequence.name
                ));
            }
            for (tracks, kind) in [
                (&sequence.video_tracks, TrackKind::Video),
                (&sequence.audio_tracks, TrackKind::Audio),
            ] {
                for track in tracks {
                    if track.kind != kind {
                        problems.push(format!("track {} is in the wrong area", track.name));
                    }
                    self.check_clips(sequence, track, &mut problems);
                }
            }
        }
        problems
    }

    fn check_clips(&self, sequence: &Sequence, track: &Track, problems: &mut Vec<String>) {
        let mut previous_end: Option<Time> = None;
        for clip in &track.clips {
            let place = format!("{} › {} › {}", sequence.name, track.name, clip.id);
            if !self.media.contains_key(&clip.media) {
                problems.push(format!("{place} uses missing media {}", clip.media));
            }
            if clip.duration <= Time::ZERO {
                problems.push(format!("{place} has no length"));
            }
            if clip.start.is_negative() || clip.source_in.is_negative() {
                problems.push(format!("{place} starts before zero"));
            }
            if previous_end.is_some_and(|end| clip.start < end) {
                problems.push(format!("{place} overlaps the clip before it"));
            }
            previous_end = Some(clip.end());
        }
    }

    /// Checks the model's rules; the error lists what is wrong.
    pub fn validate(&self) -> diag::Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            return Ok(());
        }
        Err(AppError::new("The project is damaged")
            .why(problems.join("; "))
            .fix("Open the latest autosave or backup. If this keeps happening, report it with the Diagnostics details."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Project {
        let mut p = Project::new("Lofoten trip", SequenceSettings::GAME_CLIP);
        let media: MediaId = p.allocate_id();
        p.media.insert(
            media,
            MediaRef {
                id: media,
                path: PathBuf::from("recordings/drone.mkv"),
                fingerprint: Fingerprint {
                    size: 1_000_000,
                    sample_hash: "abc123".into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(30),
            },
        );
        let clip_id = p.allocate_id();
        let link = Some(p.allocate_id());
        let seq = p.sequences.front_mut().unwrap();
        seq.video_tracks[0].clips.push_back(Clip {
            id: clip_id,
            media,
            stream: 0,
            start: Time::seconds(2),
            source_in: Time::seconds(5),
            duration: Time::seconds(10),
            link,
        });
        p
    }

    #[test]
    fn a_new_project_is_valid_and_has_a_timeline() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        assert!(p.problems().is_empty(), "{:?}", p.problems());
        let seq = p.active().unwrap();
        assert_eq!(seq.video_tracks.len(), 1);
        assert_eq!(seq.audio_tracks.len(), 1);
        assert_eq!(seq.length(), Time::ZERO);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let a: ClipId = p.allocate_id();
        let b: ClipId = p.allocate_id();
        assert_ne!(a, b);
    }

    #[test]
    fn length_is_the_end_of_the_last_clip() {
        assert_eq!(sample().active().unwrap().length(), Time::seconds(12));
    }

    #[test]
    fn overlapping_clips_and_missing_media_are_reported() {
        let mut p = sample();
        let seq = p.sequences.front_mut().unwrap();
        let mut copy = seq.video_tracks[0].clips[0].clone();
        copy.start = Time::seconds(5);
        copy.media = MediaId(999);
        seq.video_tracks[0].clips.push_back(copy);
        let problems = p.problems().join("\n");
        assert!(problems.contains("overlaps"), "{problems}");
        assert!(problems.contains("missing media"), "{problems}");
        assert!(p.validate().is_err());
    }

    #[test]
    fn snapshots_share_unchanged_data() {
        let a = sample();
        let mut b = a.clone();
        b.name = "Renamed".into();
        assert_eq!(a.sequences, b.sequences);
        assert_ne!(a, b);
    }

    #[test]
    fn regions_are_ordered_and_never_empty() {
        let r = Region::between(Time::seconds(5), Time::seconds(2)).unwrap();
        assert_eq!((r.start, r.end), (Time::seconds(2), Time::seconds(5)));
        assert!(Region::between(Time::seconds(1), Time::seconds(1)).is_none());
    }
}
