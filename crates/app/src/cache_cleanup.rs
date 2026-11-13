//! Clearing the cache on disk (Settings › Cache): how much there is for the
//! open project and in all, and one click to clear either, after a
//! question that says how much goes.
//!
//! The only cache on disk today is proxies (`cache::proxy`). They belong
//! to recordings, not projects: "this project's" are the proxies of its
//! video files, and other projects may use the same recordings. So the
//! question for this project offers a choice each time: only the proxies
//! of recordings no other project uses, or also the shared ones (other
//! projects lose those too). Which recordings other projects use comes
//! from the projects the launcher knows (`project_index`): each one's file
//! is read on the worker. A project file that is gone is passed over; one
//! that is there but can't be read counts as using every recording, so on
//! the safe side nothing of this project's is "only this project's".
//!
//! Clearing never touches recordings or project files (it only removes
//! proxy files from the cache folder), and a cleared proxy is not built
//! again by itself, ever, until "Build proxy now" (`Shelf::is_cleared`).
//! Waveforms, clip pictures and the frames kept on the graphics card live
//! in memory and go when the editor closes, so there is nothing to clear
//! there.
//!
//! Measuring and clearing run on workers; the Settings window shows
//! "Measuring…" meanwhile.

use cache::proxy::{Shelf, Usage};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui_kit::Theme;

/// Which cache a clearing is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The proxies of the open project's files.
    Project,
    /// Every proxy, whichever project it was made for.
    All,
}

/// For this project: which of its proxies go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Only proxies of recordings no other project uses (the usual).
    OnlyOwn,
    /// Every proxy of this project's recordings: other projects that use
    /// the same recordings lose them too.
    AlsoShared,
}

/// What the cache takes on disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Measured {
    /// This project's proxies of recordings no other project uses.
    pub own: Usage,
    /// Where those are.
    pub own_paths: Vec<PathBuf>,
    /// All of the open project's proxies (nothing without a project).
    pub project: Usage,
    pub all: Usage,
}

/// Where to look for the projects other than the open one.
#[derive(Debug, Clone)]
pub struct OtherProjects {
    /// The launcher's list of known projects.
    pub index_file: PathBuf,
    /// The open project's file, which is not another project.
    pub this_project: Option<PathBuf>,
}

/// What a finished clearing hands the page.
#[derive(Debug, Default)]
pub struct Polled {
    /// A clearing that could not remove everything.
    pub problem: Option<diag::AppError>,
    /// Measure again (a clearing ended).
    pub measure_again: bool,
    /// The proxies a clearing removed (`ProxyBuilds::cleared`).
    pub cleared: Vec<PathBuf>,
}

/// The question on screen.
#[derive(Debug, Clone)]
struct Question {
    scope: Scope,
    choice: Choice,
}

#[derive(Default)]
pub struct CacheCleanup {
    measuring: Option<JobHandle<Measured>>,
    measured: Option<Measured>,
    asking: Option<Question>,
    clearing: Option<(Scope, JobHandle<cache::proxy::Cleared>)>,
    /// What the last clearing did, for the rows to say.
    said: Option<String>,
}

impl CacheCleanup {
    /// Measures again on a worker (the Settings window opened, or a
    /// clearing ended). `project` are the open project's proxies.
    pub fn measure(
        &mut self,
        jobs: &JobSystem,
        shelf: &Arc<Shelf>,
        project: Vec<PathBuf>,
        others: OtherProjects,
    ) {
        let shelf = Arc::clone(shelf);
        self.measuring = Some(jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "measure the cache",
            move |_| Ok(measure(&shelf, project, &others)),
        ));
    }

    /// Takes in finished work.
    pub fn poll(&mut self) -> Polled {
        if let Some(handle) = &self.measuring
            && let Some(result) = handle.try_result()
        {
            self.measuring = None;
            match result {
                Ok(measured) => self.measured = Some(measured),
                Err(err) => tracing::warn!("the cache could not be measured: {}", err.report()),
            }
        }
        let Some((scope, handle)) = &self.clearing else {
            return Polled::default();
        };
        let Some(result) = handle.try_result() else {
            return Polled::default();
        };
        let scope = *scope;
        self.clearing = None;
        match result {
            Ok(cleared) => {
                self.said = Some(cleared_says(scope, cleared.usage));
                Polled {
                    problem: cleared.problem,
                    measure_again: true,
                    cleared: cleared.paths,
                }
            }
            Err(err) => Polled {
                problem: Some(err),
                measure_again: true,
                cleared: Vec::new(),
            },
        }
    }

    pub fn busy(&self) -> bool {
        self.measuring.is_some() || self.clearing.is_some()
    }

    /// The Settings window's Cache rows: what there is, and the two
    /// buttons, which ask first. `has_project`: a project is open.
    pub fn rows(&mut self, ui: &mut egui::Ui, theme: &Theme, has_project: bool) {
        let line = match (&self.measured, &self.clearing) {
            (_, Some(_)) => "Clearing…".to_owned(),
            (Some(m), None) if has_project => format!(
                "This project: {} · All projects: {}",
                usage_text(m.project),
                usage_text(m.all)
            ),
            (Some(m), None) => format!("All projects: {}", usage_text(m.all)),
            (None, None) => "Measuring…".to_owned(),
        };
        ui.label(line);
        let idle = self.clearing.is_none();
        let (project, all) = self
            .measured
            .as_ref()
            .map_or((0, 0), |m| (m.project.files, m.all.files));
        ui.horizontal(|ui| {
            let clear_project = ui
                .add_enabled(
                    idle && has_project && project > 0,
                    egui::Button::new("Clear cache for this project…"),
                )
                .on_hover_text("Deletes the proxies of this project's recordings. Asks first.")
                .on_disabled_hover_text("This project has nothing in the cache.");
            if clear_project.clicked() {
                self.asking = Some(Question {
                    scope: Scope::Project,
                    choice: Choice::OnlyOwn,
                });
            }
            let clear_all = ui
                .add_enabled(idle && all > 0, egui::Button::new("Clear all caches…"))
                .on_hover_text("Deletes every proxy, for every project. Asks first.")
                .on_disabled_hover_text("The cache is empty.");
            if clear_all.clicked() {
                self.asking = Some(Question {
                    scope: Scope::All,
                    choice: Choice::AlsoShared,
                });
            }
        });
        let note = self.said.clone().unwrap_or_else(|| {
            "Proxies are kept by recording and shared by the projects that use it. Recordings and project files are never touched.".to_owned()
        });
        ui.label(egui::RichText::new(note).color(theme.muted));
    }

    /// The question before clearing, when one was asked for. Starts the
    /// clearing on a worker on yes, and says so (the page stops the build
    /// under way).
    pub fn question(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        jobs: &JobSystem,
        shelf: &Arc<Shelf>,
        project: Vec<PathBuf>,
    ) -> bool {
        let (Some(question), Some(measured)) = (&mut self.asking, &self.measured) else {
            return false;
        };
        let (mut yes, mut no) = (false, false);
        let modal = ui_kit::dialog(ctx, theme, "clear-cache", |ui| {
            ui.set_width(400.0);
            let (title, body) = question_words(question.scope, measured);
            ui.label(
                egui::RichText::new(title)
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(body).color(theme.text_secondary));
            if question.scope == Scope::Project {
                ui.add_space(8.0);
                for (choice, words) in choice_lines(measured) {
                    ui.radio_value(&mut question.choice, choice, words);
                }
            }
            ui.add_space(10.0);
            let going = going(question.scope, question.choice, measured);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clear = format!("Clear {}", size_text(going.bytes));
                let button =
                    ui.add_enabled_ui(going.files > 0, |ui| ui_kit::accent_button(ui, theme, &clear));
                if button.inner.clicked() {
                    yes = true;
                }
                if ui.button("Cancel").clicked() {
                    no = true;
                }
            });
        });
        if no || modal.should_close() {
            self.asking = None;
            return false;
        }
        if !yes {
            return false;
        }
        let Some(question) = self.asking.take() else {
            return false;
        };
        let only = match (question.scope, question.choice) {
            (Scope::All, _) => None,
            (Scope::Project, Choice::OnlyOwn) => Some(measured.own_paths.clone()),
            (Scope::Project, Choice::AlsoShared) => Some(project),
        };
        self.said = None;
        let shelf = Arc::clone(shelf);
        self.clearing = Some((
            question.scope,
            jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "clear the cache",
                move |_| Ok(shelf.clear(only.as_deref())),
            ),
        ));
        true
    }
}

/// Measures the cache: all of it, the open project's proxies, and those
/// of them no other known project uses. Reads the folder and the other
/// projects' files: on a worker.
fn measure(shelf: &Shelf, project: Vec<PathBuf>, others: &OtherProjects) -> Measured {
    let own_paths = match shared_proxies(shelf, others) {
        Some(shared) => project.iter().filter(|p| !shared.contains(*p)).cloned().collect(),
        None => Vec::new(),
    };
    Measured {
        own: shelf.usage(Some(&own_paths)),
        own_paths,
        project: shelf.usage(Some(&project)),
        all: shelf.usage(None),
    }
}

/// Where the proxies of every recording the other known projects use are.
/// `None` when that can't be told (a project that is there can't be
/// read, or the list can't be): then every recording counts as shared.
fn shared_proxies(shelf: &Shelf, others: &OtherProjects) -> Option<HashSet<PathBuf>> {
    let mut shared = HashSet::new();
    for path in other_projects(others)? {
        if !path.exists() {
            continue; // gone: it uses nothing now
        }
        match project::file::load(&path) {
            Ok(project) => shared.extend(proxies_of(shelf, &project)),
            Err(err) => {
                tracing::info!(project = %path.display(), "counted as sharing every recording: {}", err.report());
                return None;
            }
        }
    }
    Some(shared)
}

/// The project files the launcher knows, other than the open one (listed
/// or removed from the list). No list yet: none. `None` when the list
/// can't be read.
fn other_projects(others: &OtherProjects) -> Option<Vec<PathBuf>> {
    if !others.index_file.exists() {
        return Some(Vec::new());
    }
    let known = project_index::Index::open(&others.index_file)
        .and_then(|index| index.known_paths())
        .map_err(|err| tracing::info!("the list of projects can't be read: {}", err.report()))
        .ok()?;
    let this = others.this_project.as_deref().map(same_file);
    Some(known.into_iter().filter(|p| Some(same_file(p)) != this).collect())
}

/// A path as it can be compared: the real one when it can be found.
fn same_file(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Where the proxies of `project`'s video files are (or would be).
pub fn proxies_of(shelf: &Shelf, project: &project::Project) -> Vec<PathBuf> {
    project
        .media
        .values()
        .filter_map(|m| crate::proxies::video_stream(m).map(|stream| shelf.file_for(&m.fingerprint, stream)))
        .collect()
}

/// The proxies a choice clears.
fn going(scope: Scope, choice: Choice, measured: &Measured) -> Usage {
    match (scope, choice) {
        (Scope::All, _) => measured.all,
        (Scope::Project, Choice::OnlyOwn) => measured.own,
        (Scope::Project, Choice::AlsoShared) => measured.project,
    }
}

/// The two choices for this project's cache, each with what it clears.
pub fn choice_lines(measured: &Measured) -> [(Choice, String); 2] {
    let shared = Usage {
        bytes: measured.project.bytes.saturating_sub(measured.own.bytes),
        files: measured.project.files.saturating_sub(measured.own.files),
    };
    [
        (
            Choice::OnlyOwn,
            format!(
                "Only this project's: {} (recordings no other project uses)",
                usage_text(measured.own)
            ),
        ),
        (
            Choice::AlsoShared,
            if shared.files == 0 {
                format!(
                    "Also shared ones: {} (no other project uses these recordings)",
                    usage_text(measured.project)
                )
            } else {
                format!(
                    "Also shared ones: {}, of which other projects use {} and lose it",
                    usage_text(measured.project),
                    usage_text(shared)
                )
            },
        ),
    ]
}

/// The question's title and words: what goes and what stays.
pub fn question_words(scope: Scope, measured: &Measured) -> (&'static str, String) {
    match scope {
        Scope::Project => (
            "Clear the cache for this project?",
            "Deletes the proxies of this project's recordings. Recordings and project files are not touched. Cleared proxies are not built again by themselves (also under Auto): \"Build proxy now\" or \"Build proxies\" (a file's right-click menu, on the Library page or in the media drawer) makes them, and so does switching Proxies to On.".to_owned(),
        ),
        Scope::All => (
            "Clear all caches?",
            format!(
                "Deletes {}: {}, for every project. Recordings and project files are not touched. Cleared proxies are not built again by themselves (also under Auto): \"Build proxy now\" or \"Build proxies\" (a file's right-click menu, on the Library page or in the media drawer) makes them, and so does switching Proxies to On.",
                size_text(measured.all.bytes),
                count_text(measured.all.files)
            ),
        ),
    }
}

/// What a finished clearing says.
fn cleared_says(scope: Scope, cleared: Usage) -> String {
    let whose = match scope {
        Scope::Project => "this project's cache",
        Scope::All => "the cache",
    };
    format!(
        "Cleared {} from {whose} ({}). They are not built again by themselves: \"Build proxies\" or switching Proxies to On makes them again.",
        size_text(cleared.bytes),
        count_text(cleared.files)
    )
}

/// "3 proxies", "1 proxy".
fn count_text(files: usize) -> String {
    if files == 1 {
        "1 proxy".to_owned()
    } else {
        format!("{files} proxies")
    }
}

/// "1.2 GB (3 proxies)", or "nothing".
fn usage_text(usage: Usage) -> String {
    if usage.files == 0 {
        "nothing".to_owned()
    } else {
        format!("{} ({})", size_text(usage.bytes), count_text(usage.files))
    }
}

/// A size on disk in words: "640 KB", "12 MB", "1.2 GB".
pub fn size_text(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.0} MB", b / (KB * KB))
    } else {
        format!("{:.0} KB", (b / KB).ceil())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cache::proxy::ProxyMode;

    #[test]
    fn sizes_are_written_simply() {
        assert_eq!(size_text(0), "0 KB");
        assert_eq!(size_text(1), "1 KB");
        assert_eq!(size_text(640_000), "640 KB");
        assert_eq!(size_text(12_400_000), "12 MB");
        assert_eq!(size_text(1_234_000_000), "1.2 GB");
        assert_eq!(usage_text(Usage::default()), "nothing");
        assert_eq!(
            usage_text(Usage {
                bytes: 2_500_000_000,
                files: 1
            }),
            "2.5 GB (1 proxy)"
        );
    }

    fn measured() -> Measured {
        Measured {
            own: Usage {
                bytes: 1_000_000_000,
                files: 1,
            },
            own_paths: vec![PathBuf::from("a.mp4")],
            project: Usage {
                bytes: 1_200_000_000,
                files: 3,
            },
            all: Usage {
                bytes: 5_000_000_000,
                files: 9,
            },
        }
    }

    #[test]
    fn the_question_offers_both_choices_with_their_sizes() {
        let m = measured();
        let [(first, own), (second, also)] = choice_lines(&m);
        assert_eq!((first, second), (Choice::OnlyOwn, Choice::AlsoShared));
        assert!(own.contains("1.0 GB (1 proxy)"), "{own}");
        assert!(
            also.contains("1.2 GB (3 proxies)") && also.contains("200 MB (2 proxies)"),
            "{also}"
        );
        let alone = Measured {
            own: m.project,
            ..measured()
        };
        assert!(choice_lines(&alone)[1].1.contains("no other project"));
        assert_eq!(going(Scope::Project, Choice::OnlyOwn, &m), m.own);
        assert_eq!(going(Scope::Project, Choice::AlsoShared, &m), m.project);
        assert_eq!(going(Scope::All, Choice::OnlyOwn, &m), m.all, "all is all");
        let (title, body) = question_words(Scope::Project, &m);
        assert!(title.contains("this project"));
        assert!(body.contains("Recordings and project files are not touched"));
        let (title, body) = question_words(Scope::All, &m);
        assert!(title.contains("all"));
        assert!(body.contains("5.0 GB") && body.contains("9 proxies") && body.contains("every project"));
    }

    /// A project of one video file with this fingerprint, saved at `path`.
    fn project_using(path: &Path, size: u64) -> project::Project {
        let mut p = project::Project::new_automatic("P");
        let seq = p.active_sequence;
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let mut media = media::describe(&fixture).unwrap();
        media.fingerprint = project::Fingerprint {
            size,
            sample_hash: format!("{size:x}"),
        };
        p = project::Command::Import { sequence: seq, media }
            .apply(&p)
            .unwrap();
        project::file::save(&p, path).unwrap();
        p
    }

    #[test]
    fn recordings_other_projects_use_are_shared_and_unreadable_projects_share_everything() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path().join("proxies"), ProxyMode::Auto);
        let ext = brand::PROJECT_FILE_EXTENSION;
        let (ours_file, theirs_file) = (
            dir.path().join(format!("ours.{ext}")),
            dir.path().join(format!("theirs.{ext}")),
        );
        let ours = project_using(&ours_file, 1);
        // Ours: recordings 1 (and 2, which the other project uses too).
        let mut both = ours.clone();
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let mut second = media::describe(&fixture).unwrap();
        second.fingerprint = project::Fingerprint {
            size: 2,
            sample_hash: "2".into(),
        };
        both = project::Command::Import {
            sequence: both.active_sequence,
            media: second,
        }
        .apply(&both)
        .unwrap();
        let _theirs = project_using(&theirs_file, 2);
        let project = proxies_of(&shelf, &both);
        std::fs::create_dir_all(shelf.folder()).unwrap();
        for (path, bytes) in project.iter().zip([1000, 3000]) {
            std::fs::write(path, vec![0u8; bytes]).unwrap();
        }
        let index_file = dir.path().join(project_index::INDEX_FILE_NAME);
        let index = project_index::Index::open(&index_file).unwrap();
        index.record(&ours_file).unwrap();
        index.record(&theirs_file).unwrap();
        // One known project that is gone: passed over.
        let gone = dir.path().join(format!("gone.{}", brand::PROJECT_FILE_EXTENSION));
        project_using(&gone, 3);
        index.record(&gone).unwrap();
        std::fs::remove_file(&gone).unwrap();
        drop(index);
        let others = OtherProjects {
            index_file: index_file.clone(),
            this_project: Some(ours_file.clone()),
        };
        let m = measure(&shelf, project.clone(), &others);
        assert_eq!(
            m.project,
            Usage {
                bytes: 4000,
                files: 2
            }
        );
        assert_eq!(m.own.files, 1, "the recording the other project uses is shared");
        assert!(
            m.own_paths.iter().all(|p| p.to_string_lossy().contains("1-1-")),
            "{:?}",
            m.own_paths
        );
        // A project that is there but can't be read: every recording is shared.
        std::fs::write(&theirs_file, "not a project").unwrap();
        let m = measure(&shelf, project.clone(), &others);
        assert_eq!(m.own, Usage::default());
        assert_eq!(m.project.files, 2);
        // No list of projects yet: nothing is shared.
        let alone = OtherProjects {
            index_file: dir.path().join("none.sqlite"),
            this_project: Some(ours_file),
        };
        assert_eq!(measure(&shelf, project, &alone).own.files, 2);
    }

    #[test]
    fn measuring_and_clearing_go_through_the_shelf_on_workers() {
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path(), ProxyMode::Auto);
        let ours = dir.path().join("1-aa-0.mp4");
        let theirs = dir.path().join("2-bb-0.mp4");
        std::fs::write(&ours, vec![0u8; 3000]).unwrap();
        std::fs::write(&theirs, vec![0u8; 5000]).unwrap();
        let mut cleanup = CacheCleanup::default();
        let others = OtherProjects {
            index_file: dir.path().join("none.sqlite"),
            this_project: None,
        };
        cleanup.measure(&jobs, &shelf, vec![ours.clone()], others);
        while cleanup.busy() {
            cleanup.poll();
            std::thread::yield_now();
        }
        let measured = cleanup.measured.clone().unwrap();
        assert_eq!(
            measured.project,
            Usage {
                bytes: 3000,
                files: 1
            }
        );
        assert_eq!(measured.own, measured.project, "no other project");
        assert_eq!(
            measured.all,
            Usage {
                bytes: 8000,
                files: 2
            }
        );
        // Clearing the project's leaves the other recording's.
        let cleared = shelf.clear(Some(std::slice::from_ref(&ours)));
        assert_eq!(
            cleared.usage,
            Usage {
                bytes: 3000,
                files: 1
            }
        );
        assert!(!ours.exists() && theirs.exists());
        assert!(cleared_says(Scope::Project, cleared.usage).contains("3 KB"));
    }
}
