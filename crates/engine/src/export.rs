//! Exporting: the timeline, or a stretch of it, written as one file.
//!
//! A worker of its own asks for the timeline's picture frame by frame (from
//! a preview worker of its own, so pictures are decoded and put together
//! exactly as in the viewer, at the timeline's full size, from the
//! recordings themselves: never proxies, never a key frame standing in for
//! the exact one), reads each back from the graphics card as the planes an
//! encoder takes, mixes the sound for the same stretch with the playback
//! mixer, and hands both to `media::encode`. A sound-only file (WAV, MP3,
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

use crate::compose::Composing;
use crate::mix::Mixer;
use crate::preview::{Convert, OnDevice, Picture, Preview};
use diag::{AppError, Explain};
use media::Decoding;
pub use media::encode::{AudioCodec, Container, VideoCodec, combinations};
use media::encode::{AudioSettings, Place, Probed, VideoSettings, Writer};
use project::{Project, Rate, SequenceId, Time};
use render::Reader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
}

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
        }
    }

    /// Whether the container holds this picture and sound, with the reason
    /// when it doesn't.
    pub fn check(&self) -> diag::Result<()> {
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
    /// `rate`, sound at `sample_rate`) comes to.
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
        if self.frames_done < 10 || self.frames_done >= self.frames {
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
    cancel: AtomicBool,
    encoder: Mutex<Option<String>>,
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
            let part = part_path(&settings.path);
            let outcome = match run(&project, sequence, &settings, &part, devices, &worker, &notify) {
                Ok(Some(done)) => match std::fs::rename(&part, &settings.path).or_explain(|| {
                    AppError::new("The exported video could not be given its name")
                        .why(format!(
                            "It is complete, as {}, but could not be renamed to {}.",
                            part.display(),
                            settings.path.display()
                        ))
                        .fix("Close anything that has the old file open, then rename the new one yourself.")
                }) {
                    Ok(()) => Outcome::Done(done),
                    Err(err) => Outcome::Failed(err),
                },
                Ok(None) => {
                    let _ = std::fs::remove_file(&part);
                    Outcome::Cancelled
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&part);
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
            elapsed: self.started.elapsed(),
            encoder: self.shared.encoder.lock().ok().and_then(|name| name.clone()),
        }
    }

    /// Asks the worker to stop; it does so before the next frame.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
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
    part: &Path,
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
    let rate = seq.settings.frame_rate;
    let (first, end) = frames(seq.length(), settings.range, rate);
    let count = (end - first).max(0) as u64;
    if count == 0 {
        return Err(cannot("There is nothing on that stretch of the timeline.")
            .fix("Put a clip on the timeline, or export the whole timeline instead of the region."));
    }
    shared.frames.store(count, Ordering::Relaxed);

    let timeline_size = (seq.settings.width, seq.settings.height);
    let sample_rate = settings
        .audio
        .sample_rate_for(seq.settings.sample_rate.max(8_000));
    // Pictures, when the file has them: read back on the device they are
    // put together on, at the timeline's full size.
    let pictures = match settings.video {
        Some(codec) => {
            let devices = devices.ok_or_else(|| {
                cannot("This window isn't drawn on a graphics card, so pictures can't be put together for a video here.")
                    .fix("Export sound only (WAV, MP3 or OGG), or start the app on a machine with a working graphics driver.")
            })?;
            let reader = Reader::new(&devices.composing.device, timeline_size);
            Some((codec, devices, reader))
        }
        None => None,
    };
    let video = pictures.as_ref().map(|(codec, _, reader)| VideoSettings {
        codec: *codec,
        width: reader.size().0,
        height: reader.size().1,
        rate: (rate.num(), rate.den()),
        bit_rate: settings.quality.video_bit_rate(*codec, reader.size(), rate),
        software: settings.software,
    });
    let audio = AudioSettings {
        codec: settings.audio,
        sample_rate,
        bit_rate: settings.quality.audio_bit_rate(settings.audio).unwrap_or(0),
    };
    let mut writer = Writer::create(part, settings.container, video.as_ref(), Some(&audio))?;
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
        let black = render::reader::black(reader.size());
        let picture = Vec::with_capacity(reader.bytes());
        Picturing {
            preview,
            reader,
            device,
            queue,
            black,
            picture,
        }
    });
    let mut mixer = Mixer::new(sample_rate);
    let mut loudness = crate::loudness::Loudness::new(sample_rate);
    let mut sound = Vec::new();
    let mut samples_written: u64 = 0;
    let result: diag::Result<bool> = (|| {
        for index in 0..count {
            if shared.cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if let Some(p) = &mut picturing {
                let at = rate.frame_start(first + index as i64);
                p.preview
                    .show(project, sequence, at, timeline_size.0, timeline_size.1);
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
                        writer.write_picture(&p.picture)?;
                    }
                    // Nothing on the timeline here.
                    None => writer.write_picture(&p.black)?,
                }
            }
            // The sound up to the end of this frame, to the exact sample.
            let until = (u128::from(index + 1) * u128::from(sample_rate) * u128::from(rate.den())
                / u128::from(rate.num())) as u64;
            let wanted = (until - samples_written) as usize;
            sound.resize(wanted * 2, 0.0);
            let from =
                rate.frame_start(first).as_secs_f64() + samples_written as f64 / f64::from(sample_rate);
            mixer.mix(project, seq, from, &mut sound);
            loudness.feed(&sound);
            writer.write_sound(&sound)?;
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
    Ok(Some(Exported {
        path: settings.path.clone(),
        frames: count,
        took: started.elapsed(),
        length: Duration::from_secs_f64(count as f64 / rate.as_f64()),
        encoder,
        audio_encoder,
        on_graphics_card,
        loudness: loudness.measured().integrated,
        true_peak: loudness.measured().true_peak_max,
    }))
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
