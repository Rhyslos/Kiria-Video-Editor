//! The checks before an export, in plain words, each with a one-click fix
//! where there is one: media that can't be found, loudness and peaks,
//! effects that need rendering first, disk space, and the smaller things
//! (a file that will be replaced, a picture made larger than the timeline).
//!
//! `list` is pure: it turns what is known into the lines shown. What takes
//! the disk or the mixer is found on workers here: [`LoudnessWatch`]
//! measures the timeline's mix after it changed, [`DiskWatch`] looks at the
//! folder, the free space and the file.

use engine::export::{MixMeasured, VideoCodec, measure_mix};
use project::{Project, SequenceId, Time};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How a check stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    /// Something to know, nothing wrong.
    Note,
    /// Still being found out.
    Waiting,
    Warn,
    /// The export can't start until it is fixed.
    Bad,
}

/// What a check's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Look for the missing files (the window's "Find them…").
    FindMissing,
    /// Turn on normalising.
    Normalise,
    /// Export at the timeline's own size.
    TimelineSize,
    /// Pick another folder.
    ChooseFolder,
    /// Make the folder typed in.
    MakeFolder,
    /// Give the file a name that is free.
    NewName,
    /// Take characters a file name can't have out of it.
    CleanName,
    /// Use this picture codec instead.
    UseCodec(VideoCodec),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub level: Level,
    pub text: String,
    pub fix: Option<(Fix, &'static str)>,
}

impl Check {
    fn new(level: Level, text: impl Into<String>) -> Check {
        Check {
            level,
            text: text.into(),
            fix: None,
        }
    }

    fn with(mut self, fix: Fix, label: &'static str) -> Check {
        self.fix = Some((fix, label));
        self
    }
}

/// The loudness of the mix, as far as it is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Loudness {
    /// Being measured: how far, 0 to 1.
    Measuring(f32),
    Measured(MixMeasured),
}

/// What the folder and the drive look like.
#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub folder_exists: bool,
    /// Free bytes on the folder's drive, when that can be told.
    pub free: Option<u64>,
    /// The file is there already (it will be replaced).
    pub file_exists: bool,
}

/// What the checks are worked out from.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// How many files the timeline plays, and how many of them can't be found.
    pub media: (usize, usize),
    /// Nothing to export: an empty timeline or region.
    pub empty: bool,
    /// The file has pictures but this window can't put them together.
    pub no_pictures_here: bool,
    /// Whether this machine encodes the format chosen (`None`: not known yet),
    /// and the picture codec chosen.
    pub encodable: Option<bool>,
    pub video: Option<VideoCodec>,
    /// A picture codec this machine encodes, to offer instead.
    pub other_codec: Option<VideoCodec>,
    pub loudness: Option<Loudness>,
    pub normalise: bool,
    pub target: Option<f32>,
    pub disk: Option<Disk>,
    /// The file name typed, and whether it has characters a name can't have.
    pub name: String,
    pub bad_name: bool,
    /// How many bytes the export comes to.
    pub needed: u64,
    /// The timeline's size and the file's, when the file has pictures.
    pub sizes: Option<((u32, u32), (u32, u32))>,
}

/// The checks, in the order shown.
pub fn list(f: &Facts) -> Vec<Check> {
    let mut checks = Vec::new();
    if f.empty {
        checks.push(Check::new(
            Level::Bad,
            "There is nothing to export: the timeline (or the region) is empty.",
        ));
    }
    checks.push(media(f.media));
    if f.no_pictures_here {
        checks.push(Check::new(
            Level::Bad,
            "This window isn't drawn on a graphics card, so pictures can't be put together here. Sound alone can be exported.",
        ));
    }
    match (f.encodable, f.video) {
        (None, _) => checks.push(Check::new(
            Level::Waiting,
            "Finding out what this machine can encode…",
        )),
        (Some(false), Some(codec)) => {
            let check = Check::new(
                Level::Bad,
                format!("This machine can't encode {}.", codec.label()),
            );
            checks.push(match f.other_codec {
                Some(VideoCodec::H264) => check.with(Fix::UseCodec(VideoCodec::H264), "Use H.264"),
                Some(VideoCodec::H265) => check.with(Fix::UseCodec(VideoCodec::H265), "Use H.265"),
                Some(VideoCodec::Av1) => check.with(Fix::UseCodec(VideoCodec::Av1), "Use AV1"),
                None => check,
            });
        }
        (Some(false), None) => checks.push(Check::new(
            Level::Bad,
            "This machine can't encode that sound format.",
        )),
        (Some(true), _) => {}
    }
    if let Some(check) = loudness(f) {
        checks.push(check);
    }
    checks.push(Check::new(
        Level::Good,
        "Nothing needs rendering first: every effect is drawn as the file is written.",
    ));
    if let Some((timeline, file)) = f.sizes {
        checks.extend(size(timeline, file));
    }
    checks.extend(name_and_disk(f));
    checks
}

fn media((used, missing): (usize, usize)) -> Check {
    match (used, missing) {
        (0, _) => Check::new(Level::Good, "The timeline uses no media files."),
        (1, 0) => Check::new(Level::Good, "Its one media file is found."),
        (n, 0) => Check::new(Level::Good, format!("All {n} media files are found.")),
        (_, 1) => Check::new(
            Level::Bad,
            "One media file the timeline uses can't be found: its clips would export black and silent.",
        )
        .with(Fix::FindMissing, "Find it…"),
        (_, n) => Check::new(
            Level::Bad,
            format!(
                "{n} media files the timeline uses can't be found: their clips would export black and silent."
            ),
        )
        .with(Fix::FindMissing, "Find them…"),
    }
}

fn loudness(f: &Facts) -> Option<Check> {
    let lufs = |v: f32| ui_kit::master::db_text(v);
    let ceiling = ui_kit::master::TRUE_PEAK_CEILING;
    let target = f.target;
    let check = match (f.loudness?, f.normalise, target) {
        (Loudness::Measuring(share), true, Some(t)) => Check::new(
            Level::Waiting,
            format!(
                "Measuring the loudness ({:.0}%). It will be brought to {} LUFS, peaks under {ceiling:.0} dBTP.",
                share * 100.0,
                super::words::target_text(t)
            ),
        ),
        (Loudness::Measuring(share), ..) => Check::new(
            Level::Waiting,
            format!("Measuring the loudness and peaks ({:.0}%)…", share * 100.0),
        ),
        (Loudness::Measured(m), _, _) if !m.integrated.is_finite() => Check::new(
            Level::Note,
            "The timeline is silent: the file's sound will be too.",
        ),
        (Loudness::Measured(m), true, Some(t)) => Check::new(
            Level::Good,
            format!(
                "Loudness {} LUFS now: it will be brought to {} LUFS, peaks kept under {ceiling:.0} dBTP.",
                lufs(m.integrated),
                super::words::target_text(t)
            ),
        ),
        (Loudness::Measured(m), _, _) if m.true_peak > ceiling => {
            let check = Check::new(
                Level::Warn,
                format!(
                    "Loudness {} LUFS, peaks at {} dBTP: over {ceiling:.0}, so they may distort when played or encoded again.",
                    lufs(m.integrated),
                    lufs(m.true_peak)
                ),
            );
            if target.is_some() {
                check.with(Fix::Normalise, "Normalise")
            } else {
                check
            }
        }
        (Loudness::Measured(m), _, Some(t)) if (m.integrated - t).abs() > 2.0 => Check::new(
            Level::Warn,
            format!(
                "Loudness {} LUFS: {} LU {} than the {} LUFS aimed for. Peaks at {} dBTP.",
                lufs(m.integrated),
                lufs((m.integrated - t).abs()),
                if m.integrated < t { "quieter" } else { "louder" },
                super::words::target_text(t),
                lufs(m.true_peak)
            ),
        )
        .with(Fix::Normalise, "Normalise"),
        (Loudness::Measured(m), _, _) => Check::new(
            Level::Good,
            format!(
                "Loudness {} LUFS, peaks at {} dBTP.",
                lufs(m.integrated),
                lufs(m.true_peak)
            ),
        ),
    };
    Some(check)
}

fn size(timeline: (u32, u32), file: (u32, u32)) -> Vec<Check> {
    let mut checks = Vec::new();
    let shape = |(w, h): (u32, u32)| f64::from(w) / f64::from(h.max(1));
    let (t, f) = (shape(timeline), shape(file));
    if (t - f).abs() > 0.01 {
        let bars = if t > f { "above and below" } else { "at the sides" };
        checks.push(Check::new(
            Level::Note,
            format!(
                "The timeline is {} × {}: in a {} × {} file it gets black bars {bars}.",
                timeline.0, timeline.1, file.0, file.1
            ),
        ));
    }
    let fitted = engine::export::picture_size(timeline, file);
    if fitted.0 > timeline.0 + 2 || fitted.1 > timeline.1 + 2 {
        checks.push(
            Check::new(
                Level::Warn,
                format!(
                    "The timeline is {} × {}: at {} × {} its picture is made larger, not sharper.",
                    timeline.0, timeline.1, file.0, file.1
                ),
            )
            .with(Fix::TimelineSize, "Use the timeline's size"),
        );
    }
    checks
}

fn name_and_disk(f: &Facts) -> Vec<Check> {
    let mut checks = Vec::new();
    if f.name.trim().is_empty() {
        checks.push(Check::new(Level::Bad, "The file needs a name."));
    } else if f.bad_name {
        checks.push(
            Check::new(
                Level::Bad,
                "The file name has characters a file name can't have (\\ / : * ? \" < > |).",
            )
            .with(Fix::CleanName, "Replace them"),
        );
    }
    let need = super::words::bytes_text(f.needed);
    match &f.disk {
        None => checks.push(Check::new(
            Level::Waiting,
            "Looking at the folder and the free space…",
        )),
        Some(disk) if !disk.folder_exists => checks.push(
            Check::new(Level::Bad, "The folder to save to doesn't exist.").with(Fix::MakeFolder, "Make it"),
        ),
        Some(disk) => {
            if disk.file_exists {
                checks.push(
                    Check::new(
                        Level::Warn,
                        format!("{} is there already and will be replaced.", f.name),
                    )
                    .with(Fix::NewName, "Use a new name"),
                );
            }
            checks.push(match disk.free {
                None => Check::new(
                    Level::Note,
                    format!("The free space there can't be told; the export needs about {need}."),
                ),
                Some(free) if free < f.needed => Check::new(
                    Level::Bad,
                    format!(
                        "Not enough space: the export needs about {need}, and {} is free there.",
                        super::words::bytes_text(free)
                    ),
                )
                .with(Fix::ChooseFolder, "Choose another folder"),
                // Estimates can be off: a quarter more, and some room left over.
                Some(free) if free < f.needed + f.needed / 4 + 500_000_000 => Check::new(
                    Level::Warn,
                    format!(
                        "Space is tight: {} free there, and the export needs about {need}.",
                        super::words::bytes_text(free)
                    ),
                )
                .with(Fix::ChooseFolder, "Choose another folder"),
                Some(free) => Check::new(
                    Level::Good,
                    format!(
                        "{} free there; the export needs about {need}.",
                        super::words::bytes_text(free)
                    ),
                ),
            });
        }
    }
    checks
}

/// How many of the files `sequence` plays (through nested timelines too)
/// can't be found, and how many it plays.
pub fn media_counts(project: &Project, sequence: SequenceId, missing: &[project::MediaId]) -> (usize, usize) {
    let mut used = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut todo = vec![sequence];
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(seq) = project.sequences.iter().find(|s| s.id == id) else {
            continue;
        };
        for clip in seq.tracks().flat_map(|t| t.clips.iter()) {
            if clip.is_file() {
                used.insert(clip.media);
            } else if let Some(nested) = clip.nested() {
                todo.push(nested);
            }
        }
    }
    let missing = used.iter().filter(|m| missing.contains(m)).count();
    (used.len(), missing)
}

/// A file name with the characters a name can't have replaced.
pub fn clean_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if r#"\/:*?"<>|"#.contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    clean.trim().trim_matches('.').to_owned()
}

/// "Cut", "Cut 2", "Cut 3": the first name `taken` says is free.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = name.trim();
    (2..1000)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| format!("{base} new"))
}

/// What the loudness is measured for: a timeline as it is, and a stretch.
struct Measuring {
    project: Project,
    sequence: SequenceId,
    range: Option<(Time, Time)>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU32>,
    answer: Receiver<MixMeasured>,
}

/// Measures the mix's loudness and true peak on a worker, again a moment
/// after the timeline or the stretch changed.
#[derive(Default)]
pub struct LoudnessWatch {
    measuring: Option<Measuring>,
    found: Option<MixMeasured>,
    /// When the timeline was seen to change, to let edits settle first.
    changed: Option<Instant>,
}

impl std::fmt::Debug for LoudnessWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessWatch")
            .field("found", &self.found)
            .finish_non_exhaustive()
    }
}

/// How long the timeline must stay as it is before it is measured again.
const SETTLE: Duration = Duration::from_millis(700);

impl LoudnessWatch {
    /// What is known for `sequence` of `project` over `range`; starts or
    /// restarts the measuring when that changed. `notify` runs when the
    /// worker has an answer.
    pub fn look(
        &mut self,
        project: &Project,
        sequence: SequenceId,
        range: Option<(Time, Time)>,
        notify: impl Fn() + Send + 'static,
    ) -> Option<Loudness> {
        let same = self.measuring.as_ref().is_some_and(|m| {
            m.sequence == sequence
                && m.range == range
                && m.project.sequences.ptr_eq(&project.sequences)
                && m.project.media.ptr_eq(&project.media)
        });
        if !same {
            // A change: what was found is out of date, and measuring waits
            // until the edits settle.
            if let Some(old) = self.measuring.take() {
                old.cancel.store(true, Ordering::Relaxed);
                self.found = None;
                self.changed = Some(Instant::now());
            }
            if self.changed.is_some_and(|at| at.elapsed() < SETTLE) {
                return Some(Loudness::Measuring(0.0));
            }
            self.changed = None;
            self.measuring = Some(start(project, sequence, range, notify));
        }
        let measuring = self.measuring.as_ref()?;
        if let Ok(found) = measuring.answer.try_recv() {
            self.found = Some(found);
        }
        Some(match self.found {
            Some(found) => Loudness::Measured(found),
            None => Loudness::Measuring(measuring.done.load(Ordering::Relaxed) as f32 / 1000.0),
        })
    }

    /// Whether the newest measuring is done (for screenshots).
    pub fn settled(&self) -> bool {
        self.found.is_some()
    }
}

impl Drop for LoudnessWatch {
    fn drop(&mut self) {
        if let Some(m) = &self.measuring {
            m.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn start(
    project: &Project,
    sequence: SequenceId,
    range: Option<(Time, Time)>,
    notify: impl Fn() + Send + 'static,
) -> Measuring {
    let (tell, answer) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicU32::new(0));
    let (p, c, d) = (project.clone(), Arc::clone(&cancel), Arc::clone(&done));
    let spawned = std::thread::Builder::new()
        .name("export loudness".into())
        .spawn(move || {
            if let Some(found) = measure_mix(&p, sequence, range, &c, &d) {
                let _ = tell.send(found);
                notify();
            }
        });
    if let Err(err) = spawned {
        tracing::error!("the loudness check could not start: {err}");
    }
    Measuring {
        project: project.clone(),
        sequence,
        range,
        cancel,
        done,
        answer,
    }
}

/// Looks at the folder, its drive's free space and the file on a worker,
/// when they change and every few seconds.
#[derive(Debug, Default)]
pub struct DiskWatch {
    asked: Option<(PathBuf, PathBuf, Instant)>,
    looking: Option<Receiver<Disk>>,
    found: Option<Disk>,
}

/// How often the free space is looked at again while the page is open.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

impl DiskWatch {
    pub fn look(&mut self, folder: &Path, file: &Path, notify: impl Fn() + Send + 'static) -> Option<Disk> {
        if let Some(looking) = &self.looking
            && let Ok(found) = looking.try_recv()
        {
            self.found = Some(found);
            self.looking = None;
        }
        let changed = self
            .asked
            .as_ref()
            .is_none_or(|(f, p, _)| f != folder || p != file);
        let stale = self
            .asked
            .as_ref()
            .is_some_and(|(.., at)| at.elapsed() > LOOK_AGAIN);
        if changed {
            self.found = None;
        }
        if (changed || stale) && (self.looking.is_none() || changed) {
            let (tell, answer) = channel();
            let (folder, file) = (folder.to_path_buf(), file.to_path_buf());
            self.asked = Some((folder.clone(), file.clone(), Instant::now()));
            let spawned = std::thread::Builder::new()
                .name("export disk".into())
                .spawn(move || {
                    let _ = tell.send(Disk {
                        folder_exists: folder.is_dir(),
                        free: jobs::free_space(&folder),
                        file_exists: file.exists(),
                    });
                    notify();
                });
            match spawned {
                Ok(_) => self.looking = Some(answer),
                Err(err) => tracing::error!("the disk check could not start: {err}"),
            }
        }
        self.found.clone()
    }

    /// Looks again at once (after a folder was made).
    pub fn again(&mut self) {
        self.asked = None;
    }

    pub fn settled(&self) -> bool {
        self.found.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            media: (3, 0),
            empty: false,
            no_pictures_here: false,
            encodable: Some(true),
            video: Some(VideoCodec::H264),
            other_codec: None,
            loudness: Some(Loudness::Measured(MixMeasured {
                integrated: -14.2,
                true_peak: -1.5,
            })),
            normalise: false,
            target: Some(-14.0),
            disk: Some(Disk {
                folder_exists: true,
                free: Some(412_000_000_000),
                file_exists: false,
            }),
            name: "Trip.mp4".into(),
            bad_name: false,
            needed: 240_000_000,
            sizes: Some(((1920, 1080), (1920, 1080))),
        }
    }

    fn texts(f: &Facts) -> Vec<(Level, String)> {
        list(f).into_iter().map(|c| (c.level, c.text)).collect()
    }

    #[test]
    fn all_well_reads_as_four_good_lines() {
        assert_eq!(
            texts(&facts()),
            [
                (Level::Good, "All 3 media files are found.".to_owned()),
                (Level::Good, "Loudness -14.2 LUFS, peaks at -1.5 dBTP.".to_owned()),
                (
                    Level::Good,
                    "Nothing needs rendering first: every effect is drawn as the file is written.".to_owned()
                ),
                (
                    Level::Good,
                    "412.0 GB free there; the export needs about 240 MB.".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn problems_are_said_with_their_fixes() {
        let f = Facts {
            media: (3, 2),
            loudness: Some(Loudness::Measured(MixMeasured {
                integrated: -9.0,
                true_peak: 0.3,
            })),
            disk: Some(Disk {
                folder_exists: true,
                free: Some(100_000_000),
                file_exists: true,
            }),
            sizes: Some(((1920, 1080), (3840, 2160))),
            ..facts()
        };
        let checks = list(&f);
        let fixes: Vec<_> = checks.iter().filter_map(|c| c.fix.map(|(fix, _)| fix)).collect();
        assert_eq!(
            fixes,
            [
                Fix::FindMissing,
                Fix::Normalise,
                Fix::TimelineSize,
                Fix::NewName,
                Fix::ChooseFolder
            ]
        );
        assert!(checks[0].text.starts_with("2 media files"));
        assert!(
            checks
                .iter()
                .any(|c| c.level == Level::Bad && c.text.starts_with("Not enough space"))
        );
        // Quiet, not over the peak: a word on how far from the aim.
        let quiet = Facts {
            loudness: Some(Loudness::Measured(MixMeasured {
                integrated: -20.0,
                true_peak: -6.0,
            })),
            ..facts()
        };
        assert_eq!(
            list(&quiet)[1].text,
            "Loudness -20.0 LUFS: 6.0 LU quieter than the -14 LUFS aimed for. Peaks at -6.0 dBTP."
        );
        // Normalising: said what will be done.
        let normalising = Facts {
            normalise: true,
            ..quiet
        };
        assert_eq!(
            list(&normalising)[1].text,
            "Loudness -20.0 LUFS now: it will be brought to -14 LUFS, peaks kept under -1 dBTP."
        );
    }

    #[test]
    fn what_stops_an_export_is_bad() {
        let f = Facts {
            empty: true,
            encodable: Some(false),
            video: Some(VideoCodec::H265),
            other_codec: Some(VideoCodec::H264),
            disk: Some(Disk {
                folder_exists: false,
                free: None,
                file_exists: false,
            }),
            name: "a:b".into(),
            bad_name: true,
            ..facts()
        };
        let bad: Vec<_> = list(&f)
            .into_iter()
            .filter(|c| c.level == Level::Bad)
            .map(|c| c.fix)
            .collect();
        assert_eq!(
            bad,
            [
                None,
                Some((Fix::UseCodec(VideoCodec::H264), "Use H.264")),
                Some((Fix::CleanName, "Replace them")),
                Some((Fix::MakeFolder, "Make it")),
            ]
        );
        let waiting = Facts {
            encodable: None,
            loudness: Some(Loudness::Measuring(0.4)),
            disk: None,
            ..facts()
        };
        let levels: Vec<_> = list(&waiting).into_iter().map(|c| c.level).collect();
        assert_eq!(levels.iter().filter(|l| **l == Level::Waiting).count(), 3);
        // Another shape: a note about the bars.
        let vertical = Facts {
            sizes: Some(((1920, 1080), (1080, 1920))),
            ..facts()
        };
        assert!(
            list(&vertical)
                .iter()
                .any(|c| c.level == Level::Note && c.text.contains("black bars above and below"))
        );
    }

    #[test]
    fn names_are_cleaned_and_made_free() {
        assert_eq!(clean_name("Cut 2: the \"best\" bits?"), "Cut 2- the -best- bits-");
        assert_eq!(free_name("Cut", |n| n == "Cut 2"), "Cut 3");
    }

    #[test]
    fn the_media_counted_are_those_the_timeline_plays_through_nested_ones() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture).unwrap();
        let project = project::Command::Import { sequence: seq, media }
            .apply(&project)
            .unwrap();
        let id = *project.media.keys().next().unwrap();
        assert_eq!(media_counts(&project, seq, &[]), (1, 0));
        assert_eq!(media_counts(&project, seq, &[id]), (1, 1));
    }

    #[test]
    fn the_disk_is_looked_at_on_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("out.mp4");
        std::fs::write(&file, b"x").unwrap();
        let mut watch = DiskWatch::default();
        let give_up = Instant::now() + Duration::from_secs(10);
        let found = loop {
            if let Some(found) = watch.look(dir.path(), &file, || {}) {
                break found;
            }
            assert!(Instant::now() < give_up);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(found.folder_exists && found.file_exists);
        assert!(watch.settled());
        let missing = dir.path().join("not here");
        assert_eq!(
            watch.look(&missing, &missing.join("a.mp4"), || {}),
            None,
            "asked again"
        );
    }
}
