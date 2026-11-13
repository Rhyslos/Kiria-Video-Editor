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

/// File types offered in the dialog (the same the media folders are
/// watched for). Anything FFmpeg reads can still be dropped in.
const EXTENSIONS: &[&str] = library::watch::EXTENSIONS;

#[derive(Default)]
pub struct Importer {
    /// The file dialog, and whether what it picks goes on the timeline too.
    dialog: Option<(Receiver<Vec<PathBuf>>, bool)>,
    /// Files being read, in the order they were chosen, with their batch.
    reading: VecDeque<Reading>,
    /// Files picked or dropped together, while any is being read.
    batches: Vec<Batch>,
    next_batch: u64,
    /// Files whose end is missing, set aside until the person chooses.
    aside: std::collections::HashMap<u64, (Vec<PathBuf>, bool)>,
    next_aside: u64,
    /// The timeline imports go on is automatic (`set_automatic`).
    automatic: bool,
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

/// One file being checked: its job, then what the check said (kept until
/// its turn comes, or its batch is all checked when the batch waits).
struct Reading {
    path: PathBuf,
    batch: u64,
    handle: JobHandle<Checked>,
    checked: Option<diag::Result<Checked>>,
}

/// A file that finished reading.
pub enum Read {
    /// It passed its checks. `place`: put it on the timeline as well
    /// (imports do); otherwise it is only added to the project's media.
    /// `settle`: the settings an automatic timeline takes, worked out from
    /// every file of its batch (`SequenceSettings::most_common`).
    Ready {
        media: MediaRef,
        place: bool,
        settle: Option<SequenceSettings>,
    },
    /// It failed its checks. `alone`: it was picked or dropped on its own,
    /// so its own explanation is shown; otherwise its batch's summary is.
    Failed {
        path: PathBuf,
        refused: media::Refused,
        alone: bool,
        /// Whether it was to go on the timeline too.
        place: bool,
    },
}

/// Files picked or dropped together.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    id: u64,
    /// Its files go on the timeline too.
    pub place: bool,
    /// It goes onto an automatic timeline: none is placed until all are
    /// checked, so the settings come from them all, not the first.
    waits: bool,
    settle: Option<SequenceSettings>,
    pub files: usize,
    pub done: usize,
    /// Each file refused: where it is and why, in a few words.
    pub refused: Vec<(PathBuf, media::Refusal)>,
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
        let paths: Vec<&std::path::Path> = self.refused.iter().map(|(p, _)| p.as_path()).collect();
        let mut lines: Vec<String> = self
            .refused
            .iter()
            .take(NAMED)
            .map(|(path, why)| format!("{} ({})", told_apart(path, &paths), why.words()))
            .collect();
        if self.refused.len() > NAMED {
            lines.push(format!("and {} more", self.refused.len() - NAMED));
        }
        let what = match self.refused.len() {
            1 => format!("1 of {} files wasn't imported", self.files),
            n => format!("{n} of {} files weren't imported", self.files),
        };
        // Every one with its whole path, for Details.
        let every: Vec<String> = self
            .refused
            .iter()
            .map(|(path, why)| format!("{} ({})", path.display(), why.words()))
            .collect();
        let error = diag::AppError::new(what)
            .why(format!("They can't be played: {}.", lines.join(", ")))
            .source(std::io::Error::other(format!(
                "Files not imported:\n{}",
                every.join("\n")
            )));
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
    /// Opens the file dialog, unless one is already open. `place`: the
    /// files picked go on the timeline as well (the Edit page's import);
    /// otherwise into the project's media only (the Library page's).
    pub fn choose_files(&mut self, ctx: &egui::Context, place: bool) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = ui_kit::file_dialog::run(&ctx, || {
                rfd::FileDialog::new()
                    .set_title("Import media")
                    .add_filter("Video, sound and pictures", EXTENSIONS)
                    .add_filter("All files", &["*"])
                    .pick_files()
            })
            .unwrap_or_default();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some((rx, place));
    }

    /// Starts checking these files on workers, as one batch, to add them
    /// to the project and place them at the end of the timeline.
    pub fn read(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.start(jobs, ctx, paths, How::Check, true);
    }

    /// The timeline the next imports go on is automatic: batches placed
    /// from now on wait until all their files are checked, so its
    /// settings come from what most of them share.
    pub fn set_automatic(&mut self, automatic: bool) {
        self.automatic = automatic;
    }

    /// Starts checking these files on workers, as one batch, to add them
    /// to the project's media only (new files from the media folders, the
    /// Library page's import and files dropped on it).
    pub fn add(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.start(jobs, ctx, paths, How::Check, false);
    }

    /// Sets files whose end is missing aside until the person chooses
    /// (`place`: as their import would have, on the timeline too); returns
    /// the number "Import what's there" names them by.
    pub fn set_aside(&mut self, paths: Vec<PathBuf>, place: bool) -> u64 {
        let id = self.next_aside;
        self.next_aside += 1;
        self.aside.insert(id, (paths, place));
        id
    }

    /// Imports the files set aside as `id` at the length really in them, as
    /// one batch. False when there are none (asked already).
    pub fn whats_there(&mut self, jobs: &JobSystem, ctx: &egui::Context, id: u64) -> bool {
        let Some((paths, place)) = self.aside.remove(&id) else {
            return false;
        };
        self.start(jobs, ctx, paths, How::WhatsThere, place);
        true
    }

    fn start(&mut self, jobs: &JobSystem, ctx: &egui::Context, paths: Vec<PathBuf>, how: How, place: bool) {
        if paths.is_empty() {
            return;
        }
        // Full paths in the project: a relative one would mean "from the
        // project's folder" when it is opened again (`file::resolve_media_paths`).
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .collect();
        let batch = self.next_batch;
        self.next_batch += 1;
        self.batches.push(Batch {
            id: batch,
            place,
            waits: place && self.automatic && paths.len() > 1,
            settle: None,
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
            self.reading.push_back(Reading {
                path,
                batch,
                handle,
                checked: None,
            });
        }
    }

    /// Starts reading the files the dialog returned, if it has.
    pub fn poll_dialog(&mut self, jobs: &JobSystem, ctx: &egui::Context) {
        if let Some((rx, place)) = &self.dialog {
            let place = *place;
            match rx.try_recv() {
                Ok(paths) => {
                    self.dialog = None;
                    self.start(jobs, ctx, paths, How::Check, place);
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
        for reading in self.reading.iter_mut().filter(|r| r.checked.is_none()) {
            reading.checked = reading.handle.try_result();
        }
        loop {
            let front = self.reading.front()?;
            front.checked.as_ref()?;
            let batch_id = front.batch;
            let Some(at) = self.batches.iter().position(|b| b.id == batch_id) else {
                self.reading.pop_front();
                continue;
            };
            if self.batches[at].waits {
                let all_checked = self
                    .reading
                    .iter()
                    .filter(|r| r.batch == batch_id)
                    .all(|r| r.checked.is_some());
                if !all_checked {
                    return None;
                }
                let found: Vec<&MediaRef> = self
                    .reading
                    .iter()
                    .filter(|r| r.batch == batch_id)
                    .filter_map(|r| match &r.checked {
                        Some(Ok(Ok(media))) => Some(media),
                        _ => None,
                    })
                    .collect();
                let settle = SequenceSettings::most_common(&found);
                let batch = &mut self.batches[at];
                batch.settle = settle;
                batch.waits = false;
            }
            let reading = self.reading.pop_front()?;
            let path = reading.path;
            let result = reading.checked?;
            let batch = &mut self.batches[at];
            batch.done += 1;
            let checked = result.unwrap_or_else(|error| {
                Err(media::Refused {
                    refusal: media::Refusal::Unreadable,
                    error,
                })
            });
            let read = match checked {
                Ok(media) => Read::Ready {
                    media,
                    place: batch.place,
                    settle: batch.settle,
                },
                Err(refused) => {
                    batch.refused.push((path.clone(), refused.refusal));
                    if refused.refusal == media::Refusal::CutShort {
                        batch.cut_short.push(path.clone());
                    }
                    Read::Failed {
                        path,
                        refused,
                        alone: batch.files == 1,
                        place: batch.place,
                    }
                }
            };
            let finished = (batch.done == batch.files).then(|| self.batches.remove(at));
            return Some((read, finished));
        }
    }

    /// Whether this file is being checked now.
    pub fn is_reading(&self, path: &std::path::Path) -> bool {
        self.reading.iter().any(|r| r.path == path)
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

/// A refused file's name, with its folder's name when another refused
/// file has the same name ("not-media.mkv in broken"), or its whole folder
/// when those are the same too.
fn told_apart(path: &std::path::Path, all: &[&std::path::Path]) -> String {
    let name = name_of(path);
    let twins: Vec<&&std::path::Path> = all.iter().filter(|p| name_of(p) == name).collect();
    if twins.len() < 2 {
        return name;
    }
    let folder_name = |p: &std::path::Path| p.parent().map(name_of).unwrap_or_default();
    let mine = folder_name(path);
    if twins.iter().filter(|p| folder_name(p) == mine).count() < 2 {
        return format!("{name} in {mine}");
    }
    let folder = path
        .parent()
        .map_or_else(String::new, |p| p.display().to_string());
    format!("{name} in {folder}")
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
        // A stream's made-up name ("Audio 1") would read like a track's.
        1 if project::tracks::titled(&media.info.audio[0].name) => parts.push(format!(
            "sound stream \u{201c}{}\u{201d}",
            media.info.audio[0].name
        )),
        1 => parts.push("with sound".into()),
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
            place: true,
            files: 200,
            done: 200,
            refused: Vec::new(),
            cut_short: Vec::new(),
            waits: false,
            settle: None,
        };
        assert!(batch.summary(None).is_none(), "nothing refused, nothing to say");
        batch.refused = (0..10)
            .map(|i| (PathBuf::from(format!("/r/clip{i}.mkv")), media::Refusal::Damaged))
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
    fn refused_files_of_the_same_name_are_told_apart() {
        let batch = Batch {
            id: 0,
            place: false,
            files: 4,
            done: 4,
            refused: vec![
                ("/m/recordings/not-media.mkv".into(), media::Refusal::Unsupported),
                (
                    "/m/recordings/broken/not-media.mkv".into(),
                    media::Refusal::Unsupported,
                ),
                ("/m/recordings/empty.mp3".into(), media::Refusal::Unsupported),
            ],
            cut_short: Vec::new(),
            waits: false,
            settle: None,
        };
        let summary = batch.summary(None).unwrap();
        let why = summary.why_text().unwrap();
        assert!(why.contains("not-media.mkv in recordings ("), "{why}");
        assert!(why.contains("not-media.mkv in broken ("), "{why}");
        assert!(why.contains("empty.mp3 ("), "a name alone stays short: {why}");
        assert!(
            summary.report().contains("/m/recordings/broken/not-media.mkv"),
            "Details have every path"
        );
        let same_folder_name = ["/a/x/f.mkv", "/b/x/f.mkv"].map(std::path::Path::new);
        assert_eq!(
            told_apart(same_folder_name[1], &same_folder_name),
            "f.mkv in /b/x"
        );
    }

    #[test]
    fn files_set_aside_are_imported_once() {
        let mut importer = Importer::default();
        let id = importer.set_aside(vec!["cut.mkv".into()], true);
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
            "match.mkv · sound stream \u{201c}Mic\u{201d}"
        );
        assert_eq!(
            summary(&media(MediaKind::Audio, &["Audio 1"])),
            "match.mkv · with sound"
        );
        assert_eq!(
            summary(&media(MediaKind::Image, &[])),
            "match.mkv · 2560×1440 picture"
        );
    }
}
