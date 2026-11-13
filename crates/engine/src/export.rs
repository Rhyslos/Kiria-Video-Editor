//! Exporting: the timeline, or a stretch of it, written as one file.
//!
//! A worker of its own asks for the timeline's picture frame by frame (from
//! a preview worker of its own, so pictures are decoded and put together
//! exactly as in the viewer, at the timeline's full size, from the
//! recordings themselves: never proxies, never a key frame standing in for
//! the exact one), reads each back from the graphics card as the planes an
//! encoder takes, mixes the sound for the same stretch exactly as playback
//! does (the same feeder and real-time mixer, track effects and master
//! included: [`crate::offline`]), and hands both to `media::encode`. A sound-only file (WAV, MP3,
//! OGG) needs no pictures and no graphics card. The editor stays usable
//! meanwhile; [`Export`] says how far it is and can be told to stop.
//!
//! What can be written is listed by `media::encode` ([`Container`],
//! [`VideoCodec`], [`AudioCodec`], [`combinations`]); what this machine can
//! encode is [`Encoders::detect`], to be called on a worker.
//!
//! The file is written beside its final place under another name and only
//! takes its name when it is complete, so a cancelled or failed export
//! leaves nothing that looks like a video.
//!
//! The file's pictures can be of another size than the timeline's (the
//! timeline's picture is fitted inside, with black bars where the shapes
//! differ: [`picture_size`], [`letterbox`]) and at another frame rate (each
//! of the file's frames shows the timeline at that frame's start). The
//! sound can also be written beside the file as 24-bit WAV, the same
//! samples. An export can be paused between frames ([`Export::pause`]).

use crate::compose::Composing;
use crate::offline::OfflineMix;
use crate::preview::{Convert, OnDevice, Picture, Preview};
use audio::settings::{LimiterSettings, MasterSettings};
use diag::{AppError, Explain};
use media::Decoding;
pub use media::encode::{AudioCodec, Container, VideoCodec, combinations};
use media::encode::{AudioSettings, Place, Probed, VideoSettings, Writer};
use project::{Project, Rate, Sequence, SequenceId, Time};
use render::Reader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How large the file may get for how good it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// For keeping, or for a site that encodes it again.
    High,
    #[default]
    Medium,
    /// For sending.
    Small,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::High, Quality::Medium, Quality::Small];

    pub fn label(self) -> &'static str {
        match self {
            Quality::High => "High",
            Quality::Medium => "Medium",
            Quality::Small => "Small",
        }
    }

    /// Bits for each pixel of each frame.
    fn bits_a_pixel(self) -> f64 {
        match self {
            Quality::High => 0.24,
            Quality::Medium => 0.14,
            Quality::Small => 0.07,
        }
    }

    /// Bits a second for H.264 pictures of `size` at `rate`.
    pub fn bit_rate(self, size: (u32, u32), rate: Rate) -> u64 {
        let pixels = f64::from(size.0) * f64::from(size.1);
        // Twice the frames don't need twice the bits: each differs less
        // from the last.
        let frames = rate.as_f64().max(1.0);
        let rate_factor = 30.0 * (frames / 30.0).powf(0.6);
        (pixels * rate_factor * self.bits_a_pixel())
            .round()
            .max(200_000.0) as u64
    }

    /// Bits a second for `codec` pictures of `size` at `rate`: what H.264
    /// takes, less for the newer codecs, which need fewer for the same
    /// picture.
    pub fn video_bit_rate(self, codec: VideoCodec, size: (u32, u32), rate: Rate) -> u64 {
        ((self.bit_rate(size, rate) as f64 * codec.bits_against_h264()).round() as u64).max(200_000)
    }

    /// Bits a second for stereo `codec` sound; `None` for lossless sound,
    /// whose size depends on the sound itself.
    pub fn audio_bit_rate(self, codec: AudioCodec) -> Option<u64> {
        let kilobits = match (codec, self) {
            (AudioCodec::Flac | AudioCodec::Pcm24, _) => return None,
            (AudioCodec::Aac, Quality::High) => 256,
            (AudioCodec::Aac, Quality::Medium) => 192,
            (AudioCodec::Aac, Quality::Small) => 128,
            (AudioCodec::Opus, Quality::High) => 192,
            (AudioCodec::Opus, Quality::Medium) => 128,
            (AudioCodec::Opus, Quality::Small) => 96,
            (AudioCodec::Mp3, Quality::High) => 320,
            (AudioCodec::Mp3, Quality::Medium) => 192,
            (AudioCodec::Mp3, Quality::Small) => 128,
            (AudioCodec::Vorbis, Quality::High) => 256,
            (AudioCodec::Vorbis, Quality::Medium) => 192,
            (AudioCodec::Vorbis, Quality::Small) => 128,
        };
        Some(kilobits * 1000)
    }
}

/// What to export, and where to.
///
/// The container, picture codec and sound codec must go together
/// ([`Container::allows`]; [`ExportSettings::check`] says why not); which
/// of those this machine can encode is [`Encoders`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    /// The file to write; it is replaced if it is there. Its ending is the
    /// page's business: the container is the one named here.
    pub path: PathBuf,
    /// A stretch of the timeline, or all of it.
    pub range: Option<(Time, Time)>,
    pub container: Container,
    /// The picture codec; `None` for a sound-only file (WAV, MP3, OGG).
    pub video: Option<VideoCodec>,
    pub audio: AudioCodec,
    /// Sets the picture's and the sound's bit rates (not the lossless sound's).
    pub quality: Quality,
    /// Encode the picture on the processor even when a graphics card could.
    pub software: bool,
    /// Turn the sound up or down to this integrated loudness (LUFS): the
    /// whole stretch is mixed and measured first, then exported with the
    /// gain that brings it there. `None` exports the mix as it is.
    pub normalise: Option<f32>,
    /// Keep the sound's true peak under [`TRUE_PEAK_CEILING`] with a
    /// limiter on the master (wanted with `normalise`, which can push peaks
    /// over). Off, the mix is exported sample for sample as it plays.
    pub limit: bool,
    /// The size of the file's pictures (made even); `None` for the
    /// timeline's own. The timeline's picture is fitted inside, with black
    /// bars where the shapes differ, enlarged when the size is larger.
    pub size: Option<(u32, u32)>,
    /// The file's frame rate; `None` for the timeline's. Each frame shows
    /// the timeline at its own start.
    pub rate: Option<Rate>,
    /// Also write the sound beside the file as 24-bit WAV, the same samples
    /// (nothing more for a WAV file): [`ExportSettings::sound_beside_path`].
    pub sound_beside: bool,
}

/// The highest true peak a limited export lets through, dBTP: safe for
/// files played or encoded again.
pub const TRUE_PEAK_CEILING: f32 = -1.0;
/// The smallest and largest side of an exported picture, pixels.
pub const MIN_SIDE: u32 = 16;
pub const MAX_SIDE: u32 = 8192;
/// The most normalising turns the sound up, dB: more would mostly raise
/// noise and hum in something nearly silent.
pub const MOST_GAIN_DB: f32 = 20.0;

impl ExportSettings {
    /// An MP4 with H.264 and AAC at Medium quality: plays everywhere.
    pub fn mp4(path: PathBuf) -> ExportSettings {
        ExportSettings {
            path,
            range: None,
            container: Container::Mp4,
            video: Some(VideoCodec::H264),
            audio: AudioCodec::Aac,
            quality: Quality::Medium,
            software: false,
            normalise: None,
            limit: false,
            size: None,
            rate: None,
            sound_beside: false,
        }
    }

    /// Where the sound is written beside the file, when it is: the same
    /// name ending in `.wav`.
    pub fn sound_beside_path(&self) -> Option<PathBuf> {
        (self.sound_beside && self.container != Container::Wav).then(|| self.path.with_extension("wav"))
    }

    /// The size of the file's pictures for a timeline of `timeline`: the
    /// size asked for, or the timeline's own, even on both sides.
    pub fn frame_size(&self, timeline: (u32, u32)) -> (u32, u32) {
        even(self.size.unwrap_or(timeline))
    }

    /// Whether the container holds this picture and sound, with the reason
    /// when it doesn't.
    pub fn check(&self) -> diag::Result<()> {
        if let Some((width, height)) = self.size
            && ![width, height]
                .iter()
                .all(|side| (MIN_SIDE..=MAX_SIDE).contains(side))
        {
            return Err(AppError::new("The file can't be exported at that size")
                .why(format!(
                    "Its pictures would be {width}×{height}; each side must be {MIN_SIDE} to {MAX_SIDE} pixels."
                ))
                .fix("Choose another size, or the timeline's own."));
        }
        if self.container.allows(self.video, self.audio) {
            return Ok(());
        }
        let what = match self.video {
            Some(video) => format!("{} pictures with {} sound", video.label(), self.audio.label()),
            None => format!("{} sound alone", self.audio.label()),
        };
        Err(AppError::new("The file can't be exported")
            .why(format!("A {} file can't hold {what}.", self.container.label()))
            .fix(format!(
                "Choose one of the sound formats a {} file takes: {}.",
                self.container.label(),
                self.container
                    .audio_codecs()
                    .iter()
                    .map(|c| c.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
    }

    /// About how many bytes `length` of the timeline (pictures of `size` at
    /// `rate`, sound at `sample_rate`) comes to, in the file alone. `size`
    /// and `rate` are the file's: [`ExportSettings::frame_size`], and the
    /// rate asked for or the timeline's.
    pub fn estimated_bytes(&self, size: (u32, u32), rate: Rate, sample_rate: u32, length: Duration) -> u64 {
        let picture = self
            .video
            .map_or(0, |codec| self.quality.video_bit_rate(codec, size, rate));
        let sound_rate = self.audio.sample_rate_for(sample_rate);
        let sound = self.quality.audio_bit_rate(self.audio).unwrap_or_else(|| {
            // 24-bit stereo; FLAC keeps about 60 % of it for mixed sound.
            let plain = u64::from(sound_rate) * 24 * 2;
            if self.audio == AudioCodec::Flac {
                plain * 6 / 10
            } else {
                plain
            }
        });
        ((picture + sound) as f64 * length.as_secs_f64() / 8.0).round() as u64
    }

    /// About how many bytes the WAV written beside the file comes to (none
    /// when it isn't).
    pub fn estimated_beside_bytes(&self, sample_rate: u32, length: Duration) -> u64 {
        if self.sound_beside_path().is_none() {
            return 0;
        }
        let rate = self.audio.sample_rate_for(sample_rate);
        (f64::from(rate) * 6.0 * length.as_secs_f64()).round() as u64
    }
}

/// What this machine can encode: for each picture codec the graphics card's
/// encoder that opened and the processor's, for each sound codec its
/// encoder. Found with [`Encoders::detect`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Encoders {
    video: Vec<(VideoCodec, Probed)>,
    audio: Vec<(AudioCodec, Option<&'static str>)>,
}

impl Encoders {
    /// Tries every graphics card encoder this build has, briefly: up to a
    /// second in all on a machine with one. Call it on a worker (or use
    /// [`Encoders::detect_in_background`]), never on the interface thread.
    pub fn detect() -> Encoders {
        Encoders {
            video: VideoCodec::ALL
                .into_iter()
                .map(|codec| (codec, media::encode::probe_video(codec)))
                .collect(),
            audio: AudioCodec::ALL
                .into_iter()
                .map(|codec| (codec, media::encode::audio_encoder(codec)))
                .collect(),
        }
    }

    /// [`Encoders::detect`] on a thread of its own; the answer arrives on
    /// the receiver, and `notify` runs once it is there.
    pub fn detect_in_background(notify: impl FnOnce() + Send + 'static) -> Receiver<Encoders> {
        let (tell, answer) = channel();
        let spawned = std::thread::Builder::new()
            .name("encoders".into())
            .spawn(move || {
                let _ = tell.send(Encoders::detect());
                notify();
            });
        if let Err(err) = spawned {
            tracing::error!("the encoder check could not start: {err}");
        }
        answer
    }

    /// The encoders found for a picture codec.
    pub fn video(&self, codec: VideoCodec) -> Probed {
        self.video
            .iter()
            .find(|(c, _)| *c == codec)
            .map(|(_, probed)| *probed)
            .unwrap_or_default()
    }

    /// The encoder found for a sound codec (FFmpeg's name).
    pub fn audio(&self, codec: AudioCodec) -> Option<&'static str> {
        self.audio
            .iter()
            .find(|(c, _)| *c == codec)
            .and_then(|(_, name)| *name)
    }

    /// Whether this combination can be exported here (on the processor
    /// only, with `software`).
    pub fn can(
        &self,
        container: Container,
        video: Option<VideoCodec>,
        audio: AudioCodec,
        software: bool,
    ) -> bool {
        let picture = video.is_none_or(|codec| {
            let probed = self.video(codec);
            probed.processor.is_some() || (!software && probed.graphics_card.is_some())
        });
        container.allows(video, audio) && picture && self.audio(audio).is_some()
    }

    /// Every combination this machine can export, in [`combinations`]' order.
    pub fn combinations(&self) -> Vec<(Container, Option<VideoCodec>, AudioCodec)> {
        combinations()
            .into_iter()
            .filter(|&(container, video, audio)| self.can(container, video, audio, false))
            .collect()
    }
}

/// How an encoder is said in a few words: where it works.
pub fn encoder_place(name: &str) -> Place {
    VideoCodec::ALL
        .into_iter()
        .flat_map(|codec| codec.encoders().iter())
        .find(|e| e.name == name)
        .map_or(Place::Processor, |e| e.place)
}

/// How far an export is.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub frames_done: u64,
    pub frames: u64,
    pub elapsed: Duration,
    /// Which encoder makes the pictures, once it is known (FFmpeg's name).
    pub encoder: Option<String>,
    /// The sound is being measured for normalising, before anything is
    /// written: `frames_done` counts that pass.
    pub measuring: bool,
    /// Paused between frames ([`Export::pause`]); `elapsed` stands still.
    pub paused: bool,
}

impl Progress {
    /// 0 to 1.
    pub fn share(&self) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        (self.frames_done as f64 / self.frames as f64).min(1.0) as f32
    }

    /// How long the rest should take at the pace so far. `None` until
    /// there is a pace to go by.
    pub fn time_left(&self) -> Option<Duration> {
        if self.measuring || self.paused || self.frames_done < 10 || self.frames_done >= self.frames {
            return None;
        }
        let each = self.elapsed.as_secs_f64() / self.frames_done as f64;
        Some(Duration::from_secs_f64(
            each * (self.frames - self.frames_done) as f64,
        ))
    }
}

/// How an export ended well.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub path: PathBuf,
    pub frames: u64,
    pub took: Duration,
    /// The length of what was exported.
    pub length: Duration,
    /// The size of its pictures (none for sound alone).
    pub size: Option<(u32, u32)>,
    /// The sound written beside it as WAV, when asked for.
    pub sound_beside: Option<PathBuf>,
    /// The picture's encoder (FFmpeg's name), or the sound's for a
    /// sound-only file.
    pub encoder: String,
    /// The sound's encoder (FFmpeg's name).
    pub audio_encoder: String,
    pub on_graphics_card: bool,
    /// The sound's integrated loudness (LUFS) and highest true peak
    /// (dBTP), measured as it was written (`-inf` for silence).
    pub loudness: f32,
    pub true_peak: f32,
    /// What normalising found and did, when asked for.
    pub normalised: Option<Normalised>,
    /// The deepest the limiter turned the sound down, dB (0 when it never
    /// had to), when it was on.
    pub limited_db: Option<f32>,
}

/// What normalising found and did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normalised {
    /// The mix's integrated loudness before, LUFS (`-inf` for silence).
    pub measured: f32,
    /// The gain applied, dB.
    pub gain_db: f32,
}

impl Normalised {
    /// The gain that brings `measured` to `target`: none for silence, and
    /// at most [`MOST_GAIN_DB`] up.
    pub fn for_target(measured: f32, target: f32) -> Normalised {
        let gain_db = if measured.is_finite() {
            (target - measured).min(MOST_GAIN_DB)
        } else {
            0.0
        };
        Normalised { measured, gain_db }
    }
}

/// How an export ended.
#[derive(Debug)]
pub enum Outcome {
    Done(Exported),
    /// Stopped when asked to; nothing is left behind.
    Cancelled,
    Failed(AppError),
}

/// What the worker needs of the editor: how video is decoded and where
/// pictures are put together, as the viewer's own worker gets them.
#[derive(Debug, Clone)]
pub struct Devices {
    pub decoding: Decoding,
    pub convert: Convert,
    /// The window's graphics device, when video may be decoded on it.
    pub on_device: Option<OnDevice>,
    /// The device the window is drawn with: pictures are put together there.
    pub composing: Composing,
}

/// Shared between the worker and whoever watches it.
#[derive(Debug, Default)]
struct Shared {
    frames_done: AtomicU64,
    frames: AtomicU64,
    measuring: AtomicBool,
    cancel: AtomicBool,
    paused: AtomicBool,
    /// Time spent paused: what is behind, and since when it is paused now.
    pauses: Mutex<(Duration, Option<Instant>)>,
    encoder: Mutex<Option<String>>,
}

impl Shared {
    fn pause(&self, on: bool) {
        if let Ok(mut pauses) = self.pauses.lock() {
            let (behind, since) = &mut *pauses;
            match (on, *since) {
                (true, None) => *since = Some(Instant::now()),
                (false, Some(from)) => {
                    *behind += from.elapsed();
                    *since = None;
                }
                _ => {}
            }
        }
        self.paused.store(on, Ordering::Relaxed);
    }

    /// The time since `started` not spent paused.
    fn working(&self, started: Instant) -> Duration {
        let paused = self.pauses.lock().map_or(Duration::ZERO, |pauses| {
            pauses.0 + pauses.1.map_or(Duration::ZERO, |since| since.elapsed())
        });
        started.elapsed().saturating_sub(paused)
    }

    /// Waits while paused. False when cancelled.
    fn carry_on(&self) -> bool {
        while self.paused.load(Ordering::Relaxed) && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(40));
        }
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// An export under way.
#[derive(Debug)]
pub struct Export {
    shared: Arc<Shared>,
    started: Instant,
    outcome: Receiver<Outcome>,
    path: PathBuf,
}

impl Export {
    /// Starts exporting `sequence` of `project` as it is now (later edits
    /// don't change what is exported). `devices` are needed for a file with
    /// pictures; a sound-only file is made without them. `notify` runs on
    /// the worker now and then while it works and when it ends.
    pub fn start(
        project: Project,
        sequence: SequenceId,
        settings: ExportSettings,
        devices: Option<Devices>,
        notify: impl Fn() + Send + 'static,
    ) -> Export {
        let shared = Arc::new(Shared::default());
        let (tell, outcome) = channel();
        let path = settings.path.clone();
        let worker = shared.clone();
        let spawned = std::thread::Builder::new().name("export".into()).spawn(move || {
            let parts = Parts::of(&settings);
            let outcome = match run(&project, sequence, &settings, &parts, devices, &worker, &notify) {
                Ok(Some(done)) => match parts.name() {
                    Ok(()) => Outcome::Done(done),
                    Err(err) => Outcome::Failed(err),
                },
                Ok(None) => {
                    parts.remove();
                    Outcome::Cancelled
                }
                Err(err) => {
                    parts.remove();
                    Outcome::Failed(err)
                }
            };
            let _ = tell.send(outcome);
            notify();
        });
        if let Err(err) = spawned {
            tracing::error!("the export worker could not start: {err}");
        }
        Export {
            shared,
            started: Instant::now(),
            outcome,
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn progress(&self) -> Progress {
        Progress {
            frames_done: self.shared.frames_done.load(Ordering::Relaxed),
            frames: self.shared.frames.load(Ordering::Relaxed),
            elapsed: self.shared.working(self.started),
            encoder: self.shared.encoder.lock().ok().and_then(|name| name.clone()),
            measuring: self.shared.measuring.load(Ordering::Relaxed),
            paused: self.is_paused(),
        }
    }

    /// Asks the worker to stop; it does so before the next frame (also
    /// when paused).
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// Holds the worker before its next frame until [`Export::resume`]; its
    /// decoders and the file stay open. Time paused doesn't count in
    /// `elapsed` or `took`.
    pub fn pause(&self) {
        self.shared.pause(true);
    }

    pub fn resume(&self) {
        self.shared.pause(false);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// How it ended, once it has. Asked again after that, nothing.
    pub fn finished(&self) -> Option<Outcome> {
        match self.outcome.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Outcome::Failed(
                AppError::new("The export stopped unexpectedly")
                    .why("Its worker ended without saying how it went.")
                    .fix("Export again. If it keeps happening, open Diagnostics and report the problem."),
            )),
        }
    }
}

/// Where the file is written until it is complete: beside its final place,
/// with the same ending so the container is the same.
fn part_path(path: &Path) -> PathBuf {
    let ending = path.extension().and_then(|e| e.to_str()).unwrap_or("part");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
    path.with_file_name(format!("{stem}.exporting.{ending}"))
}

/// The files an export writes under other names until all are complete:
/// the file, and the sound beside it when asked for.
struct Parts {
    file: (PathBuf, PathBuf),
    beside: Option<(PathBuf, PathBuf)>,
}

impl Parts {
    fn of(settings: &ExportSettings) -> Parts {
        let pair = |path: PathBuf| (part_path(&path), path);
        Parts {
            file: pair(settings.path.clone()),
            beside: settings.sound_beside_path().map(pair),
        }
    }

    /// Gives each its name.
    fn name(&self) -> diag::Result<()> {
        for (part, path) in std::iter::once(&self.file).chain(&self.beside) {
            std::fs::rename(part, path).or_explain(|| {
                AppError::new("The exported file could not be given its name")
                    .why(format!(
                        "It is complete, as {}, but could not be renamed to {}.",
                        part.display(),
                        path.display()
                    ))
                    .fix("Close anything that has the old file open, then rename the new one yourself.")
            })?;
        }
        Ok(())
    }

    fn remove(&self) {
        for (part, _) in std::iter::once(&self.file).chain(&self.beside) {
            let _ = std::fs::remove_file(part);
        }
    }
}

/// `size` made even on both sides (encoders take even sizes), at least 2.
fn even(size: (u32, u32)) -> (u32, u32) {
    ((size.0 & !1).max(2), (size.1 & !1).max(2))
}

/// The size the timeline's picture takes in a file of `frame`: the largest
/// of its shape that fits, even on both sides (an even margin each side
/// when centred, so the colour planes line up too).
pub fn picture_size(timeline: (u32, u32), frame: (u32, u32)) -> (u32, u32) {
    let frame = even(frame);
    let (w, h) = render::Area::fitted(timeline, frame).size();
    let fit = |side: f32, room: u32| {
        // The margin on each side even: the picture's side has the frame's
        // evenness (frame is even, so the picture is too, in steps of 4
        // when the margin is to be split into two even halves).
        let side = (side.round() as u32).min(room);
        let margin = ((room - side) / 2) & !1;
        room - 2 * margin
    };
    (fit(w, frame.0).max(2), fit(h, frame.1).max(2))
}

/// Puts `picture` (NV12, `size`) in the middle of a black NV12 frame of
/// `frame` (both even on both sides, the picture no larger than the frame)
/// into `out`.
pub fn letterbox(picture: &[u8], size: (u32, u32), frame: (u32, u32), out: &mut Vec<u8>) {
    let (pw, ph) = (size.0 as usize, size.1 as usize);
    let (fw, fh) = (frame.0 as usize, frame.1 as usize);
    out.clear();
    out.resize(fw * fh, 16);
    out.resize(fw * fh * 3 / 2, 128);
    if pw > fw || ph > fh || picture.len() < pw * ph * 3 / 2 {
        return;
    }
    let (x, y) = (((fw - pw) / 2) & !1, ((fh - ph) / 2) & !1);
    let (luma, chroma) = picture.split_at(pw * ph);
    let (out_luma, out_chroma) = out.split_at_mut(fw * fh);
    for row in 0..ph {
        let to = (y + row) * fw + x;
        out_luma[to..to + pw].copy_from_slice(&luma[row * pw..(row + 1) * pw]);
    }
    // Colour: half the rows, each a pair of bytes for every two pixels.
    for row in 0..ph / 2 {
        let to = (y / 2 + row) * fw + x;
        out_chroma[to..to + pw].copy_from_slice(&chroma[row * pw..(row + 1) * pw]);
    }
}

/// The frames `[first, end)` of the timeline to export.
fn frames(length: Time, range: Option<(Time, Time)>, rate: Rate) -> (i64, i64) {
    let last = rate.frame_at(length);
    // A timeline that ends inside a frame includes that frame.
    let end = if rate.frame_start(last) < length {
        last + 1
    } else {
        last
    };
    match range {
        Some((from, to)) => {
            let first = rate.frame_at(from.max(Time::ZERO)).clamp(0, end);
            (first, rate.frame_at(to).clamp(first, end))
        }
        None => (0, end),
    }
}

/// How long a frame may take before the export gives up on it.
const FRAME_PATIENCE: Duration = Duration::from_secs(60);

fn cannot(why: impl Into<String>) -> AppError {
    AppError::new("The video can't be exported").why(why)
}

/// Does the export. `Ok(None)` when it was cancelled.
fn run(
    project: &Project,
    sequence: SequenceId,
    settings: &ExportSettings,
    parts: &Parts,
    devices: Option<Devices>,
    shared: &Shared,
    notify: &dyn Fn(),
) -> diag::Result<Option<Exported>> {
    let started = Instant::now();
    settings.check()?;
    let seq = project
        .sequences
        .iter()
        .find(|s| s.id == sequence)
        .ok_or_else(|| cannot("The timeline is not in the project any more."))?;
    // The file's frames: each shows the timeline at its own start.
    let rate = settings.rate.unwrap_or(seq.settings.frame_rate);
    let (first, end) = frames(seq.length(), settings.range, rate);
    let count = (end - first).max(0) as u64;
    if count == 0 {
        return Err(cannot("There is nothing on that stretch of the timeline.")
            .fix("Put a clip on the timeline, or export the whole timeline instead of the region."));
    }
    shared.frames.store(count, Ordering::Relaxed);

    let timeline_size = (seq.settings.width, seq.settings.height);
    // The file's picture size, and the timeline's picture fitted inside.
    let frame_size = settings.frame_size(timeline_size);
    let picture_size = picture_size(timeline_size, frame_size);
    let sample_rate = settings
        .audio
        .sample_rate_for(seq.settings.sample_rate.max(8_000));
    // Pictures, when the file has them: read back on the device they are
    // put together on, at the size they take in the file (put together at
    // that size, or at the timeline's and enlarged as they are read back).
    let pictures = match settings.video {
        Some(codec) => {
            let devices = devices.ok_or_else(|| {
                cannot("This window isn't drawn on a graphics card, so pictures can't be put together for a video here.")
                    .fix("Export sound only (WAV, MP3 or OGG), or start the app on a machine with a working graphics driver.")
            })?;
            let reader = Reader::new(&devices.composing.device, picture_size);
            Some((codec, devices, reader))
        }
        None => None,
    };
    let video = pictures.as_ref().map(|(codec, _, _)| VideoSettings {
        codec: *codec,
        width: frame_size.0,
        height: frame_size.1,
        rate: (rate.num(), rate.den()),
        bit_rate: settings.quality.video_bit_rate(*codec, frame_size, rate),
        software: settings.software,
    });
    let audio = AudioSettings {
        codec: settings.audio,
        sample_rate,
        bit_rate: settings.quality.audio_bit_rate(settings.audio).unwrap_or(0),
    };
    let mut writer = Writer::create(&parts.file.0, settings.container, video.as_ref(), Some(&audio))?;
    // The sound beside it: the same samples, as 24-bit WAV.
    let mut beside = match &parts.beside {
        Some((part, _)) => Some(Writer::create(
            part,
            Container::Wav,
            None,
            Some(&AudioSettings {
                codec: AudioCodec::Pcm24,
                sample_rate,
                bit_rate: 0,
            }),
        )?),
        None => None,
    };
    let audio_encoder = writer.audio_encoder().unwrap_or_default().to_owned();
    let encoder = writer
        .video_encoder()
        .map_or_else(|| audio_encoder.clone(), str::to_owned);
    let on_graphics_card = writer.on_graphics_card();
    if let Ok(mut name) = shared.encoder.lock() {
        *name = Some(encoder.clone());
    }
    tracing::info!(
        "exporting {count} frames as {} ({:?} at {} bits a second with {encoder}, {} with {audio_encoder} at {sample_rate} Hz)",
        settings.container.label(),
        video.map(|v| (v.width, v.height)),
        video.map_or(0, |v| v.bit_rate),
        settings.audio.label(),
    );

    // A worker of its own, so the viewer keeps its decoders and its place.
    // Nothing is kept on the card for going back to: export only goes
    // forward. It is never handed the proxies, and waits for each exact
    // frame (no key frame shown first).
    let mut picturing = pictures.map(|(_, devices, reader)| {
        let (device, queue) = (devices.composing.device.clone(), devices.composing.queue.clone());
        let on_device = devices.on_device.map(|on_device| OnDevice {
            frames_kept: 0,
            ..on_device
        });
        let mut preview = Preview::start_composing(
            devices.decoding,
            devices.convert,
            on_device,
            Some(devices.composing),
            || {},
        );
        preview.set_key_frame_first(false);
        let black = render::reader::black(frame_size);
        let picture = Vec::with_capacity(reader.bytes());
        Picturing {
            preview,
            reader,
            device,
            queue,
            black,
            picture,
            boxed: Vec::new(),
        }
    });
    let start = rate.frame_start(first);
    // Normalising: the whole stretch is mixed and measured first.
    let normalised = match settings.normalise {
        Some(target) => {
            shared.measuring.store(true, Ordering::Relaxed);
            let measured = match measure(project, seq, (start, rate, sample_rate, count), shared, notify) {
                Ok(Some(measured)) => measured,
                // Cancelled, or the mixer couldn't be made.
                other => {
                    if let Some(p) = &mut picturing {
                        p.preview.close(Duration::from_secs(5));
                    }
                    return other.map(|_| None);
                }
            };
            shared.measuring.store(false, Ordering::Relaxed);
            shared.frames_done.store(0, Ordering::Relaxed);
            let normalised = Normalised::for_target(measured, target);
            tracing::info!(
                "the mix measures {measured:.1} LUFS: exported {:+.1} dB louder to aim at {target} LUFS",
                normalised.gain_db
            );
            Some(normalised)
        }
        None => None,
    };
    let master_settings = export_master(seq, normalised.map_or(0.0, |n| n.gain_db), settings.limit);
    let limiting = master_settings.limiter.on;
    let mut master = match OfflineMix::new(project, sequence, sample_rate, start, master_settings) {
        Ok(master) => master,
        Err(err) => {
            if let Some(p) = &mut picturing {
                p.preview.close(Duration::from_secs(5));
            }
            return Err(err);
        }
    };
    let mut loudness = crate::loudness::Loudness::new(sample_rate);
    let mut sound = Vec::new();
    let mut samples_written: u64 = 0;
    let result: diag::Result<bool> = (|| {
        for index in 0..count {
            if !shared.carry_on() {
                return Ok(false);
            }
            if let Some(p) = &mut picturing {
                let at = rate.frame_start(first + index as i64);
                p.preview
                    .show(project, sequence, at, picture_size.0, picture_size.1);
                let shown = wait_for(&p.preview, at)?;
                if let Some(err) = shown.error {
                    return Err(err);
                }
                if shown.refused {
                    return Err(cannot(
                        "A picture decoded on the graphics card could not be drawn from when the frame was put together.",
                    )
                    .fix("Turn off \"Decode video on the window's graphics device\" in Settings, under Graphics, and export again."));
                }
                match shown.composed {
                    Some(frame) => {
                        p.reader
                            .read(&p.device, &p.queue, frame.view(), frame.working(), &mut p.picture)
                            .map_err(|err| {
                                cannot(format!(
                                    "A finished frame could not be read back from the graphics card ({err:?})."
                                ))
                                .fix("Export again. If it keeps happening, update the graphics driver.")
                            })?;
                        if picture_size == frame_size {
                            writer.write_picture(&p.picture)?;
                        } else {
                            letterbox(&p.picture, picture_size, frame_size, &mut p.boxed);
                            writer.write_picture(&p.boxed)?;
                        }
                    }
                    // Nothing on the timeline here.
                    None => writer.write_picture(&p.black)?,
                }
            }
            // The sound up to the end of this frame, to the exact sample.
            let until = samples_until(index + 1, sample_rate, rate);
            master.take((until - samples_written) as usize, &mut sound);
            loudness.feed(&sound);
            writer.write_sound(&sound)?;
            if let Some(beside) = &mut beside {
                beside.write_sound(&sound)?;
            }
            samples_written = until;

            shared.frames_done.store(index + 1, Ordering::Relaxed);
            if index % 15 == 0 {
                notify();
            }
        }
        Ok(true)
    })();
    // The preview's decoders are closed before the file is completed or dropped.
    if let Some(p) = &mut picturing {
        p.preview.close(Duration::from_secs(5));
    }
    if !result? {
        return Ok(None);
    }
    writer.finish()?;
    if let Some(beside) = beside {
        beside.finish()?;
    }
    Ok(Some(Exported {
        path: settings.path.clone(),
        frames: count,
        took: shared.working(started),
        length: Duration::from_secs_f64(count as f64 / rate.as_f64()),
        size: video.map(|v| (v.width, v.height)),
        sound_beside: parts.beside.as_ref().map(|(_, path)| path.clone()),
        encoder,
        audio_encoder,
        on_graphics_card,
        loudness: loudness.measured().integrated,
        true_peak: loudness.measured().true_peak_max,
        normalised,
        limited_db: limiting.then(|| master.limited_db()),
    }))
}

/// How many samples of sound the first `frames` frames take, exactly.
fn samples_until(frames: u64, sample_rate: u32, rate: Rate) -> u64 {
    (u128::from(frames) * u128::from(sample_rate) * u128::from(rate.den()) / u128::from(rate.num())) as u64
}

/// The integrated loudness of `count` frames' sound from `start`, mixed as
/// the export mixes it (the timeline's master without its limiter), without
/// writing anything. `None` when cancelled.
fn measure(
    project: &Project,
    seq: &Sequence,
    (start, rate, sample_rate, count): (Time, Rate, u32, u64),
    shared: &Shared,
    notify: &dyn Fn(),
) -> diag::Result<Option<f32>> {
    let mut master = OfflineMix::new(project, seq.id, sample_rate, start, measured_master(seq))?;
    let mut loudness = crate::loudness::Loudness::new(sample_rate);
    let mut sound = Vec::new();
    let mut done = 0;
    for index in 0..count {
        if !shared.carry_on() {
            return Ok(None);
        }
        let until = samples_until(index + 1, sample_rate, rate);
        master.take((until - done) as usize, &mut sound);
        loudness.feed(&sound);
        done = until;
        shared.frames_done.store(index + 1, Ordering::Relaxed);
        if index % 60 == 0 {
            notify();
        }
    }
    Ok(Some(loudness.measured().integrated))
}

/// The timeline's master as measured for normalising and the Export
/// page's checks: everything but the limiter.
fn measured_master(seq: &Sequence) -> MasterSettings {
    let mut master = MasterSettings::from_project(seq);
    master.limiter.on = false;
    master
}

/// The master an export mixes through: the timeline's, turned up or down
/// by normalising's `gain_db` after its compressor, and with `limit` the
/// limiter on at [`TRUE_PEAK_CEILING`] (or the timeline's own, when lower).
fn export_master(seq: &Sequence, gain_db: f32, limit: bool) -> MasterSettings {
    let mut master = MasterSettings::from_project(seq);
    master.trim_db = gain_db;
    if limit {
        let own = master.limiter;
        let ceiling_db = if own.on {
            own.ceiling_db.min(TRUE_PEAK_CEILING)
        } else {
            TRUE_PEAK_CEILING
        };
        master.limiter = LimiterSettings { on: true, ceiling_db };
    }
    master
}

/// What [`measure_mix`] found: the mix's integrated loudness (LUFS) and
/// highest true peak (dBTP), `-inf` for silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixMeasured {
    pub integrated: f32,
    pub true_peak: f32,
}

/// Measures the sound of `sequence` (or a stretch of it) as an export
/// would mix it, without normalising or limiting: for saying before an
/// export how loud it is. Takes as long as mixing it does, so call it on a
/// worker. `done` is set to how far it is, in thousandths. `None` when
/// `cancel` was set or the timeline isn't there.
pub fn measure_mix(
    project: &Project,
    sequence: SequenceId,
    range: Option<(Time, Time)>,
    cancel: &AtomicBool,
    done: &AtomicU32,
) -> Option<MixMeasured> {
    let seq = project.sequences.iter().find(|s| s.id == sequence)?;
    let rate = seq.settings.frame_rate;
    let sample_rate = seq.settings.sample_rate.max(8_000);
    let (first, end) = frames(seq.length(), range, rate);
    let start = rate.frame_start(first);
    let total = samples_until((end - first).max(0) as u64, sample_rate, rate);
    let mut master = match OfflineMix::new(project, sequence, sample_rate, start, measured_master(seq)) {
        Ok(master) => master,
        Err(err) => {
            tracing::warn!("{}", err.report());
            return None;
        }
    };
    let mut loudness = crate::loudness::Loudness::new(sample_rate);
    let mut sound = Vec::new();
    // A tenth of a second at a time.
    let step = u64::from(sample_rate / 10).max(1);
    let mut mixed = 0;
    while mixed < total {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let take = step.min(total - mixed);
        master.take(take as usize, &mut sound);
        loudness.feed(&sound);
        mixed += take;
        done.store((mixed * 1000 / total.max(1)) as u32, Ordering::Relaxed);
    }
    let measured = loudness.measured();
    Some(MixMeasured {
        integrated: measured.integrated,
        true_peak: measured.true_peak_max,
    })
}

/// What making the pictures takes: a preview worker of the export's own and
/// the reader that brings its frames back from the graphics card.
struct Picturing {
    preview: Preview,
    reader: Reader,
    device: wgpu::Device,
    queue: wgpu::Queue,
    black: Vec<u8>,
    picture: Vec<u8>,
    /// The picture with bars, when the file's shape is not the timeline's.
    boxed: Vec<u8>,
}

/// The picture for the moment `at`, which was just asked for: the exact
/// one, never a picture standing in for it.
fn wait_for(preview: &Preview, at: Time) -> diag::Result<Picture> {
    let give_up = Instant::now() + FRAME_PATIENCE;
    loop {
        match preview.wait(Duration::from_millis(250)) {
            Some(picture) if picture.at == at && !picture.stand_in => return Ok(picture),
            // One for an earlier request, or one standing in: not the one waited for.
            Some(_) => {}
            None if Instant::now() >= give_up => {
                return Err(cannot("A frame was not ready after a minute.").fix(
                    "Export again. If it stops at the same place, that part of a recording may be damaged.",
                ));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests;
