//! Clearing the cache on disk (Settings › Cache): how much there is for the
//! open project and in all, and one click to clear either, after a
//! question that says how much goes.
//!
//! The only cache on disk today is proxies (`cache::proxy`). They belong
//! to recordings, not projects: "this project's" are the proxies of its
//! video files, which other projects using the same recordings share.
//! Clearing never touches recordings or project files (it only removes
//! proxy files from the cache folder). Waveforms, clip pictures and the
//! frames kept on the graphics card live in memory and go when the editor
//! closes, so there is nothing to clear there.
//!
//! Measuring and clearing run on workers; the Settings window shows
//! "Measuring…" meanwhile.

use cache::proxy::{Shelf, Usage};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use std::path::PathBuf;
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

/// What the cache takes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measured {
    /// The open project's part (nothing without a project).
    pub project: Usage,
    pub all: Usage,
}

/// Something the page asks for after drawing the rows or the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Clearing started: the project's proxies stop being built by
    /// themselves this session (`ProxyBuilds::cleared`).
    Clearing(Scope),
}

#[derive(Default)]
pub struct CacheCleanup {
    measuring: Option<JobHandle<Measured>>,
    measured: Option<Measured>,
    /// The question on screen: clear this, of this size?
    asking: Option<(Scope, Usage)>,
    clearing: Option<(Scope, JobHandle<Usage>)>,
    /// What the last clearing did, for the rows to say.
    said: Option<String>,
}

impl CacheCleanup {
    /// Measures again on a worker (the Settings window opened, or a
    /// clearing ended). `project` are the open project's proxies.
    pub fn measure(&mut self, jobs: &JobSystem, shelf: &Arc<Shelf>, project: Vec<PathBuf>) {
        let shelf = Arc::clone(shelf);
        self.measuring = Some(jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "measure the cache",
            move |_| {
                Ok(Measured {
                    project: shelf.usage(Some(&project)),
                    all: shelf.usage(None),
                })
            },
        ));
    }

    /// Takes in finished work. Returns an error to show (a clearing that
    /// could not remove everything) and whether to measure again.
    pub fn poll(&mut self) -> (Option<diag::AppError>, bool) {
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
            return (None, false);
        };
        let Some(result) = handle.try_result() else {
            return (None, false);
        };
        let scope = *scope;
        self.clearing = None;
        match result {
            Ok(cleared) => {
                self.said = Some(cleared_says(scope, cleared));
                (None, true)
            }
            Err(err) => (Some(err), true),
        }
    }

    pub fn busy(&self) -> bool {
        self.measuring.is_some() || self.clearing.is_some()
    }

    /// The Settings window's Cache rows: what there is, and the two
    /// buttons, which ask first. `has_project`: a project is open.
    pub fn rows(&mut self, ui: &mut egui::Ui, theme: &Theme, has_project: bool) {
        let measured = self.measured;
        let line = match (measured, &self.clearing) {
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
        ui.horizontal(|ui| {
            let project = measured.map(|m| m.project).filter(|u| u.files > 0);
            let clear_project = ui
                .add_enabled(
                    idle && has_project && project.is_some(),
                    egui::Button::new("Clear cache for this project…"),
                )
                .on_hover_text("Deletes the proxies of this project's recordings. Asks first.")
                .on_disabled_hover_text("This project has nothing in the cache.");
            if clear_project.clicked()
                && let Some(usage) = project
            {
                self.asking = Some((Scope::Project, usage));
            }
            let all = measured.map(|m| m.all).filter(|u| u.files > 0);
            let clear_all = ui
                .add_enabled(idle && all.is_some(), egui::Button::new("Clear all caches…"))
                .on_hover_text("Deletes every proxy, for every project. Asks first.")
                .on_disabled_hover_text("The cache is empty.");
            if clear_all.clicked()
                && let Some(usage) = all
            {
                self.asking = Some((Scope::All, usage));
            }
        });
        let note = self.said.clone().unwrap_or_else(|| {
            "Proxies are kept by recording and shared by the projects that use it. Recordings and project files are never touched.".to_owned()
        });
        ui.label(egui::RichText::new(note).color(theme.muted));
    }

    /// The question before clearing, when one was asked for. Starts the
    /// clearing on a worker on yes.
    pub fn question(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        jobs: &JobSystem,
        shelf: &Arc<Shelf>,
        project: Vec<PathBuf>,
    ) -> Option<Asked> {
        let (scope, usage) = self.asking?;
        let (mut yes, mut no) = (false, false);
        let modal = ui_kit::dialog(ctx, theme, "clear-cache", |ui| {
            ui.set_width(380.0);
            let (title, body) = question_words(scope, usage);
            ui.label(
                egui::RichText::new(title)
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(body).color(theme.text_secondary));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, theme, &format!("Clear {}", size_text(usage.bytes))).clicked() {
                    yes = true;
                }
                if ui.button("Cancel").clicked() {
                    no = true;
                }
            });
        });
        if no || modal.should_close() {
            self.asking = None;
            return None;
        }
        if !yes {
            return None;
        }
        self.asking = None;
        self.said = None;
        let shelf = Arc::clone(shelf);
        let only = match scope {
            Scope::Project => Some(project),
            Scope::All => None,
        };
        self.clearing = Some((
            scope,
            jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "clear the cache",
                move |_| shelf.clear(only.as_deref()),
            ),
        ));
        Some(Asked::Clearing(scope))
    }
}

/// The question's title and words: what goes, how much, and what stays.
pub fn question_words(scope: Scope, usage: Usage) -> (&'static str, String) {
    let size = size_text(usage.bytes);
    let proxies = count_text(usage.files);
    match scope {
        Scope::Project => (
            "Clear the cache for this project?",
            format!(
                "Deletes {size}: {proxies} of this project's recordings. Other projects that use the same recordings lose them too. Recordings and project files are not touched; a proxy can be built again from the media drawer."
            ),
        ),
        Scope::All => (
            "Clear all caches?",
            format!(
                "Deletes {size}: {proxies}, for every project. Recordings and project files are not touched; a proxy can be built again from the media drawer."
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
        "Cleared {} from {whose} ({}). They are not built again by themselves until the editor starts again.",
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

/// "1.2 GB (3 proxies)", or "empty".
fn usage_text(usage: Usage) -> String {
    if usage.files == 0 {
        "empty".to_owned()
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

    #[test]
    fn sizes_are_written_simply() {
        assert_eq!(size_text(0), "0 KB");
        assert_eq!(size_text(1), "1 KB");
        assert_eq!(size_text(640_000), "640 KB");
        assert_eq!(size_text(12_400_000), "12 MB");
        assert_eq!(size_text(1_234_000_000), "1.2 GB");
        assert_eq!(usage_text(Usage::default()), "empty");
        assert_eq!(
            usage_text(Usage {
                bytes: 2_500_000_000,
                files: 1
            }),
            "2.5 GB (1 proxy)"
        );
    }

    #[test]
    fn the_question_says_how_much_goes_and_that_recordings_stay() {
        let usage = Usage {
            bytes: 1_200_000_000,
            files: 3,
        };
        let (title, body) = question_words(Scope::Project, usage);
        assert!(title.contains("this project"));
        assert!(body.contains("1.2 GB") && body.contains("3 proxies"), "{body}");
        assert!(body.contains("Recordings and project files are not touched"));
        let (title, body) = question_words(Scope::All, usage);
        assert!(title.contains("all"));
        assert!(body.contains("every project"));
    }

    #[test]
    fn measuring_and_clearing_go_through_the_shelf_on_workers() {
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::new(dir.path(), cache::proxy::ProxyMode::Auto);
        let ours = dir.path().join("1-aa-0.mp4");
        let theirs = dir.path().join("2-bb-0.mp4");
        std::fs::write(&ours, vec![0u8; 3000]).unwrap();
        std::fs::write(&theirs, vec![0u8; 5000]).unwrap();
        let mut cleanup = CacheCleanup::default();
        cleanup.measure(&jobs, &shelf, vec![ours.clone()]);
        while cleanup.busy() {
            cleanup.poll();
            std::thread::yield_now();
        }
        let measured = cleanup.measured.unwrap();
        assert_eq!(
            measured.project,
            Usage {
                bytes: 3000,
                files: 1
            }
        );
        assert_eq!(
            measured.all,
            Usage {
                bytes: 8000,
                files: 2
            }
        );
        // Clearing the project's leaves the other project's.
        let cleared = shelf.clear(Some(std::slice::from_ref(&ours))).unwrap();
        assert_eq!(
            cleared,
            Usage {
                bytes: 3000,
                files: 1
            }
        );
        assert!(!ours.exists() && theirs.exists());
        assert!(cleared_says(Scope::Project, cleared).contains("3 KB"));
    }
}
