//! Export presets: the built-in ones (Web 1080p, Web 4K, Vertical, Archive
//! master, Small preview, sound alone) and the person's own.
//!
//! Own presets belong to the person, not to a project, like templates and
//! the labels offered everywhere: one small file, `export-presets.ron` in
//! the app's data folder, with its own format version. Codecs are written
//! by short names of their own (`"h264"`, `"flac"`), never by their place
//! in a list, so the file reads the same when formats are added.
//!
//! ```ron
//! (
//!   format_version: 1,
//!   presets: [
//!     (name: "Clips for the team", container: "mp4", video: "h264", audio: "aac",
//!      quality: "small", size: Lines(720), rate: Some((30, 1)), normalise: true),
//!   ],
//! )
//! ```

use super::choice::{Choice, FrameSize};
use diag::{AppError, Explain};
use engine::export::{AudioCodec, Container, Encoders, Quality, VideoCodec};
use project::Rate;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 1;
/// The file's name in the app's data folder.
pub const FILE_NAME: &str = "export-presets.ron";
/// The longest name a preset may have, in characters.
pub const NAME_LIMIT: usize = 48;

/// A preset: settings with a name.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub choice: Choice,
    /// A few words after the format in the list ("for phones").
    pub note: &'static str,
    /// One of the person's own (can be removed).
    pub own: bool,
}

impl Preset {
    /// The line under its name: the format, the loudness, the note.
    pub fn about(&self, target: Option<f32>) -> String {
        let mut parts = vec![self.choice.format_words()];
        if let (true, Some(target)) = (self.choice.normalise, target) {
            parts.push(format!("{} LUFS", super::words::target_text(target)));
        }
        if !self.note.is_empty() {
            parts.push(self.note.to_owned());
        }
        parts.join(" · ")
    }
}

/// The built-in presets. Those that prefer H.265 take H.264 when this
/// machine can't encode H.265 (`encoders`, once known).
pub fn built_in(encoders: Option<&Encoders>) -> Vec<Preset> {
    let h265 = || {
        if encoders.is_none_or(|e| e.video(VideoCodec::H265).any()) {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        }
    };
    let web = Choice {
        quality: Quality::High,
        normalise: true,
        ..Choice::default()
    };
    let preset = |name: &str, note, choice: Choice| {
        let mut choice = choice;
        choice.settle(encoders);
        Preset {
            name: name.to_owned(),
            choice,
            note,
            own: false,
        }
    };
    vec![
        preset(
            "Web, 1080p",
            "",
            Choice {
                size: FrameSize::Lines(1080),
                ..web.clone()
            },
        ),
        preset(
            "Web, 4K",
            "",
            Choice {
                size: FrameSize::Lines(2160),
                video: h265(),
                ..web.clone()
            },
        ),
        preset(
            "Vertical, 1080×1920",
            "for phones",
            Choice {
                size: FrameSize::Exact(1080, 1920),
                ..web.clone()
            },
        ),
        preset(
            "Archive master",
            "large, for keeping",
            Choice {
                container: Container::Mkv,
                video: h265(),
                audio: AudioCodec::Flac,
                quality: Quality::High,
                ..Choice::default()
            },
        ),
        preset(
            "Small preview",
            "quick to send",
            Choice {
                quality: Quality::Small,
                size: FrameSize::Lines(720),
                ..Choice::default()
            },
        ),
        preset(
            "Audio only, WAV",
            "lossless",
            Choice {
                container: Container::Wav,
                audio: AudioCodec::Pcm24,
                ..Choice::default()
            },
        ),
        preset(
            "Audio only, MP3",
            "320 kb/s",
            Choice {
                container: Container::Mp3,
                audio: AudioCodec::Mp3,
                quality: Quality::High,
                normalise: true,
                ..Choice::default()
            },
        ),
    ]
}

/// The person's own presets, in the order they were saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnPresets {
    pub presets: Vec<Preset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum StoredSize {
    Timeline,
    Lines(u32),
    Exact(u32, u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Stored {
    name: String,
    container: String,
    #[serde(default)]
    video: Option<String>,
    audio: String,
    quality: String,
    size: StoredSize,
    #[serde(default)]
    rate: Option<(u32, u32)>,
    #[serde(default)]
    normalise: bool,
    #[serde(default)]
    software: bool,
    #[serde(default)]
    sound_beside: bool,
}

#[derive(Serialize)]
struct FileOut<'a> {
    format_version: u32,
    presets: &'a [Stored],
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    presets: Vec<Stored>,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
}

fn video_id(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
        VideoCodec::Av1 => "av1",
    }
}

fn audio_id(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "opus",
        AudioCodec::Flac => "flac",
        AudioCodec::Pcm24 => "pcm24",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Vorbis => "vorbis",
    }
}

fn quality_id(quality: Quality) -> &'static str {
    match quality {
        Quality::High => "high",
        Quality::Medium => "medium",
        Quality::Small => "small",
    }
}

impl Stored {
    fn of(preset: &Preset) -> Stored {
        let c = &preset.choice;
        Stored {
            name: preset.name.clone(),
            container: c.container.extension().to_owned(),
            video: c.container.has_video().then(|| video_id(c.video).to_owned()),
            audio: audio_id(c.audio).to_owned(),
            quality: quality_id(c.quality).to_owned(),
            size: match c.size {
                FrameSize::Timeline => StoredSize::Timeline,
                FrameSize::Lines(lines) => StoredSize::Lines(lines),
                FrameSize::Exact(w, h) => StoredSize::Exact(w, h),
            },
            rate: c.rate.map(|r| (r.num(), r.den())),
            normalise: c.normalise,
            software: c.software,
            sound_beside: c.sound_beside,
        }
    }

    fn preset(self) -> diag::Result<Preset> {
        let container = Container::from_extension(&self.container).ok_or_else(damaged)?;
        let video = match &self.video {
            Some(id) => VideoCodec::ALL
                .into_iter()
                .find(|&c| video_id(c) == id)
                .ok_or_else(damaged)?,
            None => VideoCodec::H264,
        };
        let audio = AudioCodec::ALL
            .into_iter()
            .find(|&c| audio_id(c) == self.audio)
            .ok_or_else(damaged)?;
        let quality = Quality::ALL
            .into_iter()
            .find(|&q| quality_id(q) == self.quality)
            .ok_or_else(damaged)?;
        let rate = match self.rate {
            Some((num, den)) => Some(Rate::new(num, den).map_err(|_| damaged())?),
            None => None,
        };
        if !container.allows(container.has_video().then_some(video), audio) {
            return Err(damaged());
        }
        Ok(Preset {
            name: self.name,
            choice: Choice {
                container,
                video,
                audio,
                quality,
                size: match self.size {
                    StoredSize::Timeline => FrameSize::Timeline,
                    StoredSize::Lines(lines) => FrameSize::Lines(lines),
                    StoredSize::Exact(w, h) => FrameSize::Exact(w, h),
                },
                rate,
                normalise: self.normalise,
                software: self.software,
                sound_beside: self.sound_beside,
            },
            note: "",
            own: true,
        })
    }
}

/// Where the file is.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

impl OwnPresets {
    /// Reads them from the app's data folder: none when there is no file
    /// yet; refused when it was written by a newer version or is damaged
    /// (the file is then left as it is).
    pub fn load(data_dir: &Path) -> diag::Result<OwnPresets> {
        let file = path(data_dir);
        if !file.exists() {
            return Ok(OwnPresets::default());
        }
        let text = std::fs::read_to_string(&file).or_explain(|| {
            AppError::new("Your export presets can't be read")
                .why(format!("{} could not be read.", file.display()))
                .fix("Check that the app's data folder can be read. The built-in presets still work.")
        })?;
        OwnPresets::parse(&text)
    }

    pub fn parse(text: &str) -> diag::Result<OwnPresets> {
        let version = ron::from_str::<Header>(text).or_explain(damaged)?.format_version;
        if version > FORMAT_VERSION {
            return Err(AppError::new("Your export presets were saved by a newer version")
                .why(format!(
                    "They use format {version}; this version reads up to format {FORMAT_VERSION}."
                ))
                .fix(
                    "Update the app. Until then the built-in presets work, and the file is left as it is.",
                ));
        }
        let file: FileIn = ron::from_str(text).or_explain(damaged)?;
        let presets = file
            .presets
            .into_iter()
            .map(Stored::preset)
            .collect::<diag::Result<_>>()?;
        Ok(OwnPresets { presets })
    }

    /// The file text in the current format.
    pub fn to_text(&self) -> diag::Result<String> {
        let stored: Vec<Stored> = self.presets.iter().map(Stored::of).collect();
        let config = ron::ser::PrettyConfig::new().indentor("  ".to_owned());
        ron::ser::to_string_pretty(
            &FileOut {
                format_version: FORMAT_VERSION,
                presets: &stored,
            },
            config,
        )
        .or_explain(|| {
            AppError::new("Your export presets can't be saved")
                .why("They could not be turned into file text.")
        })
    }

    /// Writes them to the app's data folder (a temporary file, then
    /// renamed over the old one). Touches the disk: call it from a worker.
    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("Your export presets can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full and that the app's data folder can be written to.")
        };
        let text = self.to_text()?;
        std::fs::create_dir_all(data_dir).or_explain(explain)?;
        let file = path(data_dir);
        let temporary = file.with_extension("ron.saving");
        std::fs::write(&temporary, text).or_explain(explain)?;
        std::fs::rename(&temporary, &file).or_explain(explain)
    }

    /// Saves `choice` under `name`; a preset of the same name (in any
    /// capitals) is replaced. Returns whether one was replaced.
    pub fn add(&mut self, name: &str, choice: Choice) -> diag::Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new("A preset needs a name").fix("Type at least one letter."));
        }
        if name.chars().count() > NAME_LIMIT {
            return Err(AppError::new("That preset name is too long")
                .why(format!(
                    "Names are up to {NAME_LIMIT} characters, so they fit in the list."
                ))
                .fix("Use a shorter name."));
        }
        if built_in(None).iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            return Err(AppError::new("That name is a built-in preset's")
                .why("Built-in presets can't be replaced.")
                .fix("Give yours another name."));
        }
        let preset = Preset {
            name: name.to_owned(),
            choice,
            note: "",
            own: true,
        };
        match self
            .presets
            .iter_mut()
            .find(|p| p.name.to_lowercase() == name.to_lowercase())
        {
            Some(old) => {
                *old = preset;
                Ok(true)
            }
            None => {
                self.presets.push(preset);
                Ok(false)
            }
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.presets.retain(|p| p.name != name);
    }
}

fn damaged() -> AppError {
    AppError::new("Your export presets can't be read")
        .why("The file holding them is damaged.")
        .fix(format!(
            "Delete {FILE_NAME} in the app's data folder and save your presets again. The built-in presets still work."
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine() -> Choice {
        Choice {
            quality: Quality::Small,
            size: FrameSize::Lines(720),
            rate: Some(Rate::FPS_30),
            normalise: true,
            sound_beside: true,
            ..Choice::default()
        }
    }

    #[test]
    fn own_presets_are_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            OwnPresets::load(dir.path()).unwrap(),
            OwnPresets::default(),
            "no file: none"
        );
        let mut own = OwnPresets::default();
        assert!(!own.add(" Clips for the team ", mine()).unwrap());
        let archive = Choice {
            container: Container::Mkv,
            video: VideoCodec::Av1,
            audio: AudioCodec::Flac,
            size: FrameSize::Exact(1080, 1080),
            ..Choice::default()
        };
        own.add("Square keep", archive).unwrap();
        own.add(
            "Sound",
            Choice {
                container: Container::Ogg,
                audio: AudioCodec::Vorbis,
                ..Choice::default()
            },
        )
        .unwrap();
        own.save(dir.path()).unwrap();
        let back = OwnPresets::load(dir.path()).unwrap();
        assert_eq!(back, own);
        assert_eq!(back.presets[0].name, "Clips for the team");
        // The same name again replaces it.
        assert!(own.add("clips FOR the team", Choice::default()).unwrap());
        assert_eq!(own.presets.len(), 3);
        own.remove("Sound");
        assert_eq!(own.presets.len(), 2);
        assert!(own.add("", mine()).is_err());
        assert!(own.add(&"x".repeat(NAME_LIMIT + 1), mine()).is_err());
        assert!(own.add("web, 1080P", mine()).is_err(), "a built-in name");
    }

    #[test]
    fn the_file_is_written_with_names_not_numbers() {
        let mut own = OwnPresets::default();
        own.add("Clips", mine()).unwrap();
        let text = own.to_text().unwrap();
        for word in [
            "format_version: 1",
            "\"mp4\"",
            "\"h264\"",
            "\"aac\"",
            "\"small\"",
            "Lines(720)",
        ] {
            assert!(text.contains(word), "{word} in {text}");
        }
        // Written by hand, with only what is needed.
        let by_hand = r#"(format_version: 1, presets: [(name: "A", container: "wav", audio: "pcm24", quality: "high", size: Timeline)])"#;
        let read = OwnPresets::parse(by_hand).unwrap();
        assert_eq!(read.presets[0].choice.container, Container::Wav);
        assert!(!read.presets[0].choice.normalise);
    }

    #[test]
    fn newer_and_damaged_files_are_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let newer = "(format_version: 99, presets: [])";
        std::fs::write(path(dir.path()), newer).unwrap();
        let err = OwnPresets::load(dir.path()).unwrap_err();
        assert!(err.report().contains("newer version"));
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), newer);
        for damaged in [
            "not ron",
            r#"(format_version: 1, presets: [(name: "A", container: "avi", audio: "aac", quality: "high", size: Timeline)])"#,
            r#"(format_version: 1, presets: [(name: "A", container: "mp4", video: "h264", audio: "flac", quality: "high", size: Timeline)])"#,
        ] {
            assert!(
                OwnPresets::parse(damaged)
                    .unwrap_err()
                    .report()
                    .contains("damaged"),
                "{damaged}"
            );
        }
    }

    #[test]
    fn built_in_presets_are_the_planned_ones_and_fit_their_formats() {
        let presets = built_in(None);
        let names: Vec<_> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Web, 1080p",
                "Web, 4K",
                "Vertical, 1080×1920",
                "Archive master",
                "Small preview",
                "Audio only, WAV",
                "Audio only, MP3"
            ]
        );
        for preset in &presets {
            let c = &preset.choice;
            assert!(
                c.container
                    .allows(c.container.has_video().then_some(c.video), c.audio),
                "{}",
                preset.name
            );
        }
        assert_eq!(presets[3].choice.audio, AudioCodec::Flac);
        assert_eq!(presets[3].choice.video, VideoCodec::H265);
        assert_eq!(presets[0].about(Some(-14.0)), "MP4 · H.264 · AAC · -14 LUFS");
        assert_eq!(presets[2].about(None), "MP4 · H.264 · AAC · for phones");
        // A machine without H.265 gets H.264 for the archive.
        let none = Encoders::default();
        let fallback = built_in(Some(&none));
        assert_eq!(fallback[3].choice.video, VideoCodec::H264);
    }
}
