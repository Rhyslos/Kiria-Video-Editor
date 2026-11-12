//! Importing media: pick files (or drop them on the window), check each one
//! on a worker (`media::check`: codecs this build decodes, first picture
//! and sound readable, nothing missing at the end), then add those that
//! pass to the project in the order chosen. A file that fails is not added:
//! a file alone is explained in full; files picked or dropped together are
//! summed up in one message when the last of them is done ([`Batch`]).
//! A recording whose end is missing is the person's choice: its message
//! (or its batch's) offers "Import what's there", which imports it at the
//! length it really has (`media::salvage`), beside "Skip".
//!
//! Nothing here blocks the interface: the file dialog runs on its own thread
//! and checking runs in the analysis pool. The editor polls [`Importer::poll`]
//! once a frame and applies whatever is ready.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaKind, MediaRef, SequenceSettings};
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
    /// Files being read, in the order they were chosen, with their batch.
    reading: VecDeque<(PathBuf, u64, JobHandle<Checked>)>,
    /// Files picked or dropped together, while any is being read.
    batches: Vec<Batch>,
    next_batch: u64,
    /// Files whose end is missing, set aside until the person chooses.
    aside: std::collections::HashMap<u64, Vec<PathBuf>>,
    next_aside: u64,
}

/// The start of the command a message's "Import what's there" runs; the
/// number of the files set aside follows it.
pub const WHATS_THERE: &str = "import.whats-there#";

/// How the files of a batch are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// Every check (`media::check`).
    Check,
    /// As far as they go: files whose end is missing, imported anyway.
    WhatsThere,
}

type Checked = Result<MediaRef, media::Refused>;

/// A file that finished reading.
pub enum Read {
    Ready(MediaRef),
    /// It failed its checks. `alone`: it was picked or dropped on its own,
    /// so its own explanation is shown; otherwise its batch's summary is.
    Failed {
        path: PathBuf,
        refused: media::Refused,
        alone: bool,
    },
}

/// Files picked or dropped together.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    id: u64,
    pub files: usize,
    pub done: usize,
    /// Each file refused: its name and why, in a few words.
    pub refused: Vec<(String, media::Refusal)>,
    /// The refused files whose end is missing, which can be imported anyway.
    pub cut_short: Vec<PathBuf>,
}

impl Batch {
    /// How many were added.
    pub fn imported(&self) -> usize {
        self.files - self.refused.len()
    }

    /// The one message for files refused in a batch of several, naming up
    /// to eight with why; `None` when all were imported. With files whose
    /// end is missing set aside as `whats_there`, it offers to import them
    /// anyway.
    pub fn summary(&self, whats_there: Option<u64>) -> Option<diag::AppError> {
        if self.refused.is_empty() {
            return None;
        }
        const NAMED: usize = 8;
        let mut lines: Vec<String> = self
            .refused
            .iter()
            .take(NAMED)
            .map(|(name, why)| format!("{name} ({})", why.words()))
            .collect();
        if self.refused.len() > NAMED {
            lines.push(format!("and {} more", self.refused.len() - NAMED));
        }
        let what = match self.refused.len() {
            1 => format!("1 of {} files wasn't imported", self.files),
            n => format!("{n} of {} files weren't imported", self.files),
        };
        let error = diag::AppError::new(what).why(format!("They can't be played: {}.", lines.join(", ")));
        Some(match (whats_there, self.cut_short.len()) {
            (Some(id), n) if n > 0 => error
                .fix(format!(
                    "{} can be imported at the length really there: Import them anyway. Import any of the others on its own to see what is wrong with it and what to do.",
                    if n == 1 { "The one whose end is missing" } else { "Those whose end is missing" }
                ))
                .action(whats_there_action("Import them anyway", id)),
            _ => error.fix("Import one of them on its own to see what is wrong with it and what to do. The others were imported."),
        })
    }
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

    /// Starts checking these files on workers, as one batch.
    pub fn read(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.start(jobs, ctx, paths, How::Check);
    }

    /// Sets files whose end is missing aside until the person chooses;
    /// returns the number "Import what's there" names them by.
    pub fn set_aside(&mut self, paths: Vec<PathBuf>) -> u64 {
        let id = self.next_aside;
        self.next_aside += 1;
        self.aside.insert(id, paths);
        id
    }

    /// Imports the files set aside as `id` at the length really in them, as
    /// one batch. False when there are none (asked already).
    pub fn whats_there(&mut self, jobs: &JobSystem, ctx: &egui::Context, id: u64) -> bool {
        let Some(paths) = self.aside.remove(&id) else {
            return false;
        };
        self.start(jobs, ctx, paths, How::WhatsThere);
        true
    }

    fn start(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>, how: How) {
        if paths.is_empty() {
            return;
        }
        let batch = self.next_batch;
        self.next_batch += 1;
        self.batches.push(Batch {
            id: batch,
            files: paths.len(),
            done: 0,
            refused: Vec::new(),
            cut_short: Vec::new(),
        });
        for path in paths {
            let (ctx, worker_path) = (ctx.clone(), path.clone());
            let handle = jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "check media file",
                move |_| {
                    let result = match how {
                        How::Check => media::check(&worker_path),
                        How::WhatsThere => media::salvage(&worker_path).map_err(|error| media::Refused {
                            refusal: media::Refusal::Damaged,
                            error,
                        }),
                    };
                    ctx.request_repaint();
                    Ok(result)
                },
            );
            self.reading.push_back((path, batch, handle));
        }
    }

    /// Starts reading the files the dialog returned, if it has.
    pub fn poll_dialog(&mut self, jobs: &JobSystem, ctx: &egui::Context) {
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
    }

    /// The next file that finished reading, in the order chosen, with its
    /// batch when it was the batch's last. `None` while the next one is
    /// still being read. The editor takes as many as fit in a few
    /// milliseconds each frame, since adding one to the project writes the
    /// autosave journal.
    pub fn next_read(&mut self) -> Option<(Read, Option<Batch>)> {
        loop {
            let (_, _, handle) = self.reading.front()?;
            let result = handle.try_result()?;
            let (path, batch_id, _) = self.reading.pop_front()?;
            let Some(at) = self.batches.iter().position(|b| b.id == batch_id) else {
                continue;
            };
            let batch = &mut self.batches[at];
            batch.done += 1;
            let checked = result.unwrap_or_else(|error| {
                Err(media::Refused {
                    refusal: media::Refusal::Unreadable,
                    error,
                })
            });
            let read = match checked {
                Ok(media) => Read::Ready(media),
                Err(refused) => {
                    batch.refused.push((name_of(&path), refused.refusal));
                    if refused.refusal == media::Refusal::CutShort {
                        batch.cut_short.push(path.clone());
                    }
                    Read::Failed {
                        path,
                        refused,
                        alone: batch.files == 1,
                    }
                }
            };
            let finished = (batch.done == batch.files).then(|| self.batches.remove(at));
            return Some((read, finished));
        }
    }

    /// How many files are still being read.
    pub fn busy(&self) -> usize {
        self.reading.len()
    }
}

/// Picture size and frame rate in a few words: "1440p 60", or "1080×1920 60"
/// for an upright picture.
pub fn settings_text(s: &SequenceSettings) -> String {
    s.short_label()
}

/// The button that imports the files set aside as `id` as far as they go,
/// beside "Skip".
pub fn whats_there_action(label: &str, id: u64) -> diag::FixAction {
    diag::FixAction::new(label, format!("{WHATS_THERE}{id}")).declined_with("Skip")
}

/// The number of the files set aside that a fix's command names, if it is
/// an "Import what's there".
pub fn whats_there_id(command: &str) -> Option<u64> {
    command.strip_prefix(WHATS_THERE)?.parse().ok()
}

fn name_of(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
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
            _ if v.variable_frame_rate => format!(
                "{size} {} {} (variable frame rate, shown at {} fps)",
                v.frame_rate.nominal_fps(),
                v.codec,
                v.frame_rate
            ),
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
                    variable_frame_rate: false,
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
                stated_length: None,
            },
        }
    }

    #[test]
    fn a_batch_sums_up_what_was_refused() {
        let mut batch = Batch {
            id: 0,
            files: 200,
            done: 200,
            refused: Vec::new(),
            cut_short: Vec::new(),
        };
        assert!(batch.summary(None).is_none(), "nothing refused, nothing to say");
        batch.refused = (0..10)
            .map(|i| (format!("clip{i}.mkv"), media::Refusal::Damaged))
            .collect();
        batch.refused[0].1 = media::Refusal::CutShort;
        let summary = batch.summary(None).unwrap();
        assert_eq!(summary.what(), "10 of 200 files weren't imported");
        let why = summary.why_text().unwrap();
        assert!(why.contains("clip0.mkv (end missing)"), "{why}");
        assert!(
            why.contains("clip7.mkv (damaged)") && !why.contains("clip8.mkv"),
            "{why}"
        );
        assert!(why.contains("and 2 more"), "{why}");
        assert_eq!(batch.imported(), 190);
        assert!(
            summary.fix_action().is_none(),
            "nothing set aside: nothing to offer"
        );

        batch.cut_short = vec!["clip0.mkv".into()];
        let offered = batch.summary(Some(7)).unwrap();
        let action = offered.fix_action().unwrap();
        assert_eq!(action.label, "Import them anyway");
        assert_eq!(action.decline.as_deref(), Some("Skip"));
        assert_eq!(whats_there_id(&action.command), Some(7));
        assert!(
            offered.fix_text().contains("whose end is missing"),
            "{}",
            offered.fix_text()
        );
    }

    #[test]
    fn files_set_aside_are_imported_once() {
        let mut importer = Importer::default();
        let id = importer.set_aside(vec!["cut.mkv".into()]);
        assert_eq!(whats_there_id("import.whats-there#x"), None);
        assert_eq!(whats_there_id("media.relink"), None);
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let ctx = egui::Context::default();
        assert!(importer.whats_there(&jobs, &ctx, id));
        assert_eq!(importer.busy(), 1);
        assert!(!importer.whats_there(&jobs, &ctx, id), "asked already");
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
