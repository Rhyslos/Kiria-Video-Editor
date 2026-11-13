//! The Export page, as in the mockup: presets on the left, the settings in
//! the middle with the checks under them, a summary in plain words on the
//! right with Export and Add to queue, and the queue along the bottom.
//!
//! The work is the engine's (`engine::export`), on workers, one export at a
//! time; editing goes on meanwhile and the queue keeps running when another
//! page is shown. What is exported is the timeline as it was when Export
//! (or Add to queue) was pressed. Nothing here touches the disk or the
//! mixer on the interface thread: the checks' facts come from workers
//! (`checks`), own presets are saved on a thread.
//!
//! Parts: `choice` (what the settings say), `presets` (built-in and own),
//! `checks`, `queue`, `words` (how things are said, and the time an export
//! should take on this machine).

mod checks;
mod choice;
mod presets;
mod queue;
mod words;

use checks::{DiskWatch, Fix, Level, LoudnessWatch};
use choice::{Choice, FrameSize, LOWER_RATES};
use diag::AppError;
use engine::export::{AudioCodec, Container, Devices, Encoders, Quality, VideoCodec};
use presets::{OwnPresets, Preset};
use project::{MediaId, Project, Rate, SequenceId, Time};
pub use queue::Unfinished;
use queue::{Job, Queue, State};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use ui_kit::{Theme, layout};
pub use words::Pace;

/// The queue's height along the bottom of the page.
const QUEUE_HEIGHT: f32 = 172.0;
/// The label column of the Video and Audio sections.
const LABEL_WIDTH: f32 = 92.0;

/// What the page asks of the window.
#[derive(Debug, Default)]
pub struct Asked {
    pub problem: Option<AppError>,
    /// Look for the missing files (the window's "Find them…").
    pub find_missing: bool,
}

#[derive(Debug)]
pub struct ExportPage {
    choice: Choice,
    /// The file's name without its ending, as typed.
    name: String,
    /// The folder to save to, as typed or chosen.
    folder: String,
    /// The project the name and folder were set up for.
    for_project: Option<PathBuf>,
    /// Export the region only (when there is one).
    region_only: bool,
    /// The loudness aimed for, from Settings › Audio (none: no target).
    target: Option<f32>,
    /// What this machine can encode, once a worker has found out.
    encoders: Option<Encoders>,
    finding: Option<Receiver<Encoders>>,
    /// The folder picker, while it is open.
    folder_dialog: Option<Receiver<Option<PathBuf>>>,
    /// A folder being made ("Make it").
    making: Option<Receiver<diag::Result<()>>>,
    own: OwnPresets,
    /// The presets file couldn't be read: it is never written over.
    own_read_only: bool,
    /// The name being typed for a new preset.
    saving_as: Option<String>,
    /// Writes the own presets file on a thread of its own, one write at a
    /// time, the newest set last.
    own_keeper: project::disk::Keeper,
    queue: Queue,
    loudness: LoudnessWatch,
    disk: DiskWatch,
    pace: Pace,
    /// The pace changed: the window saves it with the settings.
    learned: bool,
    /// How many of the files the timeline plays can't be found, kept until
    /// the timeline or the missing files change.
    media_counts: Option<(Project, SequenceId, usize, (usize, usize))>,
    /// A line said under the presets (a preset saved, removed).
    said: Option<String>,
    /// `--export-queue`: fill the queue once, for screenshots.
    demo: Demo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Demo {
    Off,
    Wanted,
    /// Filled: the second job is paused once it is under way.
    Filled,
}

impl ExportPage {
    /// The page, with the person's own presets read from `data_dir` (an
    /// unreadable file is explained once and left alone) and the export
    /// pace measured before.
    pub fn new(data_dir: &Path, pace: Pace, errors: &mut ui_kit::ErrorCenter) -> ExportPage {
        let (own, own_read_only) = match OwnPresets::load(data_dir) {
            Ok(own) => (own, false),
            Err(err) => {
                errors.push(&err);
                (OwnPresets::default(), true)
            }
        };
        ExportPage {
            choice: Choice::default(),
            name: String::new(),
            folder: String::new(),
            for_project: None,
            region_only: false,
            target: Some(-14.0),
            encoders: None,
            finding: None,
            folder_dialog: None,
            making: None,
            own,
            own_read_only,
            saving_as: None,
            own_keeper: project::disk::Keeper::new(presets::path(data_dir), "Your export presets"),
            queue: Queue::default(),
            loudness: LoudnessWatch::default(),
            disk: DiskWatch::default(),
            pace,
            learned: false,
            media_counts: None,
            said: None,
            demo: Demo::Off,
        }
    }

    /// The loudness aimed for, as Settings has it now (`None`: no target).
    pub fn aim_at(&mut self, target: Option<f32>) {
        self.target = target;
    }

    /// Fills the queue once the page is drawn (`--export-queue`, for screenshots).
    pub fn demo_queue(&mut self) {
        self.demo = Demo::Wanted;
    }

    /// A newly measured export pace, to be saved with the settings.
    pub fn take_pace(&mut self) -> Option<Pace> {
        std::mem::take(&mut self.learned).then_some(self.pace)
    }

    /// What closing the app now would stop or drop, if anything.
    pub fn unfinished(&self) -> Option<Unfinished> {
        self.queue.unfinished()
    }

    /// Stops the export under way, if any: for closing the app.
    pub fn stop(&mut self) {
        self.queue.stop();
    }

    /// Whether what the page shows is complete (for screenshots): the
    /// encoders, the loudness, the disk, and a filled demo queue.
    pub fn settled(&self) -> bool {
        let demo = match self.demo {
            Demo::Off => true,
            Demo::Wanted => false,
            Demo::Filled => {
                let jobs = self.queue.jobs();
                jobs.first().is_some_and(|j| j.state.is_over())
                    && jobs
                        .get(1)
                        .is_some_and(|j| j.progress().is_some_and(|p| p.paused) || j.state.is_over())
            }
        };
        self.encoders.is_some() && self.loudness.settled() && self.disk.settled() && demo
    }

    /// Takes in what the workers did since the last frame: every frame,
    /// whichever page is shown. Returns errors to show.
    pub fn poll(&mut self) -> Vec<AppError> {
        let mut errors = Vec::new();
        if let Some(dialog) = &self.folder_dialog
            && let Ok(picked) = dialog.try_recv()
        {
            self.folder_dialog = None;
            if let Some(folder) = picked {
                self.folder = folder.display().to_string();
            }
        }
        if let Some(making) = &self.making
            && let Ok(made) = making.try_recv()
        {
            self.making = None;
            self.disk.again();
            if let Err(err) = made {
                errors.push(err);
            }
        }
        if let Some(err) = self.own_keeper.take_failure() {
            errors.push(err);
        }
        if let Some(finding) = &self.finding
            && let Ok(found) = finding.try_recv()
        {
            self.finding = None;
            self.encoders = Some(found);
            self.choice.settle(self.encoders.as_ref());
        }
        let polled = self.queue.poll();
        for done in &polled.finished {
            self.pace.learn(done);
            self.learned = true;
        }
        errors.extend(polled.failed);
        errors
    }

    /// Draws the page. `missing` are the project's files that can't be
    /// found; `devices` gives what a worker needs to put pictures together
    /// (`None`: this window can't).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        project_file: &Path,
        missing: &[MediaId],
        devices: impl FnOnce() -> Option<Devices>,
    ) -> Asked {
        let mut asked = Asked::default();
        let Some(seq) = project.active() else { return asked };
        let ctx = ui.ctx().clone();
        if self.encoders.is_none() && self.finding.is_none() {
            let ctx = ctx.clone();
            self.finding = Some(Encoders::detect_in_background(move || ctx.request_repaint()));
        }
        {
            let ctx = ctx.clone();
            self.queue.notify_with(Arc::new(move || ctx.request_repaint()));
        }
        if self.for_project.as_deref() != Some(project_file) {
            self.for_project = Some(project_file.to_path_buf());
            self.name = checks::clean_name(&project.name);
            if self.name.is_empty() {
                self.name = "Video".to_owned();
            }
            self.folder = project_file
                .parent()
                .unwrap_or(Path::new("."))
                .display()
                .to_string();
        }
        let devices = devices();
        let plan = self.plan(project, seq, missing, devices.is_some(), &ctx);
        if self.demo == Demo::Wanted && self.encoders.is_some() {
            self.fill_demo(project, &plan, devices.clone());
        }
        if self.demo == Demo::Filled
            && let Some(job) = self.queue.jobs().get(1)
            && job.progress().is_some_and(|p| p.share() >= 0.3 && !p.paused)
        {
            let id = job.id;
            self.queue.pause(id, true);
        }

        let page = ui.available_rect_before_wrap();
        let gap = layout::PANEL_GAP;
        let top_bottom = page.bottom() - QUEUE_HEIGHT - gap;
        let left = egui::Rect::from_min_max(
            page.min,
            egui::pos2(page.left() + layout::LEFT_PANEL_WIDTH, top_bottom),
        );
        let right = egui::Rect::from_min_max(
            egui::pos2(page.right() - layout::RIGHT_PANEL_WIDTH, page.top()),
            egui::pos2(page.right(), top_bottom),
        );
        let middle = egui::Rect::from_min_max(
            egui::pos2(left.right() + gap, page.top()),
            egui::pos2(right.left() - gap, top_bottom),
        );
        let bottom =
            egui::Rect::from_min_max(egui::pos2(page.left(), page.bottom() - QUEUE_HEIGHT), page.max);

        layout::panel(ui, theme, left, |ui| self.presets_ui(ui, theme, &mut asked));
        let mut column = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(middle)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .id_salt("export settings")
            .auto_shrink([false, false])
            .show(&mut column, |ui| {
                ui.add_space(4.0);
                self.file_row(ui, theme, seq, &plan);
                ui.add_space(14.0);
                ui.columns(2, |columns| {
                    self.video_section(&mut columns[0], theme, seq, &plan);
                    self.audio_section(&mut columns[1], theme, seq);
                });
                ui.add_space(14.0);
                self.checks_ui(ui, theme, &plan, &mut asked);
            });
        layout::panel(ui, theme, right, |ui| {
            self.summary_ui(ui, theme, project, &plan, devices, &mut asked);
        });
        layout::panel(ui, theme, bottom, |ui| self.queue_ui(ui, theme, &mut asked));
        ui.allocate_rect(page, egui::Sense::hover());

        if plan.checks.iter().any(|c| c.level == Level::Waiting) || self.queue.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        asked
    }
}

/// What the page works out once a frame from the settings and the timeline.
struct Plan {
    sequence: SequenceId,
    rate: Rate,
    /// The file's frame rate.
    out_rate: Rate,
    timeline_size: (u32, u32),
    /// The file's picture size (none for sound alone).
    frame_size: Option<(u32, u32)>,
    region: Option<(Time, Time)>,
    range: Option<(Time, Time)>,
    folder: PathBuf,
    path: PathBuf,
    settings: engine::export::ExportSettings,
    bytes: u64,
    beside_bytes: u64,
    estimate: words::Estimate,
    checks: Vec<checks::Check>,
    /// A check stops the export.
    blocked: bool,
    disk: Option<checks::Disk>,
}

impl ExportPage {
    fn plan(
        &mut self,
        project: &Project,
        seq: &project::Sequence,
        missing: &[MediaId],
        can_picture: bool,
        ctx: &egui::Context,
    ) -> Plan {
        let rate = seq.settings.frame_rate;
        let end = seq.length();
        let region = seq
            .region
            .map(|r| (r.start, r.end.min(end)))
            .filter(|(from, to)| to > from);
        if region.is_none() {
            self.region_only = false;
        }
        let range = if self.region_only { region } else { None };
        let stretch = range.map_or(end, |(from, to)| to - from);
        let length = Duration::from_secs_f64(stretch.as_secs_f64().max(0.0));
        let timeline_size = (seq.settings.width, seq.settings.height);
        let folder = PathBuf::from(self.folder.trim());
        let file_name = format!("{}.{}", self.name.trim(), self.choice.container.extension());
        let path = folder.join(&file_name);
        let settings = self
            .choice
            .settings(path.clone(), range, timeline_size, self.target);
        let frame_size = settings.size;
        let out_rate = self.choice.rate.unwrap_or(rate);
        let sample_rate = seq.settings.sample_rate;
        let bytes = settings.estimated_bytes(frame_size.unwrap_or((2, 2)), out_rate, sample_rate, length);
        let beside_bytes = settings.estimated_beside_bytes(sample_rate, length);
        let frames = (length.as_secs_f64() * out_rate.as_f64()).round() as u64;
        let on_card = !self.choice.software
            && self
                .encoders
                .as_ref()
                .is_some_and(|e| e.video(self.choice.video).graphics_card.is_some());
        let estimate = self.pace.estimate(words::Work {
            pictures: frame_size.map(|size| (size, frames)),
            on_card,
            length,
            normalise: settings.normalise.is_some(),
        });

        // What the checks need, from the workers and the timeline.
        let notify = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let loudness = self.loudness.look(project, seq.id, range, notify.clone());
        let disk = self.disk.look(&folder, &path, notify);
        let media = self.media_counts(project, seq.id, missing);
        let video = settings.video;
        let encodable = self
            .encoders
            .as_ref()
            .map(|e| e.can(settings.container, video, settings.audio, settings.software));
        let other_codec = self.encoders.as_ref().and_then(|e| {
            VideoCodec::ALL.into_iter().find(|&c| {
                Some(c) != video && e.can(settings.container, Some(c), settings.audio, settings.software)
            })
        });
        let facts = checks::Facts {
            media,
            empty: stretch <= Time::ZERO,
            no_pictures_here: video.is_some() && !can_picture,
            encodable,
            video,
            other_codec,
            loudness,
            normalise: self.choice.normalise,
            target: self.target,
            disk: disk.clone(),
            name: file_name,
            bad_name: checks::clean_name(&self.name) != self.name.trim(),
            needed: bytes + beside_bytes,
            sizes: frame_size.map(|f| (timeline_size, f)),
        };
        let checks = checks::list(&facts);
        let blocked = checks.iter().any(|c| matches!(c.level, Level::Bad))
            || facts.encodable != Some(true)
            || facts.disk.is_none()
            || self.name.trim().is_empty();
        Plan {
            sequence: seq.id,
            rate,
            out_rate,
            timeline_size,
            frame_size,
            region,
            range,
            folder,
            path,
            settings,
            bytes,
            beside_bytes,
            estimate,
            checks,
            blocked,
            disk,
        }
    }

    fn media_counts(
        &mut self,
        project: &Project,
        sequence: SequenceId,
        missing: &[MediaId],
    ) -> (usize, usize) {
        if let Some((p, s, n, counts)) = &self.media_counts
            && *s == sequence
            && *n == missing.len()
            && p.sequences.ptr_eq(&project.sequences)
        {
            return *counts;
        }
        let counts = checks::media_counts(project, sequence, missing);
        self.media_counts = Some((project.clone(), sequence, missing.len(), counts));
        counts
    }

    /// The presets on the left: built-in, then the person's own, then
    /// saving the settings as a preset.
    fn presets_ui(&mut self, ui: &mut egui::Ui, theme: &Theme, asked: &mut Asked) {
        heading(ui, theme, "PRESETS");
        let built_in = presets::built_in(self.encoders.as_ref());
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("export presets")
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 44.0)
            .show(ui, |ui| {
                for preset in &built_in {
                    if preset_button(ui, theme, preset, &self.choice, self.target).clicked() {
                        picked = Some(preset.clone());
                    }
                }
                if !self.own.presets.is_empty() {
                    ui.add_space(10.0);
                    heading(ui, theme, "YOUR PRESETS");
                }
                let mut remove = None;
                for preset in &self.own.presets {
                    let response = preset_button(ui, theme, preset, &self.choice, self.target);
                    if response.clicked() {
                        picked = Some(preset.clone());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Remove this preset").clicked() {
                            remove = Some(preset.name.clone());
                            ui.close();
                        }
                    });
                }
                if let Some(name) = remove {
                    self.own.remove(&name);
                    self.said = Some(format!("Removed \"{name}\"."));
                    self.save_own(asked);
                }
            });
        if let Some(preset) = picked {
            self.choice = preset.choice;
            self.choice.settle(self.encoders.as_ref());
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            match &mut self.saving_as {
                None => {
                    if ui
                        .add_enabled(!self.own_read_only, egui::Button::new("+ Save as a preset"))
                        .on_hover_text(
                            "Keep the current settings under a name of yours, offered in every project",
                        )
                        .clicked()
                    {
                        self.saving_as = Some(String::new());
                        self.said = None;
                    }
                    if let Some(said) = &self.said {
                        ui.label(egui::RichText::new(said).color(theme.muted).small());
                    }
                }
                Some(name) => {
                    let mut done = None;
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                    let field = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("Name of the preset")
                            .desired_width(f32::INFINITY),
                    );
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = Some(true);
                    }
                    if !field.has_focus() && name.is_empty() && done.is_none() {
                        field.request_focus();
                    }
                    match done {
                        Some(true) => {
                            let name = name.clone();
                            match self.own.add(&name, self.choice.clone()) {
                                Ok(replaced) => {
                                    self.said = Some(if replaced {
                                        format!("Replaced \"{}\".", name.trim())
                                    } else {
                                        format!("Saved \"{}\": it is offered in every project.", name.trim())
                                    });
                                    self.saving_as = None;
                                    self.save_own(asked);
                                }
                                Err(err) => asked.problem = Some(err),
                            }
                        }
                        Some(false) => self.saving_as = None,
                        None => {}
                    }
                }
            }
        });
    }

    /// Saves the own presets off this thread.
    fn save_own(&mut self, asked: &mut Asked) {
        if self.own_read_only {
            asked.problem = Some(
                AppError::new("Your export presets can't be changed")
                    .why("Their file couldn't be read when the app started, so it is left as it is.")
                    .fix("See the message about it in Diagnostics; the built-in presets still work."),
            );
            return;
        }
        let own = self.own.clone();
        self.own_keeper.keep(move || own.to_text());
    }

    /// File name, folder and range, in one row.
    fn file_row(&mut self, ui: &mut egui::Ui, theme: &Theme, seq: &project::Sequence, plan: &Plan) {
        let rate = plan.rate;
        // The three share the column: the name and the range a quarter
        // each, the folder the rest.
        let room = ui.available_width() - 2.0 * 16.0;
        let (name_width, range_width) = (room * 0.26 - 40.0, room * 0.27);
        // What else the row holds: the ending, Browse…, the spacing.
        let folder_width = room - room * 0.26 - range_width - 150.0;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                small_label(ui, theme, "File name");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(name_width));
                    ui.label(
                        egui::RichText::new(format!(".{}", self.choice.container.extension()))
                            .color(theme.muted),
                    );
                });
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                small_label(ui, theme, "Save to");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.folder).desired_width(folder_width));
                    if ui.button("Browse…").clicked() && self.folder_dialog.is_none() {
                        self.folder_dialog = Some(ask_folder(ui.ctx(), &plan.folder));
                    }
                });
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                small_label(ui, theme, "Range");
                let whole = format!(
                    "Whole timeline ({})",
                    ui_kit::time_text::duration(seq.length().as_secs_f64() as u64)
                );
                let region_text = match plan.region {
                    Some((from, to)) => format!(
                        "Region, in to out ({} – {})",
                        from.timecode(rate),
                        to.timecode(rate)
                    ),
                    None => "Region, in to out (none marked)".to_owned(),
                };
                egui::ComboBox::from_id_salt("export range")
                    .width(range_width)
                    .selected_text(if self.region_only { &region_text } else { &whole })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.region_only, false, &whole);
                        ui.add_enabled_ui(plan.region.is_some(), |ui| {
                            ui.selectable_value(&mut self.region_only, true, &region_text)
                                .on_disabled_hover_text(
                                    "Mark in and out (or Shift-drag in the timeline's ruler) first",
                                );
                        });
                    });
            });
        });
    }

    fn video_section(&mut self, ui: &mut egui::Ui, theme: &Theme, seq: &project::Sequence, plan: &Plan) {
        let encoders = self.encoders.clone();
        section(ui, theme, "Video", |ui| {
            row(ui, theme, "Container", |ui| {
                egui::ComboBox::from_id_salt("export container")
                    .width(200.0)
                    .selected_text(container_words(self.choice.container))
                    .show_ui(ui, |ui| {
                        for container in Container::ALL {
                            if ui
                                .selectable_value(
                                    &mut self.choice.container,
                                    container,
                                    container_words(container),
                                )
                                .clicked()
                            {
                                self.choice.settle(encoders.as_ref());
                            }
                        }
                    });
            });
            if !self.choice.container.has_video() {
                ui.label(egui::RichText::new("Sound only: no picture is written.").color(theme.muted));
                return;
            }
            row(ui, theme, "Codec", |ui| {
                egui::ComboBox::from_id_salt("export video codec")
                    .width(200.0)
                    .selected_text(codec_words(self.choice.video))
                    .show_ui(ui, |ui| {
                        for &codec in self.choice.container.video_codecs() {
                            let usable = encoders.as_ref().is_none_or(|e| e.video(codec).any());
                            ui.add_enabled_ui(usable, |ui| {
                                ui.selectable_value(&mut self.choice.video, codec, codec_words(codec))
                                    .on_disabled_hover_text("This machine has no encoder for it");
                            });
                        }
                    });
            });
            row(ui, theme, "Size", |ui| {
                let timeline = plan.timeline_size;
                egui::ComboBox::from_id_salt("export size")
                    .width(200.0)
                    .selected_text(self.choice.size.label(timeline))
                    .show_ui(ui, |ui| {
                        for size in std::iter::once(FrameSize::Timeline).chain(FrameSize::OFFERED) {
                            ui.selectable_value(&mut self.choice.size, size, size.label(timeline));
                        }
                        if let FrameSize::Exact(..) | FrameSize::Lines(..) = self.choice.size
                            && !FrameSize::OFFERED.contains(&self.choice.size)
                        {
                            let size = self.choice.size;
                            ui.selectable_value(&mut self.choice.size, size, size.label(timeline));
                        }
                    });
            });
            row(ui, theme, "Frame rate", |ui| {
                let own = seq.settings.frame_rate;
                let own_text = format!("{} fps (the timeline's)", words::short(own.as_f64()));
                egui::ComboBox::from_id_salt("export rate")
                    .width(200.0)
                    .selected_text(match self.choice.rate {
                        Some(r) => format!("{} fps", words::short(r.as_f64())),
                        None => own_text.clone(),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.choice.rate, None, &own_text);
                        for rate in LOWER_RATES
                            .into_iter()
                            .filter(|r| r.as_f64() < own.as_f64() - 0.01)
                        {
                            ui.selectable_value(
                                &mut self.choice.rate,
                                Some(rate),
                                format!("{} fps", words::short(rate.as_f64())),
                            );
                        }
                    });
            });
            row(ui, theme, "Quality", |ui| {
                let size = plan.frame_size.unwrap_or(plan.timeline_size);
                let words = |q: Quality| {
                    let bits = q.video_bit_rate(self.choice.video, size, plan.out_rate);
                    let megabits = bits as f64 / 1_000_000.0;
                    if megabits < 10.0 {
                        format!("{} · {megabits:.1} Mb/s", q.label())
                    } else {
                        format!("{} · {megabits:.0} Mb/s", q.label())
                    }
                };
                egui::ComboBox::from_id_salt("export quality")
                    .width(200.0)
                    .selected_text(words(self.choice.quality))
                    .show_ui(ui, |ui| {
                        for quality in Quality::ALL {
                            ui.selectable_value(&mut self.choice.quality, quality, words(quality))
                                .on_hover_text(quality_about(quality));
                        }
                    });
            });
            let probed = encoders.as_ref().map(|e| e.video(self.choice.video));
            let card = probed.and_then(|p| p.graphics_card);
            // Ticked when the card will encode (or may: not known yet).
            let mut use_card = !self.choice.software && (probed.is_none() || card.is_some());
            let tip = match (probed, card) {
                (None, _) => "Finding out what this machine can encode…".to_owned(),
                (Some(_), Some(name)) => format!(
                    "This graphics card encodes {} ({name})",
                    self.choice.video.label()
                ),
                (Some(_), None) => format!(
                    "This graphics card has no encoder for {}: the processor encodes it",
                    self.choice.video.label()
                ),
            };
            ui.add_space(2.0);
            let enabled = probed.is_none() || card.is_some();
            if ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut use_card, "Use the graphics card to encode"),
                )
                .on_hover_text(&tip)
                .on_disabled_hover_text(&tip)
                .changed()
            {
                self.choice.software = !use_card;
            }
        });
    }

    fn audio_section(&mut self, ui: &mut egui::Ui, theme: &Theme, seq: &project::Sequence) {
        let encoders = self.encoders.clone();
        let target = self.target;
        section(ui, theme, "Audio", |ui| {
            row(ui, theme, "Codec", |ui| {
                egui::ComboBox::from_id_salt("export audio codec")
                    .width(200.0)
                    .selected_text(audio_words(self.choice.audio))
                    .show_ui(ui, |ui| {
                        for &codec in self.choice.container.audio_codecs() {
                            let usable = encoders.as_ref().is_none_or(|e| e.audio(codec).is_some());
                            ui.add_enabled_ui(usable, |ui| {
                                ui.selectable_value(&mut self.choice.audio, codec, audio_words(codec))
                                    .on_disabled_hover_text("This machine has no encoder for it");
                            });
                        }
                    });
            });
            row(ui, theme, "Bit rate", |ui| {
                let text = match self.choice.quality.audio_bit_rate(self.choice.audio) {
                    Some(bits) => format!("{} kb/s (from the quality)", bits / 1000),
                    None => "Lossless: every sample as mixed".to_owned(),
                };
                ui.label(egui::RichText::new(text).color(theme.text_secondary));
            });
            row(ui, theme, "Sample rate", |ui| {
                let own = seq.settings.sample_rate;
                let rate = self.choice.audio.sample_rate_for(own);
                let text = if rate == own {
                    format!("{} kHz (the timeline's)", words::short(f64::from(rate) / 1000.0))
                } else {
                    format!(
                        "{} kHz (what {} takes)",
                        words::short(f64::from(rate) / 1000.0),
                        self.choice.audio.label()
                    )
                };
                ui.label(egui::RichText::new(text).color(theme.text_secondary));
            });
            ui.add_space(2.0);
            let label = match target {
                Some(t) => format!(
                    "Normalise loudness to {} LUFS, peaks under {:.0} dBTP",
                    words::target_text(t),
                    engine::export::TRUE_PEAK_CEILING
                ),
                None => "Normalise loudness (choose a loudness in Settings, under Audio)".to_owned(),
            };
            ui.add_enabled(target.is_some(), egui::Checkbox::new(&mut self.choice.normalise, label))
                .on_hover_text("The whole stretch is measured first, then exported turned up or down to the loudness aimed for, with a limiter keeping the peaks safe");
            let wav = self.choice.container != Container::Wav;
            ui.add_enabled(
                wav,
                egui::Checkbox::new(
                    &mut self.choice.sound_beside,
                    "Also save the sound as a separate WAV",
                ),
            )
            .on_hover_text("The same sound, 24-bit, beside the file with the same name")
            .on_disabled_hover_text("The file is a WAV already");
        });
    }

    fn checks_ui(&mut self, ui: &mut egui::Ui, theme: &Theme, plan: &Plan, asked: &mut Asked) {
        let mut fix = None;
        section(ui, theme, "Checks", |ui| {
            for check in &plan.checks {
                ui.horizontal(|ui| {
                    let colour = match check.level {
                        Level::Good => ui_kit::status::GOOD,
                        Level::Warn => ui_kit::status::WARNING,
                        Level::Bad => ui_kit::status::ERROR,
                        Level::Note | Level::Waiting => theme.muted,
                    };
                    if check.level == Level::Waiting {
                        ui.add(egui::Spinner::new().size(10.0));
                    } else {
                        ui_kit::status_dot(ui, colour);
                    }
                    ui.add_space(4.0);
                    let width = ui.available_width() - if check.fix.is_some() { 170.0 } else { 0.0 };
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 20.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(width);
                            ui.add(
                                egui::Label::new(egui::RichText::new(&check.text).color(theme.text)).wrap(),
                            );
                        },
                    );
                    if let Some((what, label)) = check.fix {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(label).clicked() {
                                fix = Some(what);
                            }
                        });
                    }
                });
            }
        });
        if let Some(what) = fix {
            self.apply_fix(ui.ctx(), what, plan, asked);
        }
    }

    fn apply_fix(&mut self, ctx: &egui::Context, fix: Fix, plan: &Plan, asked: &mut Asked) {
        match fix {
            Fix::FindMissing => asked.find_missing = true,
            Fix::Normalise => self.choice.normalise = true,
            Fix::TimelineSize => self.choice.size = FrameSize::Timeline,
            Fix::UseCodec(codec) => self.choice.video = codec,
            Fix::CleanName => self.name = checks::clean_name(&self.name),
            Fix::ChooseFolder => {
                if self.folder_dialog.is_none() {
                    self.folder_dialog = Some(ask_folder(ctx, &plan.folder));
                }
            }
            Fix::NewName => {
                let ending = self.choice.container.extension();
                let folder = plan.folder.clone();
                // A few looks at the disk, at a click.
                self.name = checks::free_name(&self.name, |n| folder.join(format!("{n}.{ending}")).exists());
            }
            Fix::MakeFolder => {
                let (tell, made) = channel();
                let folder = plan.folder.clone();
                let ctx = ctx.clone();
                let spawned = std::thread::Builder::new()
                    .name("export folder".into())
                    .spawn(move || {
                        let made = std::fs::create_dir_all(&folder).map_err(|err| {
                            AppError::new("The folder can't be made")
                                .why(format!("Making {} failed.", folder.display()))
                                .fix("Check the path, or choose another folder with Browse….")
                                .source(err)
                        });
                        let _ = tell.send(made);
                        ctx.request_repaint();
                    });
                match spawned {
                    Ok(_) => self.making = Some(made),
                    Err(err) => tracing::error!("making the folder could not start: {err}"),
                }
            }
        }
    }

    /// The summary on the right: the file in a sentence, what it plays on,
    /// the free space, then Export and Add to queue.
    fn summary_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        plan: &Plan,
        devices: Option<Devices>,
        asked: &mut Asked,
    ) {
        let name = plan
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        ui.add(
            egui::Label::new(
                egui::RichText::new(&name)
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            )
            .truncate(),
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new(self.sentence(plan)).color(theme.text_secondary));
        ui.add_space(12.0);
        egui::Grid::new("export summary")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let dt = |ui: &mut egui::Ui, text: &str| {
                    ui.label(egui::RichText::new(text).color(theme.muted));
                };
                let dd = |ui: &mut egui::Ui, text: String| {
                    ui.add(egui::Label::new(egui::RichText::new(text).color(theme.text)).wrap());
                };
                dt(ui, "Plays on");
                dd(ui, self.choice.plays_on().to_owned());
                ui.end_row();
                dt(ui, "Free space");
                dd(
                    ui,
                    match &plan.disk {
                        Some(checks::Disk { free: Some(free), .. }) => {
                            format!("{} there", words::bytes_text(*free))
                        }
                        Some(_) => "Can't be told".to_owned(),
                        None => "Looking…".to_owned(),
                    },
                );
                ui.end_row();
                dt(ui, "While exporting");
                dd(ui, "You can keep editing".to_owned());
                ui.end_row();
            });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let width = ui.available_width();
            let mut add = None;
            if ui
                .add_enabled(
                    !plan.blocked,
                    egui::Button::new("Add to queue for later").min_size(egui::vec2(width, 30.0)),
                )
                .on_hover_text("Waits in the queue until you press its Start")
                .clicked()
            {
                add = Some(false);
            }
            let export = ui.add_enabled(!plan.blocked, |ui: &mut egui::Ui| {
                ui.add(
                    egui::Button::new(
                        egui::RichText::new("Export")
                            .color(theme.accent_ink)
                            .family(ui_kit::fonts::semibold()),
                    )
                    .fill(theme.accent)
                    .min_size(egui::vec2(width, 34.0)),
                )
            });
            if export.clicked() {
                add = Some(true);
            }
            if plan.blocked {
                ui.label(egui::RichText::new("Fix what the checks mark in red first.").color(theme.muted));
            } else if self.queue.busy() {
                ui.label(
                    egui::RichText::new("Export starts once the export under way is done.")
                        .color(theme.muted),
                );
            }
            if let Some(now) = add {
                self.add_job(project, plan, devices, now, asked);
            }
        });
    }

    /// "1920 × 1080, H.264, 60 fps, AAC sound. About 240 MB, roughly 1 min
    /// 40 s on this machine."
    fn sentence(&self, plan: &Plan) -> String {
        let what = match plan.frame_size {
            Some((w, h)) => format!(
                "{w} × {h}, {}, {} fps, {} sound",
                self.choice.video.label(),
                words::short(plan.out_rate.as_f64()),
                self.choice.audio.label()
            ),
            None => format!("Sound only, {}", self.choice.format_words()),
        };
        let beside = if plan.beside_bytes > 0 {
            format!(
                ", and a WAV of {} beside it",
                words::bytes_text(plan.beside_bytes)
            )
        } else {
            String::new()
        };
        let guess = if plan.estimate.measured {
            ""
        } else {
            " (a first guess: the first export measures this machine)"
        };
        format!(
            "{what}. About {}{beside}, {} on this machine{guess}.",
            words::bytes_text(plan.bytes),
            words::about_time(plan.estimate.time),
        )
    }

    fn add_job(
        &mut self,
        project: &Project,
        plan: &Plan,
        devices: Option<Devices>,
        now: bool,
        asked: &mut Asked,
    ) {
        if let Err(err) = plan.settings.check() {
            asked.problem = Some(err);
            return;
        }
        let preset = self.preset_name().unwrap_or_else(|| "Your settings".to_owned());
        let job = Job::new(
            preset,
            plan.settings.clone(),
            self.choice.clone(),
            plan.bytes,
            (project.clone(), plan.sequence),
            devices,
        );
        self.queue.add(job, now);
    }

    /// The name of the preset the settings are, if they are one.
    fn preset_name(&self) -> Option<String> {
        presets::built_in(self.encoders.as_ref())
            .iter()
            .chain(&self.own.presets)
            .find(|p| p.choice == self.choice)
            .map(|p| p.name.clone())
    }

    fn queue_ui(&mut self, ui: &mut egui::Ui, theme: &Theme, asked: &mut Asked) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Queue").family(ui_kit::fonts::semibold()));
            ui.label(
                egui::RichText::new("Runs in the background, one at a time; you can keep editing.")
                    .color(theme.muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open export folder").clicked()
                    && let Err(err) = show_folder(Path::new(self.folder.trim()))
                {
                    asked.problem = Some(err);
                }
            });
        });
        ui.add_space(4.0);
        if self.queue.is_empty() {
            ui_kit::inspector::note(
                ui,
                theme,
                "Nothing in the queue yet. Export, or Add to queue for later, and it shows here.",
            );
            return;
        }
        let mut act = None;
        egui::ScrollArea::vertical()
            .id_salt("export queue")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for job in self.queue.jobs() {
                    if let Some(what) = job_row(ui, theme, job) {
                        act = Some((job.id, what));
                    }
                }
            });
        if let Some((id, what)) = act {
            match what {
                JobAct::Start => self.queue.start(id),
                JobAct::Pause(on) => self.queue.pause(id, on),
                JobAct::Cancel => self.queue.cancel(id),
                JobAct::Remove => {
                    self.queue.remove(id);
                }
                JobAct::Edit => {
                    if let Some(job) = self.queue.remove(id) {
                        self.choice = job.choice;
                        if let Some(stem) = job.settings.path.file_stem() {
                            self.name = stem.to_string_lossy().into_owned();
                        }
                        if let Some(folder) = job.settings.path.parent() {
                            self.folder = folder.display().to_string();
                        }
                        self.region_only = job.settings.range.is_some();
                    }
                }
                JobAct::Show(path) => {
                    if let Err(err) = show_folder(path.parent().unwrap_or(&path)) {
                        asked.problem = Some(err);
                    }
                }
            }
        }
    }

    /// `--export-queue`: one export done, one under way (paused once it is
    /// a third through), one waiting, for screenshots of the queue.
    fn fill_demo(&mut self, project: &Project, plan: &Plan, devices: Option<Devices>) {
        self.demo = Demo::Filled;
        let all = presets::built_in(self.encoders.as_ref());
        let mut add = |preset: &Preset, suffix: &str, now: bool| {
            let path = plan.folder.join(format!(
                "{}{suffix}.{}",
                self.name,
                preset.choice.container.extension()
            ));
            let settings = preset
                .choice
                .settings(path, plan.range, plan.timeline_size, self.target);
            let job = Job::new(
                preset.name.clone(),
                settings,
                preset.choice.clone(),
                plan.bytes,
                (project.clone(), plan.sequence),
                devices.clone(),
            );
            self.queue.add(job, now);
        };
        add(&all[4], " preview", true);
        add(&all[2], " vertical", true);
        add(&all[5], "", false);
    }
}

enum JobAct {
    Start,
    Pause(bool),
    Cancel,
    Remove,
    Edit,
    Show(PathBuf),
}

/// One job in the queue: its name and preset, a progress bar, where it is,
/// and what can be done with it.
fn job_row(ui: &mut egui::Ui, theme: &Theme, job: &Job) -> Option<JobAct> {
    let mut act = None;
    ui.horizontal(|ui| {
        let name_width = 340.0_f32.min(ui.available_width() * 0.3);
        ui.allocate_ui_with_layout(
            egui::vec2(name_width, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(name_width);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(job.name())
                            .family(ui_kit::fonts::semibold())
                            .color(theme.text),
                    )
                    .truncate(),
                )
                .on_hover_text(job.settings.path.display().to_string());
                ui.label(egui::RichText::new(format!("· {}", job.preset)).color(theme.muted));
            },
        );
        let (share, colour, state, tip) = match &job.state {
            State::Held => (0.0, theme.faint, "Waiting for Start".to_owned(), None),
            State::Next => (
                0.0,
                theme.faint,
                "Waiting for the export before it".to_owned(),
                None,
            ),
            State::Running(export) => {
                let progress = export.progress();
                let colour = if progress.paused {
                    ui_kit::status::WARNING
                } else {
                    theme.accent
                };
                (
                    progress.share(),
                    colour,
                    words::progress_text(&progress),
                    progress.encoder.as_deref().map(words::encoder_text),
                )
            }
            State::Done(done) => (
                1.0,
                ui_kit::status::GOOD,
                words::done_line(done, Some(job.estimated_bytes)),
                Some(words::done_text(done)),
            ),
            State::Failed { what, why } => (
                0.0,
                ui_kit::status::ERROR,
                format!("Failed: {what}"),
                Some(why.clone()),
            ),
            State::Cancelled => (0.0, theme.faint, "Cancelled; nothing was saved".to_owned(), None),
        };
        let bar_width = (ui.available_width() - 420.0).max(80.0);
        ui.add(
            egui::ProgressBar::new(share)
                .desired_width(bar_width)
                .desired_height(8.0)
                .fill(colour),
        );
        ui.add_space(8.0);
        let label = ui.add_sized(
            [210.0, 20.0],
            egui::Label::new(egui::RichText::new(state).color(theme.text_secondary)).truncate(),
        );
        if let Some(tip) = tip {
            label.on_hover_text(tip);
        }
        let mut button = |ui: &mut egui::Ui, text: &str, what: JobAct| {
            if ui.button(text).clicked() {
                act = Some(what);
            }
        };
        match &job.state {
            State::Held => {
                button(ui, "Start", JobAct::Start);
                button(ui, "Edit", JobAct::Edit);
                button(ui, "Remove", JobAct::Remove);
            }
            State::Next => {
                button(ui, "Edit", JobAct::Edit);
                button(ui, "Remove", JobAct::Remove);
            }
            State::Running(export) => {
                if export.is_paused() {
                    button(ui, "Resume", JobAct::Pause(false));
                } else {
                    button(ui, "Pause", JobAct::Pause(true));
                }
                button(ui, "Cancel", JobAct::Cancel);
            }
            State::Done(done) => {
                button(ui, "Show", JobAct::Show(done.path.clone()));
                button(ui, "Remove", JobAct::Remove);
            }
            State::Failed { .. } | State::Cancelled => {
                button(ui, "Try again", JobAct::Start);
                button(ui, "Remove", JobAct::Remove);
            }
        }
    });
    act
}

/// A preset in the list: its name, and the format under it; marked when
/// the settings are this preset's.
fn preset_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    preset: &Preset,
    choice: &Choice,
    target: Option<f32>,
) -> egui::Response {
    let on = preset.choice == *choice;
    let font = |size: f32, semibold: bool| {
        if semibold {
            egui::FontId::new(size, ui_kit::fonts::semibold())
        } else {
            egui::FontId::proportional(size)
        }
    };
    let width = ui.available_width();
    let about = ui
        .painter()
        .layout(preset.about(target), font(11.5, false), theme.muted, width - 20.0);
    let height = 30.0 + about.size().y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter();
    if on || response.hovered() {
        painter.rect_filled(rect, 6.0, theme.active);
    }
    if on {
        painter.rect_stroke(
            rect.shrink(0.5),
            6.0,
            egui::Stroke::new(1.0, theme.accent),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(10.0, 7.0),
        egui::Align2::LEFT_TOP,
        &preset.name,
        font(13.5, on),
        theme.text,
    );
    painter.galley(rect.left_top() + egui::vec2(10.0, 25.0), about, theme.muted);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn heading(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .color(theme.muted)
            .family(ui_kit::fonts::semibold()),
    );
    ui.add_space(4.0);
}

fn small_label(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme.text_secondary));
}

/// A section of the middle column: a framed box with its title.
fn section(ui: &mut egui::Ui, theme: &Theme, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme.panel)
        .stroke(egui::Stroke::new(1.0, theme.line))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).family(ui_kit::fonts::semibold()));
            ui.add_space(6.0);
            add(ui);
        });
}

/// A row: a name on the left, its value on the right.
fn row(ui: &mut egui::Ui, theme: &Theme, label: &str, value: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui_kit::inspector::label_cell(ui, theme, label, LABEL_WIDTH);
        value(ui);
    });
    ui.add_space(4.0);
}

fn container_words(container: Container) -> String {
    match container {
        Container::Mp4 => "MP4".to_owned(),
        Container::Mkv => "MKV (for keeping)".to_owned(),
        Container::Wav => "WAV (sound only)".to_owned(),
        Container::Mp3 => "MP3 (sound only)".to_owned(),
        Container::Ogg => "OGG (sound only)".to_owned(),
    }
}

fn codec_words(codec: VideoCodec) -> String {
    match codec {
        VideoCodec::H264 => "H.264 (plays everywhere)".to_owned(),
        VideoCodec::H265 => "H.265 (smaller files)".to_owned(),
        VideoCodec::Av1 => "AV1 (smallest, newest)".to_owned(),
    }
}

fn audio_words(codec: AudioCodec) -> String {
    match codec {
        AudioCodec::Pcm24 => "24-bit, uncompressed".to_owned(),
        AudioCodec::Flac => "FLAC (lossless)".to_owned(),
        other => other.label().to_owned(),
    }
}

fn quality_about(quality: Quality) -> &'static str {
    match quality {
        Quality::High => "For keeping, or for a site that encodes it again",
        Quality::Medium => "A good picture at a sensible size",
        Quality::Small => "For sending",
    }
}

/// Opens the folder picker on its own thread; the choice arrives later.
fn ask_folder(ctx: &egui::Context, now: &Path) -> Receiver<Option<PathBuf>> {
    let (tell, chosen) = channel();
    let ctx = ctx.clone();
    let now = now.to_path_buf();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Save the export in");
        if now.is_dir() {
            dialog = dialog.set_directory(now);
        }
        let _ = tell.send(ui_kit::file_dialog::run(&ctx, || dialog.pick_folder()));
        ctx.request_repaint();
    });
    chosen
}

fn show_folder(folder: &Path) -> diag::Result<()> {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(folder)
        .spawn()
        .map_err(|err| {
            AppError::new("The folder can't be shown")
                .why(format!(
                    "Starting the file manager for {} failed.",
                    folder.display()
                ))
                .fix("Open the folder yourself; its path is on the Export page.")
                .source(err)
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_preset_changes_are_written_one_at_a_time_and_the_newest_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut errors = ui_kit::ErrorCenter::default();
        let mut page = ExportPage::new(dir.path(), Pace::default(), &mut errors);
        let mut asked = Asked::default();
        // Save one, save another, remove the first: three writes in a row.
        page.own.add("First", Choice::default()).unwrap();
        page.save_own(&mut asked);
        page.own.add("Second", Choice::default()).unwrap();
        page.save_own(&mut asked);
        page.own.remove("First");
        page.save_own(&mut asked);
        page.own_keeper.flush().unwrap();
        let names: Vec<String> = OwnPresets::load(dir.path())
            .unwrap()
            .presets
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Second"]);
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1, "no temporary file is left");
        assert!(asked.problem.is_none() && errors.all().is_empty());
    }
}
