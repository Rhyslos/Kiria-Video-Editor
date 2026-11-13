//! Package project (`Action::PackageProject`): the project and only the
//! media its timelines use, copied into one new folder that opens on
//! another machine with nothing missing (`project::package`).
//!
//! The dialog asks for a folder (the system's folder picker, on its own
//! thread), works out what goes in on a worker (file sizes, and the free
//! space on that drive), and shows it before anything is written: how many
//! files, how much in all, the free space, files left out because no
//! timeline uses them, and files that can't be found (packaging waits for
//! those). Copying runs on a worker with progress, time left and Cancel;
//! a cancelled or failed package leaves nothing behind, and the originals
//! are only read.

use crate::cache_cleanup::size_text;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::Project;
use project::package::{self, Outcome, Plan};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use ui_kit::Theme;

/// What was worked out before copying.
#[derive(Debug)]
pub struct Planned {
    pub plan: Plan,
    /// Free space on the drive the package goes to, when it can be told.
    pub free: Option<u64>,
}

struct Copying {
    handle: JobHandle<Outcome>,
    done: Arc<AtomicU64>,
    total: u64,
    started: Instant,
    folder: PathBuf,
}

#[derive(Default)]
pub struct PackageWindow {
    open: bool,
    picking: Option<Receiver<Option<PathBuf>>>,
    planning: Option<JobHandle<Planned>>,
    planned: Option<Planned>,
    copying: Option<Copying>,
    /// `--hold-package`: copying stops at this share, for screenshots.
    hold: Option<f32>,
    /// "Include saved versions", asked each time: off when the dialog opens.
    with_versions: bool,
}

/// What the dialog asked for this frame.
#[derive(Debug)]
pub enum Asked {
    Nothing,
    /// Packaging ended; for the status line.
    Said(String),
    Failed(diag::AppError),
}

impl PackageWindow {
    pub fn holding_at(hold: Option<f32>) -> PackageWindow {
        PackageWindow {
            hold,
            ..PackageWindow::default()
        }
    }

    /// Opens the dialog and asks for a folder to package into.
    pub fn open(&mut self, ctx: &egui::Context) {
        if self.copying.is_some() {
            self.open = true;
            return;
        }
        *self = PackageWindow {
            open: true,
            hold: self.hold,
            ..PackageWindow::default()
        };
        self.pick(ctx);
    }

    /// Opens the dialog with the folder already chosen (`--package-to`).
    pub fn open_into(&mut self, jobs: &JobSystem, project: &Project, project_file: &Path, into: PathBuf) {
        self.open = true;
        self.start_plan(jobs, project, project_file, into);
    }

    fn pick(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Where should the package go?")
                .pick_folder();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.picking = Some(rx);
    }

    fn start_plan(&mut self, jobs: &JobSystem, project: &Project, project_file: &Path, into: PathBuf) {
        let (project, file) = (project.clone(), project_file.to_path_buf());
        self.planned = None;
        self.planning = Some(jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "plan a package",
            move |_| {
                let plan = package::plan(&project, &file, &into)?;
                Ok(Planned {
                    free: jobs::free_space(&into),
                    plan,
                })
            },
        ));
    }

    fn start_copy(&mut self, jobs: &JobSystem, ctx: &egui::Context) {
        let Some(planned) = self.planned.take() else {
            return;
        };
        let done = Arc::new(AtomicU64::new(0));
        let with_versions = self.with_versions;
        let total = planned.plan.total_bytes(with_versions);
        let folder = planned.plan.folder.clone();
        let (counter, hold, ctx) = (Arc::clone(&done), self.hold, ctx.clone());
        let handle = jobs.submit(
            PoolKind::Export,
            Priority::Interactive,
            "package project",
            move |job| {
                let hold_at = hold.map(|h| (f64::from(h) * total as f64) as u64);
                let outcome = package::run(&planned.plan, with_versions, &counter, || {
                    ctx.request_repaint();
                    if let Some(at) = hold_at {
                        while counter.load(Ordering::Relaxed) >= at && !job.is_cancelled() {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                    job.is_cancelled()
                });
                ctx.request_repaint();
                outcome
            },
        );
        self.copying = Some(Copying {
            handle,
            done,
            total,
            started: Instant::now(),
            folder,
        });
    }

    /// Whether copying is going on (the page repaints to show progress).
    pub fn busy(&self) -> bool {
        self.copying.is_some() || self.planning.is_some()
    }

    /// Gives up packaging under way (the editor is closing); the worker
    /// takes away what it copied.
    pub fn stop(&mut self) {
        if let Some(copying) = &self.copying {
            copying.handle.cancel();
        }
    }

    /// For screenshots: the plan is shown, and with `--hold-package`
    /// copying has reached the share it stops at.
    pub fn settled(&self) -> bool {
        if !self.open {
            return true;
        }
        match (self.hold, &self.copying) {
            (Some(hold), Some(copying)) => {
                copying.done.load(Ordering::Relaxed) as f64 >= f64::from(hold) * copying.total as f64
            }
            (Some(_), None) => false,
            (None, _) => self.planning.is_none() && self.picking.is_none(),
        }
    }

    /// The dialog, and finished work. `project` and `project_file` are the
    /// open project, packaged as it is when the folder is chosen.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        jobs: &JobSystem,
        project: &Project,
        project_file: &Path,
    ) -> Asked {
        if let Some(rx) = &self.picking
            && let Ok(picked) = rx.try_recv()
        {
            self.picking = None;
            match picked {
                Some(into) => self.start_plan(jobs, project, project_file, into),
                None if self.planned.is_none() => self.open = false,
                None => {}
            }
        }
        let mut asked = Asked::Nothing;
        if let Some(handle) = &self.planning
            && let Some(result) = handle.try_result()
        {
            self.planning = None;
            match result {
                Ok(planned) => {
                    self.planned = Some(planned);
                    // `--hold-package`: start at once, for screenshots of copying.
                    if self.hold.is_some() {
                        self.start_copy(jobs, ctx);
                    }
                }
                Err(err) => {
                    self.open = false;
                    return Asked::Failed(err);
                }
            }
        }
        if let Some(copying) = &self.copying
            && let Some(result) = copying.handle.try_result()
        {
            let folder = copying.folder.clone();
            self.copying = None;
            self.open = false;
            return match result {
                Ok(Outcome::Packaged) => Asked::Said(format!(
                    "Packaged the project into {}: open that folder's project on any machine, nothing is missing.",
                    folder.display()
                )),
                Ok(Outcome::Cancelled) => {
                    Asked::Said("Packaging cancelled; nothing was left behind.".to_owned())
                }
                Err(err) => Asked::Failed(err),
            };
        }
        if !self.open {
            return asked;
        }
        let (mut choose, mut start, mut cancel, mut close) = (false, false, false, false);
        let modal = ui_kit::dialog(ctx, theme, "package-project", |ui| {
            ui.set_width(480.0);
            ui.label(
                egui::RichText::new("Package project")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Copies the project and only the media its timelines use into one new folder, which opens on another machine with nothing missing. The originals stay as they are.",
                )
                .color(theme.text_secondary),
            );
            ui.add_space(10.0);
            if let Some(copying) = &self.copying {
                progress(ui, theme, copying);
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Cancel")
                        .on_hover_text("Stop; nothing is left behind")
                        .clicked()
                    {
                        cancel = true;
                    }
                });
                return;
            }
            if self.picking.is_some() || self.planning.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if self.picking.is_some() {
                        "Choose a folder…"
                    } else {
                        "Adding up what goes in…"
                    });
                });
            }
            let with_versions = &mut self.with_versions;
            let ready = self
                .planned
                .as_ref()
                .map(|planned| summary(ui, theme, planned, with_versions));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_start = ready == Some(true);
                let words = self.planned.as_ref().map_or("Package".to_owned(), |p| {
                    format!("Package {}", size_text(p.plan.total_bytes(self.with_versions)))
                });
                if ui
                    .add_enabled_ui(can_start, |ui| ui_kit::accent_button(ui, theme, &words))
                    .inner
                    .clicked()
                {
                    start = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if self.picking.is_none()
                    && self.planning.is_none()
                    && ui.button("Choose another folder…").clicked()
                {
                    choose = true;
                }
            });
        });
        if cancel && let Some(copying) = &self.copying {
            copying.handle.cancel();
        }
        if self.copying.is_none() && (close || modal.should_close()) {
            self.open = false;
            self.planned = None;
            return asked;
        }
        if choose {
            self.pick(ctx);
        }
        if start {
            self.start_copy(jobs, ctx);
            asked = Asked::Nothing;
        }
        asked
    }
}

/// What goes in, before anything is written. Returns whether it can start.
fn summary(ui: &mut egui::Ui, theme: &Theme, planned: &Planned, with_versions: &mut bool) -> bool {
    let plan = &planned.plan;
    let total = plan.total_bytes(*with_versions);
    row(ui, theme, "Into", &plan.folder.display().to_string());
    row(
        ui,
        theme,
        "Media",
        &match plan.media_files(*with_versions) {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        },
    );
    row(ui, theme, "In all", &size_text(total));
    let fits = planned.free.is_none_or(|free| free > total);
    let free_text = planned.free.map_or("can't be told".to_owned(), |free| {
        format!("{} free on that drive", size_text(free))
    });
    row(ui, theme, "Space", &free_text);
    if plan.left_out > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} left out: no timeline uses {}.",
                match plan.left_out {
                    1 => "1 file is".to_owned(),
                    n => format!("{n} files are"),
                },
                if plan.left_out == 1 { "it" } else { "them" }
            ))
            .color(theme.muted),
        );
    }
    versions_box(ui, theme, &plan.versions, with_versions);
    ui.label(
        egui::RichText::new("Not copied: the autosave file and proxies (built again where it opens).")
            .color(theme.muted),
    );
    if !plan.missing.is_empty() {
        ui.colored_label(
            ui_kit::theme::status::ERROR,
            format!(
                "{} can't be found. Find them first (the banner's \"Find them…\"), then package.",
                match plan.missing.len() {
                    1 => "1 file the timelines use".to_owned(),
                    n => format!("{n} files the timelines use"),
                }
            ),
        );
    }
    if !fits {
        ui.colored_label(
            ui_kit::theme::status::ERROR,
            "There isn't room for it on that drive. Choose another folder, or make room.",
        );
    }
    plan.missing.is_empty() && fits
}

/// "Include saved versions (N, size)", asked each time, off at first; the
/// sizes above follow it.
fn versions_box(ui: &mut egui::Ui, theme: &Theme, versions: &package::Versions, with: &mut bool) {
    let count = versions.files.len();
    if count == 0 {
        *with = false;
        ui.add_enabled(false, egui::Checkbox::new(with, "Include saved versions (none)"));
        return;
    }
    ui.checkbox(with, versions_words(count, versions.bytes()))
        .on_hover_text(
            "Copies the saved versions too, with the files only they use, so any of them can be restored where the package opens.",
        );
    if *with && versions.missing > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} that only saved versions use can't be found: those versions will miss {}.",
                if versions.missing == 1 {
                    "1 file".to_owned()
                } else {
                    format!("{} files", versions.missing)
                },
                if versions.missing == 1 { "it" } else { "them" }
            ))
            .color(theme.muted),
        );
    }
}

/// "Include saved versions (3, 12 MB)".
pub fn versions_words(count: usize, bytes: u64) -> String {
    format!("Include saved versions ({count}, {})", size_text(bytes))
}

fn row(ui: &mut egui::Ui, theme: &Theme, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [64.0, 18.0],
            egui::Label::new(egui::RichText::new(name).color(theme.muted)),
        );
        ui.label(egui::RichText::new(value).color(theme.text));
    });
}

/// The bar, how much is copied and the time left.
fn progress(ui: &mut egui::Ui, theme: &Theme, copying: &Copying) {
    let done = copying.done.load(Ordering::Relaxed).min(copying.total);
    let share = if copying.total == 0 {
        1.0
    } else {
        done as f32 / copying.total as f32
    };
    ui.add(egui::ProgressBar::new(share).show_percentage());
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(progress_words(done, copying.total, copying.started.elapsed()))
            .color(theme.text_secondary),
    );
    ui.label(egui::RichText::new(format!("Into {}", copying.folder.display())).color(theme.muted));
}

/// "1.2 GB of 4.0 GB copied · about 2 min left".
pub fn progress_words(done: u64, total: u64, elapsed: Duration) -> String {
    let copied = format!("{} of {} copied", size_text(done), size_text(total));
    if done == 0 || elapsed < Duration::from_millis(500) {
        return copied;
    }
    let pace = done as f32 / elapsed.as_secs_f32();
    let left = total.saturating_sub(done) as f32 / pace;
    format!("{copied} · {}", ui_kit::time_text::time_left(left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_versions_box_says_how_many_and_how_much() {
        assert_eq!(versions_words(3, 12_000_000), "Include saved versions (3, 12 MB)");
    }

    #[test]
    fn progress_says_how_much_and_how_long() {
        assert_eq!(
            progress_words(0, 4_000_000_000, Duration::from_secs(1)),
            "0 KB of 4.0 GB copied"
        );
        assert_eq!(
            progress_words(1_000_000_000, 4_000_000_000, Duration::from_secs(40)),
            "1.0 GB of 4.0 GB copied · about 2 min left"
        );
    }
}
