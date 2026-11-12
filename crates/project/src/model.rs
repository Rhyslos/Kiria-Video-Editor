//! What a project is made of.
//!
//! All collections are persistent (`imbl`): cloning a `Project` is cheap and
//! shares everything unchanged, so undo history, autosave and background
//! rendering can each hold their own snapshot.

use crate::ids::{ClipId, LabelId, LinkId, MediaId, SequenceId, TrackId};
use crate::keyframes::Keyframes;
use crate::look::Look;
use crate::ramp::Ramp;
use crate::sound::Sound;
use crate::time::{Rate, Speed, Time};
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
    /// Labels, lists and notes on the media (format 13); empty when missing.
    #[serde(default)]
    pub library: crate::library::Library,
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

    /// 2560×1440 at 144 fps, 48 kHz: the "High frame rate" preset.
    pub const HIGH_FRAME_RATE: SequenceSettings = SequenceSettings {
        width: 2560,
        height: 1440,
        frame_rate: Rate::FPS_144,
        sample_rate: 48_000,
    };

    /// 1080×1920 (portrait) at 60 fps, 48 kHz: the "Vertical short" preset.
    pub const VERTICAL_SHORT: SequenceSettings = SequenceSettings {
        width: 1080,
        height: 1920,
        frame_rate: Rate::FPS_60,
        sample_rate: 48_000,
    };

    /// The optional templates, with their names, as offered for new projects
    /// and in a timeline's settings.
    pub const TEMPLATES: [(&'static str, SequenceSettings); 3] = [
        ("Game clip", SequenceSettings::GAME_CLIP),
        ("High frame rate", SequenceSettings::HIGH_FRAME_RATE),
        ("Vertical short", SequenceSettings::VERTICAL_SHORT),
    ];

    /// Settings matching a video: its upright size and frame rate, and the
    /// sound rate of its first sound stream (48 kHz when it has none).
    pub fn matching(video: &VideoStream, audio: &[AudioStream]) -> SequenceSettings {
        let (width, height) = video.display_size();
        SequenceSettings {
            width,
            height,
            frame_rate: video.frame_rate,
            sample_rate: audio.first().map_or(48_000, |a| a.sample_rate).max(8_000),
        }
    }

    /// A short description such as "1080p 60" or "1080×1920 60" (portrait).
    pub fn short_label(&self) -> String {
        let fps = self.frame_rate.nominal_fps();
        if self.width >= self.height {
            format!("{}p {fps}", self.height)
        } else {
            format!("{}×{} {fps}", self.width, self.height)
        }
    }
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
    /// What the file holds, read when it was imported. Empty for media added
    /// before format 2.
    #[serde(default)]
    pub info: MediaInfo,
}

/// The streams of a media file, as found when it was imported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Container format, such as "matroska,webm" or "mov,mp4,m4a,3gp,3g2,mj2".
    pub container: String,
    /// The picture stream, if any.
    pub video: Option<VideoStream>,
    /// Every sound stream, in file order. A game recording often has several
    /// (game, microphone, voice chat).
    pub audio: Vec<AudioStream>,
    /// Set when the file's end is missing and it was imported anyway (format
    /// 14): the length the file says it has. The media's `duration` is
    /// what is really there.
    #[serde(default)]
    pub stated_length: Option<Time>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoStream {
    /// Stream number in the file.
    pub index: u32,
    /// Codec name, such as "h264", "hevc" or "av1".
    pub codec: String,
    /// Stored picture size, before rotation.
    pub width: u32,
    pub height: u32,
    /// The constant rate the picture is shown at: the file's own rate, or for
    /// a variable-frame-rate recording the nearest standard rate.
    pub frame_rate: Rate,
    /// The file's frames are unevenly spaced (common in game recordings).
    /// Frames are then placed by their timestamps.
    #[serde(default)]
    pub variable_frame_rate: bool,
    /// Clockwise turn to show the picture upright: 0, 90, 180 or 270.
    pub rotation: u16,
}

impl VideoStream {
    /// Picture size as shown, after rotation.
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStream {
    /// Stream number in the file.
    pub index: u32,
    pub codec: String,
    pub channels: u16,
    pub sample_rate: u32,
    /// The stream's title in the file, or "Audio N" when it has none.
    pub name: String,
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
    /// "Automatic": the first video imported sets `settings` (which hold
    /// placeholder values until then). Off once settled or chosen by hand.
    #[serde(default)]
    pub automatic: bool,
    /// Volume of the whole mix in decibels (format 6); 0 leaves it as it is.
    #[serde(default)]
    pub master_gain_db: f32,
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
    /// How fast it plays its file (format 7); normal speed when missing.
    #[serde(default)]
    pub speed: Speed,
    /// Where its picture sits in the frame, which part shows, how strong and
    /// its quick colour (format 8); the picture as it is when missing.
    /// Means nothing for sound.
    #[serde(default)]
    pub look: Look,
    /// How loud it is, its pan and its fades (format 9); the sound as it is
    /// when missing. Set on clips on audio tracks only: a picture's sound is
    /// its linked sound clips.
    #[serde(default)]
    pub sound: Sound,
    /// How numbers of its look change over time (format 10): keyframes at
    /// moments of its file. Numbers without any keep `look`'s value. Means
    /// nothing for sound.
    #[serde(default)]
    pub keyframes: Keyframes,
    /// How its speed changes over time (format 11): speed keyframes on
    /// the timeline from its start. Empty: it plays at `speed` throughout,
    /// and `speed` means nothing while it isn't.
    #[serde(default)]
    pub ramp: Ramp,
    /// What it shows between two recorded frames when it is slowed down
    /// further than its file has frames for (format 11). Means nothing
    /// for sound.
    #[serde(default)]
    pub blend: FrameBlend,
    /// What it shows and plays (format 12): a part of a file (`media`),
    /// another timeline of the project, or nothing of its own (an
    /// adjustment layer). A file when missing.
    #[serde(default)]
    pub kind: ClipKind,
}

/// What a clip shows and plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ClipKind {
    /// A part of its file, `Clip::media`.
    #[default]
    File,
    /// Another timeline of the project, played as one piece: a nested
    /// timeline. On a video track it shows that timeline's picture, on an
    /// audio track it plays that timeline's sound. Its `media` is
    /// [`MediaId::NONE`], and times "in its file" are times in that timeline.
    Timeline(SequenceId),
    /// No picture of its own, only on video tracks: its quick colour, at
    /// its opacity, applies to everything below it in the picture for as
    /// long as it lasts. Its place, size, turn and crop mean nothing. Its
    /// `media` is [`MediaId::NONE`].
    Adjustment,
}

/// What a clip shows at a moment between two of its file's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FrameBlend {
    /// The frame on screen then, as it was recorded: slowed down past
    /// the file's frame rate, frames repeat.
    #[default]
    Off,
    /// The two frames around the moment mixed by how near each is, in
    /// linear light, so slow motion moves on every frame.
    Blend,
}

impl Clip {
    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    /// The timeline it plays, when it is a nested timeline.
    pub fn nested(&self) -> Option<SequenceId> {
        match self.kind {
            ClipKind::Timeline(id) => Some(id),
            ClipKind::File | ClipKind::Adjustment => None,
        }
    }

    pub fn is_adjustment(&self) -> bool {
        self.kind == ClipKind::Adjustment
    }

    /// Whether it would play from before the start of what it plays. An
    /// adjustment layer plays nothing, so it can reach as far back as wanted.
    pub fn starts_before_its_source(&self) -> bool {
        self.source_in.is_negative() && !self.is_adjustment()
    }

    /// Whether it plays a part of a media file (and `media` means something).
    pub fn is_file(&self) -> bool {
        self.kind == ClipKind::File
    }

    /// How much of its file the clip plays.
    pub fn source_len(&self) -> Time {
        self.source_offset(self.duration)
    }

    /// Where in its file the clip is at timeline time `t`.
    pub fn source_at(&self, t: Time) -> Time {
        self.source_in + self.source_offset(t - self.start)
    }

    /// How much of its file it plays from its start to `by` after it on
    /// the timeline (negative before it): its speed, or its ramp added up.
    pub fn source_offset(&self, by: Time) -> Time {
        if self.ramp.is_empty() {
            self.speed.scale(by)
        } else {
            self.ramp.source_offset(by)
        }
    }

    /// [`Clip::source_at`] in seconds, for drawing and for sound.
    pub fn source_at_f64(&self, t: f64) -> f64 {
        let by = t - self.start.as_secs_f64();
        let offset = if self.ramp.is_empty() {
            by * self.speed.as_f64()
        } else {
            self.ramp.source_offset_f64(by)
        };
        self.source_in.as_secs_f64() + offset
    }

    /// The speed it plays at, at `t` on the timeline.
    pub fn speed_at(&self, t: Time) -> Speed {
        self.ramp.speed_at(t - self.start).unwrap_or(self.speed)
    }

    /// The same clip with its start edge `by` later on the timeline
    /// (earlier when negative), its picture staying where it is: it starts
    /// further into its file and its ramp keeps its place. Its start and
    /// length are left to the caller.
    pub fn start_cut_by(&self, by: Time) -> Clip {
        Clip {
            source_in: self.source_in + self.source_offset(by),
            ramp: self.ramp.shifted(Time::ZERO - by),
            ..self.clone()
        }
    }

    /// "50%", "Ramp 100→25%": how fast it plays, in a few words.
    pub fn speed_summary(&self) -> String {
        if self.ramp.is_empty() {
            self.speed.percent()
        } else {
            self.ramp.summary()
        }
    }
}

/// How many of `sequences` have the id `id`.
fn sequences_seen(sequences: &Vector<Sequence>, id: SequenceId) -> usize {
    sequences.iter().filter(|s| s.id == id).count()
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

impl LabelColor {
    pub const ALL: [LabelColor; 8] = [
        LabelColor::Blue,
        LabelColor::Green,
        LabelColor::Pink,
        LabelColor::Yellow,
        LabelColor::Orange,
        LabelColor::Purple,
        LabelColor::Red,
        LabelColor::Grey,
    ];
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
    /// A new project whose timeline takes its settings from the first video
    /// imported (the default for new projects).
    pub fn new_automatic(name: impl Into<String>) -> Project {
        let mut project = Project::new(name, SequenceSettings::GAME_CLIP);
        for sequence in project.sequences.iter_mut() {
            sequence.automatic = true;
        }
        project
    }

    /// A new project with one timeline holding one video and one audio track.
    pub fn new(name: impl Into<String>, settings: SequenceSettings) -> Project {
        let mut project = Project {
            name: name.into(),
            media: OrdMap::new(),
            sequences: Vector::new(),
            active_sequence: SequenceId(0),
            library: Default::default(),
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
            automatic: false,
            master_gain_db: 0.0,
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
            if sequences_seen(&self.sequences, sequence.id) > 1 {
                problems.push(format!("two timelines have the id {}", sequence.id));
            }
        }
        problems.extend(self.library.problems(|m| self.media.contains_key(&m)));
        if let Some(name) = crate::timelines::in_a_loop(self) {
            problems.push(format!("{name} plays itself through the timelines nested in it"));
        }
        problems
    }

    /// The timeline with this id.
    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.iter().find(|s| s.id == id)
    }

    /// How long what a clip plays is: its file's length (`None` for a still
    /// or a file whose length isn't known), the nested timeline's length,
    /// or `None` for an adjustment layer, which can be as long as wanted.
    pub fn source_length(&self, clip: &Clip) -> Option<Time> {
        match clip.kind {
            ClipKind::File => self
                .media
                .get(&clip.media)
                .filter(|m| m.kind != MediaKind::Image && m.duration > Time::ZERO)
                .map(|m| m.duration),
            ClipKind::Timeline(id) => self.sequence(id).map(Sequence::length),
            ClipKind::Adjustment => None,
        }
    }

    fn check_clips(&self, sequence: &Sequence, track: &Track, problems: &mut Vec<String>) {
        let mut previous_end: Option<Time> = None;
        for clip in &track.clips {
            let place = format!("{} › {} › {}", sequence.name, track.name, clip.id);
            match clip.kind {
                ClipKind::File => {
                    if !self.media.contains_key(&clip.media) {
                        problems.push(format!("{place} uses missing media {}", clip.media));
                    }
                }
                ClipKind::Timeline(id) => {
                    if self.sequence(id).is_none() {
                        problems.push(format!("{place} plays the missing timeline {id}"));
                    }
                    if id == sequence.id {
                        problems.push(format!("{place} plays the timeline it is on"));
                    }
                }
                ClipKind::Adjustment => {
                    if track.kind == TrackKind::Audio {
                        problems.push(format!("{place} is an adjustment layer on an audio track"));
                    }
                }
            }
            if !clip.is_file() && clip.media != MediaId::NONE {
                problems.push(format!("{place} plays no file but names {}", clip.media));
            }
            if clip.duration <= Time::ZERO {
                problems.push(format!("{place} has no length"));
            }
            if clip.start.is_negative() || clip.starts_before_its_source() {
                problems.push(format!("{place} starts before zero"));
            }
            if clip.sound != clip.sound.within_limits() {
                problems.push(format!("{place} has a volume or pan out of range"));
            }
            if !clip.sound.fits(clip.duration) {
                problems.push(format!("{place} has fades longer than itself"));
            }
            problems.extend(clip.keyframes.problems(&place));
            problems.extend(clip.ramp.problems(&place));
            if track.kind == TrackKind::Audio && !clip.keyframes.is_empty() {
                problems.push(format!("{place} is sound but has picture keyframes"));
            }
            if previous_end.is_some_and(|end| clip.start < end) {
                problems.push(format!("{place} overlaps the clip before it"));
            }
            previous_end = Some(clip.end());
        }
    }

    /// Cuts every clip's fades to fit its length (see [`Sound::fitted`]).
    /// Edits that change a clip's length (trims, rolls, slides, speed) leave
    /// its fades as they were; this brings them back within the clip, once,
    /// after every change. Only tracks with a clip that needs it are
    /// touched, so snapshots keep sharing everything else.
    pub fn fit_fades(&mut self) {
        let needs = |track: &Track| track.clips.iter().any(|c| !c.sound.fits(c.duration));
        for i in 0..self.sequences.len() {
            let Some(seq) = self.sequences.get(i) else {
                continue;
            };
            if !seq.tracks().any(needs) {
                continue;
            }
            let Some(seq) = self.sequences.get_mut(i) else {
                continue;
            };
            for track in seq.video_tracks.iter_mut().chain(seq.audio_tracks.iter_mut()) {
                if !needs(track) {
                    continue;
                }
                for clip in track.clips.iter_mut() {
                    clip.sound = clip.sound.fitted(clip.duration);
                }
            }
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

    #[test]
    fn settings_have_short_labels() {
        assert_eq!(SequenceSettings::GAME_CLIP.short_label(), "1080p 60");
        assert_eq!(SequenceSettings::HIGH_FRAME_RATE.short_label(), "1440p 144");
        assert_eq!(SequenceSettings::VERTICAL_SHORT.short_label(), "1080×1920 60");
    }

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
                info: MediaInfo::default(),
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
            speed: Default::default(),
            look: Default::default(),
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
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
