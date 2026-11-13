//! What a file can be written as: containers, picture codecs and sound
//! codecs, which go together, and the encoders tried for each.
//!
//! These are plain lists a page can show; whether this machine can really
//! encode a codec is [`super::probe_video`] and [`super::audio_encoder`].

/// Where an encoder does its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The graphics card's own encoder.
    GraphicsCard,
    /// The operating system's encoder, which uses the graphics card when it
    /// can (Windows only).
    System,
    /// The processor: slower, and works everywhere.
    Processor,
}

/// One encoder: FFmpeg's name for it and where it works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder {
    pub name: &'static str,
    pub place: Place,
    /// It takes pictures from a video device's own frames (VA-API), so they
    /// are uploaded first.
    pub(crate) needs_upload: bool,
}

const fn card(name: &'static str) -> Encoder {
    Encoder {
        name,
        place: Place::GraphicsCard,
        needs_upload: false,
    }
}

const fn va(name: &'static str) -> Encoder {
    Encoder {
        name,
        place: Place::GraphicsCard,
        needs_upload: true,
    }
}

const fn system(name: &'static str) -> Encoder {
    Encoder {
        name,
        place: Place::System,
        needs_upload: false,
    }
}

const fn processor(name: &'static str) -> Encoder {
    Encoder {
        name,
        place: Place::Processor,
        needs_upload: false,
    }
}

const H264: [Encoder; 6] = [
    card("h264_amf"),
    card("h264_nvenc"),
    card("h264_qsv"),
    va("h264_vaapi"),
    system("h264_mf"),
    processor("libx264"),
];
const H265: [Encoder; 6] = [
    card("hevc_amf"),
    card("hevc_nvenc"),
    card("hevc_qsv"),
    va("hevc_vaapi"),
    system("hevc_mf"),
    processor("libx265"),
];
const AV1: [Encoder; 5] = [
    card("av1_amf"),
    card("av1_nvenc"),
    card("av1_qsv"),
    va("av1_vaapi"),
    processor("libsvtav1"),
];

/// How the picture is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    /// Plays everywhere.
    H264,
    /// About a third smaller for the same picture; most newer devices play it.
    H265,
    /// About half the size of H.264; newer devices and browsers play it.
    Av1,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 3] = [VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1];

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// The encoders, in the order they are tried: the graphics cards' own
    /// (the three vendors' and VA-API on Linux), the system's, then the
    /// processor's, which is always last.
    pub fn encoders(self) -> &'static [Encoder] {
        match self {
            VideoCodec::H264 => &H264,
            VideoCodec::H265 => &H265,
            VideoCodec::Av1 => &AV1,
        }
    }

    /// How many bits this codec needs for the picture H.264 makes with one.
    pub fn bits_against_h264(self) -> f64 {
        match self {
            VideoCodec::H264 => 1.0,
            VideoCodec::H265 => 0.65,
            VideoCodec::Av1 => 0.55,
        }
    }
}

/// How the sound is compressed (or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    /// Plays everywhere.
    Aac,
    /// Better than AAC at the same size; browsers and video sites take it.
    Opus,
    /// Lossless, about half the size of the plain samples.
    Flac,
    /// Plain 24-bit samples, lossless (WAV).
    Pcm24,
    /// The oldest and most widely played.
    Mp3,
    /// An older open codec, for players that don't know Opus.
    Vorbis,
}

impl AudioCodec {
    pub const ALL: [AudioCodec; 6] = [
        AudioCodec::Aac,
        AudioCodec::Opus,
        AudioCodec::Flac,
        AudioCodec::Pcm24,
        AudioCodec::Mp3,
        AudioCodec::Vorbis,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Aac => "AAC",
            AudioCodec::Opus => "Opus",
            AudioCodec::Flac => "FLAC",
            AudioCodec::Pcm24 => "PCM 24-bit",
            AudioCodec::Mp3 => "MP3",
            AudioCodec::Vorbis => "Vorbis",
        }
    }

    /// Whether every sample comes back exactly (no bit rate to choose).
    pub fn lossless(self) -> bool {
        matches!(self, AudioCodec::Flac | AudioCodec::Pcm24)
    }

    /// The encoders, in the order tried (FFmpeg's names). Opus is FFmpeg's
    /// own encoder only when the library isn't there.
    pub fn encoders(self) -> &'static [&'static str] {
        match self {
            AudioCodec::Aac => &["aac"],
            AudioCodec::Opus => &["libopus", "opus"],
            AudioCodec::Flac => &["flac"],
            AudioCodec::Pcm24 => &["pcm_s24le"],
            AudioCodec::Mp3 => &["libmp3lame"],
            AudioCodec::Vorbis => &["libvorbis"],
        }
    }

    /// The sample rate the sound is written at for a timeline at `wanted`:
    /// the same, except for Opus, which only takes 48 kHz (or less) and is
    /// mixed at 48 kHz.
    pub fn sample_rate_for(self, wanted: u32) -> u32 {
        match self {
            AudioCodec::Opus => 48_000,
            AudioCodec::Mp3 if !MP3_RATES.contains(&wanted) => 48_000,
            _ => wanted,
        }
    }
}

/// The rates an MP3 can have (MPEG-1 and MPEG-2).
const MP3_RATES: [u32; 6] = [48_000, 44_100, 32_000, 24_000, 22_050, 16_000];

/// The kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    /// Video: plays everywhere.
    Mp4,
    /// Video: holds any codec, including lossless sound.
    Mkv,
    /// Sound only, lossless.
    Wav,
    /// Sound only.
    Mp3,
    /// Sound only (Opus or Vorbis).
    Ogg,
}

impl Container {
    pub const ALL: [Container; 5] = [
        Container::Mp4,
        Container::Mkv,
        Container::Wav,
        Container::Mp3,
        Container::Ogg,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
            Container::Wav => "WAV",
            Container::Mp3 => "MP3",
            Container::Ogg => "OGG",
        }
    }

    /// The file name's ending, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Wav => "wav",
            Container::Mp3 => "mp3",
            Container::Ogg => "ogg",
        }
    }

    /// The container a file name's ending stands for.
    pub fn from_extension(ending: &str) -> Option<Container> {
        let ending = ending.to_ascii_lowercase();
        Container::ALL
            .into_iter()
            .find(|c| c.extension() == ending)
            .or_else(|| (ending == "opus" || ending == "oga").then_some(Container::Ogg))
    }

    /// Whether it holds a picture: MP4 and MKV always do, the others never.
    pub fn has_video(self) -> bool {
        matches!(self, Container::Mp4 | Container::Mkv)
    }

    /// The picture codecs it can hold (none for sound-only files).
    pub fn video_codecs(self) -> &'static [VideoCodec] {
        if self.has_video() { &VideoCodec::ALL } else { &[] }
    }

    /// The sound codecs it can hold, the usual one first.
    ///
    /// FLAC goes in MKV only: FFmpeg writes it into MP4 too, but many
    /// players and editors don't read it there, and lossless sound with a
    /// picture is for keeping, which MKV is for.
    pub fn audio_codecs(self) -> &'static [AudioCodec] {
        match self {
            Container::Mp4 => &[AudioCodec::Aac, AudioCodec::Opus],
            Container::Mkv => &[AudioCodec::Aac, AudioCodec::Opus, AudioCodec::Flac],
            Container::Wav => &[AudioCodec::Pcm24],
            Container::Mp3 => &[AudioCodec::Mp3],
            Container::Ogg => &[AudioCodec::Opus, AudioCodec::Vorbis],
        }
    }

    /// Whether a file of this kind can hold this picture (`None`: no
    /// picture) and this sound.
    pub fn allows(self, video: Option<VideoCodec>, audio: AudioCodec) -> bool {
        let picture = match video {
            Some(codec) => self.video_codecs().contains(&codec),
            None => !self.has_video(),
        };
        picture && self.audio_codecs().contains(&audio)
    }

    /// FFmpeg's name for the muxer.
    pub(crate) fn muxer(self) -> &'static std::ffi::CStr {
        match self {
            Container::Mp4 => c"mp4",
            Container::Mkv => c"matroska",
            Container::Wav => c"wav",
            Container::Mp3 => c"mp3",
            Container::Ogg => c"ogg",
        }
    }
}

/// Every combination a file can be written in, video files first: the
/// container, its picture codec (none for sound only) and its sound codec.
pub fn combinations() -> Vec<(Container, Option<VideoCodec>, AudioCodec)> {
    let mut all = Vec::new();
    for container in Container::ALL {
        let videos: Vec<Option<VideoCodec>> = if container.has_video() {
            container.video_codecs().iter().copied().map(Some).collect()
        } else {
            vec![None]
        };
        for video in videos {
            for &audio in container.audio_codecs() {
                all.push((container, video, audio));
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_combination_is_allowed_and_nothing_else() {
        let all = combinations();
        assert_eq!(all.len(), 3 * 2 + 3 * 3 + 1 + 1 + 2);
        for container in Container::ALL {
            for video in [
                None,
                Some(VideoCodec::H264),
                Some(VideoCodec::H265),
                Some(VideoCodec::Av1),
            ] {
                for audio in AudioCodec::ALL {
                    assert_eq!(
                        container.allows(video, audio),
                        all.contains(&(container, video, audio)),
                        "{container:?} {video:?} {audio:?}"
                    );
                }
            }
        }
        // Sound-only files hold no picture; video files always have one.
        assert!(!Container::Wav.allows(Some(VideoCodec::H264), AudioCodec::Pcm24));
        assert!(!Container::Mp4.allows(None, AudioCodec::Aac));
        assert!(!Container::Mp4.allows(Some(VideoCodec::H264), AudioCodec::Flac));
    }

    #[test]
    fn endings_name_their_containers() {
        for container in Container::ALL {
            assert_eq!(Container::from_extension(container.extension()), Some(container));
        }
        assert_eq!(Container::from_extension("MKV"), Some(Container::Mkv));
        assert_eq!(Container::from_extension("opus"), Some(Container::Ogg));
        assert_eq!(Container::from_extension("avi"), None);
    }

    #[test]
    fn the_processor_is_always_the_last_encoder_tried() {
        for codec in VideoCodec::ALL {
            let encoders = codec.encoders();
            assert_eq!(encoders.last().map(|e| e.place), Some(Place::Processor));
            assert_eq!(encoders.iter().filter(|e| e.place == Place::Processor).count(), 1);
        }
    }

    #[test]
    fn opus_is_written_at_48_khz_and_others_at_the_timelines_rate() {
        assert_eq!(AudioCodec::Opus.sample_rate_for(44_100), 48_000);
        assert_eq!(AudioCodec::Aac.sample_rate_for(44_100), 44_100);
        assert_eq!(AudioCodec::Mp3.sample_rate_for(44_100), 44_100);
        assert_eq!(AudioCodec::Mp3.sample_rate_for(96_000), 48_000);
        assert_eq!(AudioCodec::Flac.sample_rate_for(96_000), 96_000);
    }
}
