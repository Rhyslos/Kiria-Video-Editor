//! Importing media: pick files (or drop them on the window), read each one on
//! a worker, then add it to the project in the order chosen.
//!
//! Nothing here blocks the interface: the file dialog runs on its own thread
//! and probing runs in the analysis pool. The editor polls [`Importer::poll`]
//! once a frame and applies whatever is ready.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaKind, MediaRef};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// File types offered in the dialog. Anything FFmpeg reads can still be dropped in.
const EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "mov", "m4v", "webm", "avi", "ts", "flv", "wmv", // video
    "wav", "mp3", "flac", "ogg", "opus", "m4a", "aac", "wma", // sound
    "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", // pictures
];

#[derive(Default)]
pub struct Importer {
    dialog: Option<Receiver<Vec<PathBuf>>>,
    /// Files being read, in the order they were chosen.
    reading: VecDeque<(PathBuf, JobHandle<MediaRef>)>,
}

/// A file that finished reading.
pub enum Read {
    Ready(MediaRef),
    Failed(PathBuf, diag::AppError),
}

impl Importer {
    /// Opens the file dialog, unless one is already open.
    pub fn choose_files(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Import media")
                .add_filter("Video, sound and pictures", EXTENSIONS)
                .add_filter("All files", &["*"])
                .pick_files()
                .unwrap_or_default();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some(rx);
    }

    /// Starts reading these files on workers.
    pub fn read(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>) {
        for path in paths {
            let (ctx, worker_path) = (ctx.clone(), path.clone());
            let handle = jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "read media file",
                move |_| {
                    let result = media::describe(&worker_path);
                    ctx.request_repaint();
                    result
                },
            );
            self.reading.push_back((path, handle));
        }
    }

    /// Files the dialog returned, and files that finished reading (in order).
    pub fn poll(&mut self, jobs: &JobSystem, ctx: &egui::Context) -> Vec<Read> {
        if let Some(rx) = &self.dialog {
            match rx.try_recv() {
                Ok(paths) => {
                    self.dialog = None;
                    self.read(jobs, ctx, paths);
                }
                Err(TryRecvError::Disconnected) => self.dialog = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut done = Vec::new();
        while let Some((_, handle)) = self.reading.front() {
            let Some(result) = handle.try_result() else { break };
            let Some((path, _)) = self.reading.pop_front() else {
                break;
            };
            done.push(match result {
                Ok(media) => Read::Ready(media),
                Err(err) => Read::Failed(path, err),
            });
        }
        done
    }

    /// How many files are still being read.
    pub fn busy(&self) -> usize {
        self.reading.len()
    }
}

/// One line about an imported file, such as
/// "match.mkv · 1440p 60 hevc · 3 sound streams (Game, Mic, Voice chat)".
pub fn summary(media: &MediaRef) -> String {
    let name = media.path.file_name().map_or_else(
        || media.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut parts = vec![name];
    if let Some(v) = &media.info.video {
        let (w, h) = v.display_size();
        let size = if w >= h {
            format!("{h}p")
        } else {
            format!("{w}×{h}")
        };
        parts.push(match media.kind {
            MediaKind::Image => format!("{w}×{h} picture"),
            _ => format!("{size} {} {}", v.frame_rate.nominal_fps(), v.codec),
        });
    }
    match media.info.audio.len() {
        0 if media.kind != MediaKind::Image => parts.push("no sound".into()),
        0 => {}
        1 => parts.push(format!("sound: {}", media.info.audio[0].name)),
        n => {
            let names: Vec<&str> = media.info.audio.iter().map(|a| a.name.as_str()).collect();
            parts.push(format!("{n} sound streams ({})", names.join(", ")));
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{AudioStream, Fingerprint, MediaId, MediaInfo, Rate, Time, VideoStream};

    fn media(kind: MediaKind, audio: &[&str]) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: "rec/match.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind,
            duration: Time::seconds(10),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: (kind != MediaKind::Audio).then(|| VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 2560,
                    height: 1440,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                }),
                audio: audio
                    .iter()
                    .map(|n| AudioStream {
                        index: 1,
                        codec: "aac".into(),
                        channels: 2,
                        sample_rate: 48_000,
                        name: (*n).into(),
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn summaries_say_what_was_found() {
        assert_eq!(
            summary(&media(MediaKind::Video, &["Game", "Mic", "Voice chat"])),
            "match.mkv · 1440p 60 hevc · 3 sound streams (Game, Mic, Voice chat)"
        );
        assert_eq!(
            summary(&media(MediaKind::Video, &[])),
            "match.mkv · 1440p 60 hevc · no sound"
        );
        assert_eq!(
            summary(&media(MediaKind::Audio, &["Mic"])),
            "match.mkv · sound: Mic"
        );
        assert_eq!(
            summary(&media(MediaKind::Image, &[])),
            "match.mkv · 2560×1440 picture"
        );
    }
}
