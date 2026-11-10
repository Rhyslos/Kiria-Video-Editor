//! The editor window.

use crate::run_mode::RunMode;
use crate::settings::Settings;
use diag::AppError;
use jobs::{BackgroundPolicy, JobHandle, JobSystem, Limit, PoolKind, Priority, UsageMonitor};
use project::{Command, Document, LabelColor, LabelId, Recovery, Time, TimelineLabel};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ui_kit::{Action, ErrorCenter, ErrorResponse, Page, ShortcutTable, Theme, TopBar, layout};

/// How long without input before the app counts as idle (autosave runs, idle-only work starts).
const IDLE_AFTER: Duration = Duration::from_secs(5);

/// The outcome of an automated run, read by `main` after the window closes.
#[derive(Debug, Default)]
pub struct RunReport {
    pub failed: bool,
    pub message: String,
}

pub struct Editor {
    settings: Settings,
    data_dir: PathBuf,
    theme: Theme,
    page: Page,
    shortcuts: ShortcutTable,
    errors: ErrorCenter,
    jobs: JobSystem,
    usage: UsageMonitor,
    document: Option<Document>,
    pending_save: Option<(u64, JobHandle<()>)>,
    show_settings: bool,
    show_diagnostics: bool,
    show_search: bool,
    search_text: String,
    status: String,
    playing: bool,
    last_input: Instant,
    mode: RunMode,
    frames: u64,
    probe: Probe,
    report: Arc<Mutex<RunReport>>,
}

/// Measurements for the automated runs.
#[derive(Default)]
struct Probe {
    job: Option<JobHandle<()>>,
    started: Option<Instant>,
    last_frame: Option<Instant>,
    slowest_ui: Duration,
    slowest_gap: Duration,
    frames: u64,
    screenshot_saved: Arc<AtomicBool>,
    screenshot_requested: bool,
}

impl Editor {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        data_dir: PathBuf,
        project: Option<ProjectToOpen>,
        mode: RunMode,
        report: Arc<Mutex<RunReport>>,
        startup_problems: Vec<AppError>,
    ) -> Editor {
        ui_kit::setup(&cc.egui_ctx, settings.theme);
        let jobs = JobSystem::new(settings.resources.resolve(&jobs::Machine::detect()));
        let mut errors = ErrorCenter::default();
        for problem in &startup_problems {
            errors.push(problem);
        }
        let document = project.and_then(|p| match open_or_create(&p.path, p.recovery) {
            Ok(doc) => {
                if doc.is_read_only() {
                    errors.push(
                        &AppError::new("Opened read-only")
                            .why("This project was saved by a newer version of the app. You can look at it, but changes can't be saved.")
                            .fix("Update the app to edit it."),
                    );
                }
                Some(doc)
            }
            Err(err) => {
                errors.push(&err);
                None
            }
        });
        Editor {
            theme: settings.theme.theme(),
            settings,
            data_dir,
            page: Page::default(),
            shortcuts: ShortcutTable::default(),
            errors,
            jobs,
            usage: UsageMonitor::start(Duration::from_secs(1)),
            document,
            pending_save: None,
            show_settings: false,
            show_diagnostics: false,
            show_search: false,
            search_text: String::new(),
            status: String::new(),
            playing: false,
            last_input: Instant::now(),
            mode,
            frames: 0,
            probe: Probe::default(),
            report,
        }
    }

    fn run_action(&mut self, action: Action) {
        let result = match action {
            Action::Undo => self.with_document(|d| d.undo().map(|_| ())),
            Action::Redo => self.with_document(|d| d.redo().map(|_| ())),
            Action::Save => self.with_document(Document::save),
            Action::AddLabel => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                let at = d.project().active().map_or(Time::ZERO, |s| s.edit_needle);
                d.apply(Command::AddLabel {
                    sequence,
                    label: TimelineLabel {
                        id: LabelId(0),
                        name: "Label".into(),
                        at,
                        color: LabelColor::Blue,
                    },
                })
            }),
            Action::Split => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                let at = d.project().active().map_or(Time::ZERO, |s| s.edit_needle);
                d.apply(Command::Split {
                    sequence,
                    at,
                    clip: None,
                })
            }),
            Action::Delete | Action::RippleDelete => {
                self.status = "Select a clip first. Clip selection arrives with the timeline.".into();
                Ok(())
            }
            Action::PlayPause => {
                self.playing = !self.playing;
                self.jobs.set_playing(self.playing);
                self.status = if self.playing { "Playing" } else { "Paused" }.into();
                Ok(())
            }
            Action::Search => {
                self.show_search = true;
                Ok(())
            }
            Action::OpenSettings => {
                self.show_settings = true;
                Ok(())
            }
        };
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }

    fn with_document(&mut self, f: impl FnOnce(&mut Document) -> diag::Result<()>) -> diag::Result<()> {
        match self.document.as_mut() {
            Some(document) => f(document),
            None => Err(AppError::new("No project is open")
                .why("This needs a project.")
                .fix("Start the editor with a project file. The launcher arrives in Phase 2.")),
        }
    }

    /// Hands due saves to a worker and collects finished ones.
    fn autosave(&mut self, now: Instant, idle: bool) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        if let Some((seq, handle)) = &self.pending_save
            && let Some(result) = handle.try_result()
        {
            if let Err(err) = document.finish_save(*seq, result) {
                self.errors.push(&err);
            }
            self.pending_save = None;
        }
        if self.pending_save.is_none()
            && let Some(job) = document.tick(now, idle)
        {
            let seq = job.journal_seq();
            let handle = self
                .jobs
                .submit(PoolKind::Export, Priority::Background, "autosave", move |_| {
                    job.run()
                });
            self.pending_save = Some((seq, handle));
        }
    }

    fn usage_text(&self) -> String {
        let u = self.usage.latest();
        let gpu = u.gpu_percent.map_or("–".to_owned(), |g| format!("{g:.0}%"));
        format!(
            "CPU {:.0}%  GPU {gpu}  RAM {:.1} GB",
            u.cpu_percent,
            u.app_ram_mb as f32 / 1024.0
        )
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let name = self
            .document
            .as_ref()
            .map_or_else(|| "No project open".to_owned(), |d| d.project().name.clone());
        let subtitle = self.document.as_ref().map_or_else(String::new, |d| {
            let saved = if d.has_unsaved_changes() {
                "saving soon"
            } else {
                "saved"
            };
            d.project().active().map_or_else(String::new, |s| {
                format!(
                    "{} · {}p {} · {saved}",
                    s.name,
                    s.settings.height,
                    s.settings.frame_rate.nominal_fps()
                )
            })
        });
        let usage = self.usage_text();
        let clicks = TopBar {
            project_name: &name,
            subtitle: &subtitle,
            page: self.page,
            usage: &usage,
        }
        .show(ui, &self.theme);
        if let Some(page) = clicks.page {
            self.page = page;
        }
        self.show_search |= clicks.search;
        self.show_settings |= clicks.settings || clicks.usage;
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        if let Some(response) = self.errors.show_inline(ui, &self.theme) {
            match response {
                ErrorResponse::OpenDiagnostics => self.show_diagnostics = true,
                ErrorResponse::Fix(action) => self.status = format!("No handler for {} yet.", action.command),
            }
            ui.add_space(8.0);
        }
        let (title, coming) = match self.page {
            Page::Library => ("Library", "Media, labels and custom lists arrive in Phase 5."),
            Page::Edit => ("Edit", "The viewer and timeline arrive in Phase 4."),
            Page::Color => ("Color", "Grading arrives in Phase 7."),
            Page::Audio => ("Audio", "Mixing arrives in Phase 8."),
            Page::Export => ("Export", "Exporting arrives in Phase 6."),
        };
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .family(ui_kit::fonts::semibold())
                    .size(22.0),
            );
            ui.label(egui::RichText::new(coming).color(self.theme.muted));
        });
        ui.add_space(24.0);
        if let Some(document) = &self.document {
            let p = document.project();
            if let Some(seq) = p.active() {
                ui.label(format!(
                    "{} timeline labels · edit needle at {}",
                    seq.labels.len(),
                    seq.edit_needle.timecode(seq.settings.frame_rate)
                ));
            }
            ui.label(
                egui::RichText::new("M adds a label, Ctrl+Z undoes, Ctrl+Shift+Z redoes, Ctrl+S saves.")
                    .color(self.theme.muted),
            );
        }
        if !self.status.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(&self.status).color(self.theme.text_secondary));
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let before = self.settings.clone();
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.heading("Look");
                egui::ComboBox::from_label("Theme")
                    .selected_text(self.settings.theme.label())
                    .show_ui(ui, |ui| {
                        for theme in ui_kit::ThemeName::ALL {
                            ui.selectable_value(&mut self.settings.theme, theme, theme.label());
                        }
                    });
                ui.add_space(8.0);
                ui.heading("Editing");
                ui.checkbox(&mut self.settings.edit_at_playhead, "Edit at the playhead")
                    .on_hover_text("Merge the red edit needle into the playhead.");
                ui.add_space(8.0);
                ui.heading("Resources");
                let machine = jobs::Machine::detect();
                limit_row(
                    ui,
                    "Worker threads",
                    &mut self.settings.resources.worker_threads,
                    1,
                    machine.cpu_threads.max(2) - 1,
                );
                limit_row(
                    ui,
                    "RAM cache (MB)",
                    &mut self.settings.resources.ram_cache_mb,
                    256,
                    machine.total_ram_mb * 3 / 4,
                );
                limit_row(
                    ui,
                    "Disk cache (GB)",
                    &mut self.settings.resources.disk_cache_gb,
                    1,
                    500,
                );
                ui.label("Background work");
                for (policy, label) in [
                    (BackgroundPolicy::PauseWhilePlaying, "Pause while playing"),
                    (BackgroundPolicy::Always, "Always run"),
                    (BackgroundPolicy::OnlyWhenIdle, "Only when idle"),
                ] {
                    ui.radio_value(&mut self.settings.resources.background, policy, label);
                }
                ui.label(
                    egui::RichText::new("Resource changes apply the next time the editor starts.")
                        .color(self.theme.muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Diagnostics…").clicked() {
                        self.show_diagnostics = true;
                    }
                    if ui.button("Copy log folder path").clicked() {
                        ctx.copy_text(self.data_dir.join("logs").display().to_string());
                    }
                });
            });
        self.show_settings = open;
        if self.settings != before {
            if self.settings.theme != before.theme {
                self.theme = self.settings.theme.theme();
                self.theme.apply(ctx);
            }
            if let Err(err) = self.settings.save(&self.data_dir) {
                self.errors.push(&err);
            }
        }
    }

    fn search_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_search;
        let mut chosen = None;
        egui::Window::new("Search")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let field = ui.text_edit_singleline(&mut self.search_text);
                field.request_focus();
                ui.label(
                    egui::RichText::new("Commands for now; clips, labels and settings join in Phase 4.")
                        .color(self.theme.muted),
                );
                let needle = self.search_text.to_lowercase();
                let mut seen = Vec::new();
                for (action, shortcut) in self.shortcuts.bindings() {
                    if seen.contains(action) || !action.label().to_lowercase().contains(&needle) {
                        continue;
                    }
                    seen.push(*action);
                    let text = format!("{}    {}", action.label(), shortcut.label());
                    if ui.selectable_label(false, text).clicked() {
                        chosen = Some(*action);
                    }
                }
            });
        self.show_search = open && chosen.is_none();
        if let Some(action) = chosen {
            self.run_action(action);
        }
    }

    /// Automated runs: smoke test, screenshot, responsiveness test.
    fn automation(&mut self, ctx: &egui::Context, ui_time: Duration) {
        let now = Instant::now();
        match &self.mode {
            RunMode::Normal => {}
            RunMode::SmokeTest => {
                ctx.request_repaint();
                if self.frames >= 30 {
                    self.finish(
                        ctx,
                        false,
                        format!("Smoke test passed: {} frames drawn.", self.frames),
                    );
                }
            }
            RunMode::Screenshot(path) => {
                ctx.request_repaint();
                if self.frames >= 10 && !self.probe.screenshot_requested {
                    self.probe.screenshot_requested = true;
                    let (path, saved, report) = (
                        path.clone(),
                        Arc::clone(&self.probe.screenshot_saved),
                        Arc::clone(&self.report),
                    );
                    ctx.request_screenshot(move |image| {
                        if let Err(err) = save_png(&image, &path) {
                            let mut report = report.lock().unwrap_or_else(|p| p.into_inner());
                            report.failed = true;
                            report.message = err;
                        }
                        saved.store(true, Ordering::Relaxed);
                    });
                }
                if self.probe.screenshot_saved.load(Ordering::Relaxed) {
                    let failed = self.report.lock().map(|r| r.failed).unwrap_or(true);
                    self.finish(ctx, failed, format!("Screenshot saved to {}", path.display()));
                }
            }
            RunMode::ResponsivenessTest => {
                ctx.request_repaint();
                let probe = &mut self.probe;
                if probe.job.is_none() {
                    probe.started = Some(now);
                    probe.job = Some(self.jobs.submit(
                        PoolKind::Analysis,
                        Priority::Interactive,
                        "8-second test job",
                        |_| {
                            std::thread::sleep(Duration::from_secs(8));
                            Ok(())
                        },
                    ));
                }
                if let Some(last) = probe.last_frame {
                    probe.slowest_gap = probe.slowest_gap.max(now - last);
                }
                probe.last_frame = Some(now);
                probe.slowest_ui = probe.slowest_ui.max(ui_time);
                probe.frames += 1;
                let done = probe.job.as_ref().and_then(JobHandle::try_result).is_some();
                if done {
                    let elapsed = probe.started.map_or(Duration::ZERO, |s| now - s);
                    let fps = probe.frames as f64 / elapsed.as_secs_f64().max(0.001);
                    let failed = probe.slowest_ui > Duration::from_millis(16);
                    let message = format!(
                        "Responsiveness test {}: {} frames in {:.1} s ({fps:.0} per second), slowest interface work {:.1} ms, longest gap between frames {:.1} ms.",
                        if failed { "FAILED" } else { "passed" },
                        probe.frames,
                        elapsed.as_secs_f64(),
                        probe.slowest_ui.as_secs_f64() * 1000.0,
                        probe.slowest_gap.as_secs_f64() * 1000.0,
                    );
                    self.finish(ctx, failed, message);
                }
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context, failed: bool, message: String) {
        let mut report = self.report.lock().unwrap_or_else(|p| p.into_inner());
        report.failed |= failed;
        if report.message.is_empty() {
            report.message = message;
        }
        drop(report);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_input = now;
        }
        let idle = now.duration_since(self.last_input) > IDLE_AFTER;
        self.jobs.set_idle(idle);
        for action in self.shortcuts.pressed(&ctx) {
            self.run_action(action);
        }
        self.autosave(now, idle);

        egui::Panel::top("top-bar")
            .exact_size(layout::TOP_BAR_HEIGHT)
            .frame(egui::Frame::new().fill(self.theme.panel))
            .show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| self.page(ui));

        self.settings_window(&ctx);
        self.search_window(&ctx);
        let mut diagnostics = self.show_diagnostics;
        self.errors
            .diagnostics_window(&ctx, &mut diagnostics, &self.theme);
        self.show_diagnostics = diagnostics;

        self.frames += 1;
        if self.frames == 1 {
            // The launcher waits for this to know the version works (update safety).
            if let Err(err) = diag::session::report_started(&self.data_dir, brand::VERSION) {
                tracing::warn!("{}", err.report());
            }
        }
        self.automation(&ctx, started.elapsed());
        // Keep the resource readout and autosave ticking without input.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn on_exit(&mut self) {
        if let Some((seq, handle)) = self.pending_save.take()
            && let Some(document) = self.document.as_mut()
        {
            let result = handle.wait();
            let _ = document.finish_save(seq, result);
        }
        if let Some(document) = self.document.as_mut()
            && document.has_unsaved_changes()
            && let Err(err) = document.save()
        {
            tracing::error!("saving on exit failed: {}", err.report());
        }
    }
}

fn limit_row<T>(ui: &mut egui::Ui, label: &str, limit: &mut Limit<T>, min: T, max: T)
where
    T: egui::emath::Numeric + Copy,
{
    ui.horizontal(|ui| {
        let mut auto = matches!(limit, Limit::Auto);
        ui.label(label);
        if ui.checkbox(&mut auto, "Auto").changed() {
            *limit = if auto { Limit::Auto } else { Limit::Manual(min) };
        }
        if let Limit::Manual(value) = limit {
            ui.add(egui::Slider::new(value, min..=max));
        }
    });
}

/// A project named on the command line, and what to do with unsaved work from a crash.
#[derive(Debug, Clone)]
pub struct ProjectToOpen {
    pub path: PathBuf,
    /// The launcher asks; the editor on its own always recovers.
    pub recovery: Recovery,
}

/// Opens a project, or creates it if it doesn't exist.
fn open_or_create(path: &Path, recovery: Recovery) -> diag::Result<Document> {
    if path.exists() {
        Document::open(path, recovery)
    } else {
        let name = path
            .file_stem()
            .map_or("Untitled".into(), |s| s.to_string_lossy().into_owned());
        Document::create(
            path,
            project::Project::new(name, project::SequenceSettings::GAME_CLIP),
        )
    }
}

fn save_png(image: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    image::save_buffer(path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("The screenshot could not be saved: {e}"))
}
