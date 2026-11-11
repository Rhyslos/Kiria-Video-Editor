//! Exporting: the timeline, or a stretch of it, written as one video file.
//!
//! A worker of its own asks for the timeline's picture frame by frame (from
//! a preview worker of its own, so pictures are decoded and put together
//! exactly as in the viewer, at the timeline's full size), reads each back
//! from the graphics card as the planes an encoder takes, mixes the sound
//! for the same stretch with the playback mixer, and hands both to
//! `media::encode`. The editor stays usable meanwhile; [`Export`] says how
//! far it is and can be told to stop.
//!
//! The file is written beside its final place under another name and only
//! takes its name when it is complete, so a cancelled or failed export
//! leaves nothing that looks like a video.

use crate::compose::Composing;
use crate::mix::Mixer;
use crate::preview::{Convert, OnDevice, Picture, Preview};
use diag::{AppError, Explain};
use media::Decoding;
use media::encode::{AudioSettings, VideoSettings, Writer};
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

    /// Bits a second for pictures of `size` at `rate`.
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
}

/// Bits a second for the sound.
const SOUND_BIT_RATE: u64 = 192_000;

/// What to export, and where to.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    /// The file to write; it is replaced if it is there.
    pub path: PathBuf,
    /// A stretch of the timeline, or all of it.
    pub range: Option<(Time, Time)>,
    pub quality: Quality,
    /// Encode on the CPU even when a graphics card could.
    pub software: bool,
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
    pub encoder: String,
    pub on_graphics_card: bool,
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
    /// don't change what is exported). `notify` runs on the worker now and
    /// then while it works and when it ends.
    pub fn start(
        project: Project,
        sequence: SequenceId,
        settings: ExportSettings,
        devices: Devices,
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
    let ending = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
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
    devices: Devices,
    shared: &Shared,
    notify: &dyn Fn(),
) -> diag::Result<Option<Exported>> {
    let started = Instant::now();
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

    let (device, queue) = (devices.composing.device.clone(), devices.composing.queue.clone());
    let mut reader = Reader::new(&device, (seq.settings.width, seq.settings.height));
    let size = reader.size();
    let sample_rate = seq.settings.sample_rate.max(8_000);
    let video = VideoSettings {
        width: size.0,
        height: size.1,
        rate: (rate.num(), rate.den()),
        bit_rate: settings.quality.bit_rate(size, rate),
        software: settings.software,
    };
    let audio = AudioSettings {
        sample_rate,
        bit_rate: SOUND_BIT_RATE,
    };
    let mut writer = Writer::create(part, &video, Some(&audio))?;
    let encoder = writer.video_encoder().to_owned();
    let on_graphics_card = writer.on_graphics_card();
    if let Ok(mut name) = shared.encoder.lock() {
        *name = Some(encoder.clone());
    }
    tracing::info!(
        "exporting {count} frames of {}×{} at {} bits a second with {encoder}",
        size.0,
        size.1,
        video.bit_rate
    );

    // A worker of its own, so the viewer keeps its decoders and its place.
    // Nothing is kept on the card for going back to: export only goes forward.
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
    let mut mixer = Mixer::new(sample_rate);
    let black = render::reader::black(size);
    let mut picture = Vec::with_capacity(reader.bytes());
    let mut sound = Vec::new();
    let mut samples_written: u64 = 0;
    let result: diag::Result<bool> = (|| {
        for index in 0..count {
            if shared.cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let at = rate.frame_start(first + index as i64);
            preview.show(project, sequence, at, seq.settings.width, seq.settings.height);
            let shown = wait_for(&preview, at)?;
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
                    reader
                        .read(&device, &queue, frame.view(), frame.working(), &mut picture)
                        .map_err(|err| {
                            cannot(format!(
                                "A finished frame could not be read back from the graphics card ({err:?})."
                            ))
                            .fix("Export again. If it keeps happening, update the graphics driver.")
                        })?;
                    writer.write_picture(&picture)?;
                }
                // Nothing on the timeline here.
                None => writer.write_picture(&black)?,
            }
            // The sound up to the end of this frame, to the exact sample.
            let until = (u128::from(index + 1) * u128::from(sample_rate) * u128::from(rate.den())
                / u128::from(rate.num())) as u64;
            let wanted = (until - samples_written) as usize;
            sound.resize(wanted * 2, 0.0);
            let from =
                rate.frame_start(first).as_secs_f64() + samples_written as f64 / f64::from(sample_rate);
            mixer.mix(project, seq, from, &mut sound);
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
    preview.close(Duration::from_secs(5));
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
        on_graphics_card,
    }))
}

/// The picture for the moment `at`, which was just asked for.
fn wait_for(preview: &Preview, at: Time) -> diag::Result<Picture> {
    let give_up = Instant::now() + FRAME_PATIENCE;
    loop {
        match preview.wait(Duration::from_millis(250)) {
            Some(picture) if picture.at == at => return Ok(picture),
            // One for an earlier request: not the one waited for.
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
