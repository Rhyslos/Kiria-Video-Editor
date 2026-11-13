//! What the Export page's settings say, apart from the file and the range:
//! the format, the picture's size and frame rate, the quality, the sound's
//! loudness and the WAV beside. A preset is one of these with a name.

use engine::export::{AudioCodec, Container, Encoders, ExportSettings, Quality, VideoCodec};
use project::{Rate, Time};
use std::path::PathBuf;

/// The size of an exported picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSize {
    /// The timeline's own.
    Timeline,
    /// So many lines on the shorter side, the timeline's shape kept: 1080
    /// is 1920×1080 for a wide timeline and 1080×1920 for a tall one.
    Lines(u32),
    /// Exactly this size; the timeline's picture is fitted inside, with
    /// black bars where the shapes differ.
    Exact(u32, u32),
}

impl FrameSize {
    /// The sizes offered in the page's list, after the timeline's own.
    pub const OFFERED: [FrameSize; 8] = [
        FrameSize::Lines(2160),
        FrameSize::Lines(1440),
        FrameSize::Lines(1080),
        FrameSize::Lines(720),
        FrameSize::Lines(480),
        FrameSize::Exact(1080, 1920),
        FrameSize::Exact(1920, 1080),
        FrameSize::Exact(1080, 1080),
    ];

    /// The size in pixels for a timeline of `timeline`, even on both sides.
    pub fn pixels(self, timeline: (u32, u32)) -> (u32, u32) {
        let even = |side: f64| ((side.round() as u32) & !1).max(2);
        match self {
            FrameSize::Timeline => (even(f64::from(timeline.0)), even(f64::from(timeline.1))),
            FrameSize::Exact(w, h) => (even(f64::from(w)), even(f64::from(h))),
            FrameSize::Lines(lines) => {
                let (w, h) = (f64::from(timeline.0.max(1)), f64::from(timeline.1.max(1)));
                let lines = f64::from(lines);
                if w >= h {
                    (even(lines * w / h), even(lines))
                } else {
                    (even(lines), even(lines * h / w))
                }
            }
        }
    }

    /// How it is named in the list, for a timeline of `timeline`.
    pub fn label(self, timeline: (u32, u32)) -> String {
        let (w, h) = self.pixels(timeline);
        match self {
            FrameSize::Timeline => format!("{w} × {h} (the timeline's)"),
            FrameSize::Lines(2160) => format!("{w} × {h} (4K)"),
            FrameSize::Lines(lines) => format!("{w} × {h} ({lines}p)"),
            FrameSize::Exact(..) if w < h => format!("{w} × {h} (vertical)"),
            FrameSize::Exact(..) if w == h => format!("{w} × {h} (square)"),
            FrameSize::Exact(..) => format!("{w} × {h}"),
        }
    }
}

/// Frame rates offered below a timeline's own (lower rates only: more
/// frames than the timeline has would only repeat them).
pub const LOWER_RATES: [Rate; 5] = [
    Rate::FPS_60,
    Rate::FPS_50,
    Rate::FPS_30,
    Rate::FPS_25,
    Rate::FPS_24,
];

/// What to export, apart from where and which stretch.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub container: Container,
    pub video: VideoCodec,
    pub audio: AudioCodec,
    pub quality: Quality,
    pub size: FrameSize,
    /// `None`: the timeline's.
    pub rate: Option<Rate>,
    /// Normalise to the loudness aimed for in Settings › Audio.
    pub normalise: bool,
    /// Encode pictures on the processor only.
    pub software: bool,
    /// Also save the sound beside the file as WAV.
    pub sound_beside: bool,
}

impl Default for Choice {
    fn default() -> Choice {
        Choice {
            container: Container::Mp4,
            video: VideoCodec::H264,
            audio: AudioCodec::Aac,
            quality: Quality::Medium,
            size: FrameSize::Timeline,
            rate: None,
            normalise: false,
            software: false,
            sound_beside: false,
        }
    }
}

impl Choice {
    /// The engine's settings for writing `path` from `range`, normalising
    /// to `target` (from Settings) when asked to and there is one.
    pub fn settings(
        &self,
        path: PathBuf,
        range: Option<(Time, Time)>,
        timeline: (u32, u32),
        target: Option<f32>,
    ) -> ExportSettings {
        let video = self.container.has_video().then_some(self.video);
        let normalise = target.filter(|_| self.normalise);
        ExportSettings {
            path,
            range,
            container: self.container,
            video,
            audio: self.audio,
            quality: self.quality,
            software: self.software,
            normalise,
            limit: normalise.is_some(),
            size: video.map(|_| self.size.pixels(timeline)),
            rate: self.rate,
            sound_beside: self.sound_beside && self.container != Container::Wav,
        }
    }

    /// Keeps the codecs to ones the container holds and, once that is
    /// known, this machine encodes (after the container changed, or a
    /// preset was picked).
    pub fn settle(&mut self, encoders: Option<&Encoders>) {
        let video_ok = |codec: VideoCodec| encoders.is_none_or(|e| e.video(codec).any());
        let video = self.container.video_codecs();
        if !video.is_empty() && (!video.contains(&self.video) || !video_ok(self.video)) {
            self.video = video.iter().copied().find(|&c| video_ok(c)).unwrap_or(video[0]);
        }
        let audio_ok = |codec: AudioCodec| encoders.is_none_or(|e| e.audio(codec).is_some());
        let audio = self.container.audio_codecs();
        if !audio.contains(&self.audio) || !audio_ok(self.audio) {
            self.audio = audio.iter().copied().find(|&c| audio_ok(c)).unwrap_or(audio[0]);
        }
        if self.container == Container::Wav {
            self.sound_beside = false;
        }
    }

    /// What the file plays on, in plain words, for the format chosen.
    pub fn plays_on(&self) -> &'static str {
        plays_on(
            self.container,
            self.container.has_video().then_some(self.video),
            self.audio,
        )
    }

    /// The format in a few words: "MP4 · H.264 · AAC", "WAV · 24-bit".
    pub fn format_words(&self) -> String {
        if self.container.has_video() {
            format!(
                "{} · {} · {}",
                self.container.label(),
                self.video.label(),
                self.audio.label()
            )
        } else if self.audio == AudioCodec::Pcm24 {
            format!("{} · 24-bit", self.container.label())
        } else {
            format!("{} · {}", self.container.label(), self.audio.label())
        }
    }
}

/// What a file in this format plays on, in plain words: what reads it, not
/// which programs or makers (none are named).
pub fn plays_on(container: Container, video: Option<VideoCodec>, audio: AudioCodec) -> &'static str {
    match (container, video, audio) {
        (Container::Mp4, Some(VideoCodec::H264), AudioCodec::Aac) => {
            "Almost everything: phones, computers, TVs, browsers and video sites"
        }
        (Container::Mp4, Some(VideoCodec::H264), _) => {
            "Most computers, browsers and video sites; some phones and TVs can't play Opus sound in an MP4"
        }
        (Container::Mp4, Some(VideoCodec::H265), _) => {
            "Recent phones, computers and TVs, and video sites; some browsers and older devices can't play it"
        }
        (Container::Mp4, Some(VideoCodec::Av1), _) => {
            "Up-to-date browsers, recent phones and TVs, and video sites; older devices can't play it"
        }
        (Container::Mkv, _, _) => {
            "Media players on computers and many TVs; most phones and browsers can't open it. Made for keeping"
        }
        (Container::Wav, _, _) => "Every sound program and player: lossless and large",
        (Container::Mp3, _, _) => "Everything that plays sound",
        (Container::Ogg, _, AudioCodec::Opus) => {
            "Browsers and most media players; some phones and car stereos can't play it"
        }
        (Container::Ogg, _, _) => {
            "Browsers and most media players; many phones and car stereos can't play it"
        }
        (Container::Mp4, None, _) => "Phones, computers and browsers",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_keep_the_timelines_shape_or_are_exact() {
        let wide = (1920, 1080);
        let tall = (1080, 1920);
        assert_eq!(FrameSize::Timeline.pixels(wide), (1920, 1080));
        assert_eq!(FrameSize::Timeline.pixels((1921, 1081)), (1920, 1080));
        assert_eq!(FrameSize::Lines(2160).pixels(wide), (3840, 2160));
        assert_eq!(FrameSize::Lines(720).pixels(wide), (1280, 720));
        assert_eq!(FrameSize::Lines(1080).pixels(tall), (1080, 1920));
        assert_eq!(FrameSize::Lines(1080).pixels((2560, 1080)), (2560, 1080));
        assert_eq!(FrameSize::Exact(1080, 1920).pixels(wide), (1080, 1920));
        assert_eq!(FrameSize::Lines(720).label(wide), "1280 × 720 (720p)");
        assert_eq!(FrameSize::Lines(2160).label(wide), "3840 × 2160 (4K)");
        assert_eq!(FrameSize::Exact(1080, 1920).label(wide), "1080 × 1920 (vertical)");
        assert_eq!(FrameSize::Timeline.label(wide), "1920 × 1080 (the timeline's)");
    }

    #[test]
    fn the_engines_settings_follow_the_choice() {
        let choice = Choice {
            size: FrameSize::Lines(720),
            rate: Some(Rate::FPS_30),
            normalise: true,
            sound_beside: true,
            ..Choice::default()
        };
        let settings = choice.settings(PathBuf::from("/a/b.mp4"), None, (1920, 1080), Some(-14.0));
        assert_eq!(settings.size, Some((1280, 720)));
        assert_eq!(settings.rate, Some(Rate::FPS_30));
        assert_eq!(settings.normalise, Some(-14.0));
        assert!(settings.limit && settings.sound_beside);
        // No loudness aimed for: nothing to normalise to.
        let plain = choice.settings(PathBuf::from("/a/b.mp4"), None, (1920, 1080), None);
        assert_eq!(plain.normalise, None);
        assert!(!plain.limit);
        // Sound alone: no picture, no size, no second WAV beside a WAV.
        let wav = Choice {
            container: Container::Wav,
            audio: AudioCodec::Pcm24,
            ..choice
        };
        let settings = wav.settings(PathBuf::from("/a/b.wav"), None, (1920, 1080), None);
        assert_eq!((settings.video, settings.size), (None, None));
        assert!(!settings.sound_beside);
        assert!(settings.check().is_ok());
    }

    #[test]
    fn settling_keeps_codecs_the_container_holds() {
        let mut choice = Choice {
            container: Container::Ogg,
            ..Choice::default()
        };
        choice.settle(None);
        assert_eq!(choice.audio, AudioCodec::Opus);
        choice.container = Container::Mkv;
        choice.audio = AudioCodec::Flac;
        choice.settle(None);
        assert_eq!(choice.audio, AudioCodec::Flac, "kept: MKV holds FLAC");
        choice.container = Container::Mp4;
        choice.settle(None);
        assert_eq!(choice.audio, AudioCodec::Aac);
        choice.container = Container::Wav;
        choice.sound_beside = true;
        choice.settle(None);
        assert_eq!(choice.audio, AudioCodec::Pcm24);
        assert!(!choice.sound_beside);
    }

    #[test]
    fn what_a_file_plays_on_is_said_for_every_format() {
        for (container, video, audio) in engine::export::combinations() {
            let words = plays_on(container, video, audio);
            assert!(!words.is_empty());
        }
        assert!(
            plays_on(Container::Mp4, Some(VideoCodec::H264), AudioCodec::Aac)
                .starts_with("Almost everything")
        );
        assert!(plays_on(Container::Mkv, Some(VideoCodec::H265), AudioCodec::Flac).contains("keeping"));
        let choice = Choice::default();
        assert_eq!(choice.format_words(), "MP4 · H.264 · AAC");
        let wav = Choice {
            container: Container::Wav,
            audio: AudioCodec::Pcm24,
            ..Choice::default()
        };
        assert_eq!(wav.format_words(), "WAV · 24-bit");
    }
}
