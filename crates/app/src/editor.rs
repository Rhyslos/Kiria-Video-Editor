//! The editor window.

use crate::import::{self, Importer, Read};
use crate::run_mode::RunMode;
use crate::settings::Settings;
use crate::timeline_panel;
use crate::viewer::Viewer;
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
    /// The launcher's picture of the project, being written after a save.
    thumbnail: Option<JobHandle<bool>>,
    /// A save finished (or the project has no picture yet): write one.
    thumbnail_due: Thumbnail,
    importer: Importer,
    import_requested: bool,
    viewer: Viewer,
    playback: Option<engine::Playback>,
    timeline: ui_kit::TimelineView,
    /// Clips picked on the timeline (their linked clips go with them).
    selected: Vec<project::ClipId>,
    /// The timeline was fitted to the project once already.
    fitted: bool,
    waveforms: crate::waveforms::Waveforms,
    pictures: crate::clip_pictures::ClipPictures,
    relink: crate::relink::Relinker,
    drawer: crate::media_drawer::MediaDrawer,
    frame_grab: crate::frame_grab::FrameGrab,
    silence: crate::silence_panel::SilencePanel,
    label_editor: Option<crate::label_editor::LabelEditor>,
    /// Save this frame was asked for; the dialog opens on the next frame.
    save_frame_requested: bool,
    /// `--play`: start playing as soon as the imports are in.
    play_when_ready: bool,
    show_timeline_settings: bool,
    show_settings: bool,
    show_diagnostics: bool,
    show_search: bool,
    search_text: String,
    status: String,
    playing: bool,
    /// Playback loops inside the region (L).
    looping: bool,
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
    /// Every frame's interface time, for the percentile.
    ui_times: Vec<Duration>,
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
        let imports = project.as_ref().map(|p| p.import.clone()).unwrap_or_default();
        let start_at = project.as_ref().and_then(|p| p.playhead);
        let play_when_ready = project.as_ref().is_some_and(|p| p.play);
        let drawer_open = project.as_ref().is_some_and(|p| p.media_drawer);
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
        let mut importer = Importer::default();
        if !imports.is_empty() {
            importer.read(&jobs, &cc.egui_ctx, imports);
        }
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
            thumbnail: None,
            thumbnail_due: Thumbnail::IfMissing,
            importer,
            import_requested: false,
            viewer: Viewer::new(&cc.egui_ctx, start_at),
            playback: None,
            timeline: ui_kit::TimelineView::default(),
            selected: Vec::new(),
            fitted: false,
            waveforms: crate::waveforms::Waveforms::default(),
            pictures: crate::clip_pictures::ClipPictures::default(),
            relink: crate::relink::Relinker::default(),
            frame_grab: crate::frame_grab::FrameGrab::default(),
            silence: crate::silence_panel::SilencePanel::default(),
            label_editor: None,
            save_frame_requested: false,
            drawer: crate::media_drawer::MediaDrawer {
                open: drawer_open,
                ..Default::default()
            },
            play_when_ready,
            show_timeline_settings: false,
            show_settings: false,
            show_diagnostics: false,
            show_search: false,
            search_text: String::new(),
            status: String::new(),
            playing: false,
            looping: false,
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
            Action::Save => self.with_document(Document::save).map(|()| {
                self.thumbnail_due = Thumbnail::Now;
            }),
            Action::AddLabel => {
                let at = self.edit_point();
                self.add_label(at)
            }
            Action::ToggleLoop => {
                self.looping = !self.looping;
                let has_region = self.region().is_some();
                self.status = match (self.looping, has_region) {
                    (true, true) => "Looping inside the region",
                    (true, false) => "Looping on: Shift-drag in the ruler to mark a region",
                    (false, _) => "Looping off",
                }
                .into();
                Ok(())
            }
            Action::ClearRegion => self.set_region(None),
            Action::RemoveSilence => self.with_document(|_| Ok(())).map(|()| self.silence.open = true),
            Action::SaveFrame => self
                .with_document(|_| Ok(()))
                .map(|()| self.save_frame_requested = true),
            Action::Split => {
                let at = self.edit_point();
                let clip = self.selected.first().copied();
                self.with_document(|d| {
                    let sequence = d.project().active_sequence;
                    d.apply(Command::Split { sequence, at, clip })
                })
            }
            Action::Delete | Action::RippleDelete => {
                if self.selected.is_empty() {
                    self.status = "Click a clip on the timeline first.".into();
                    Ok(())
                } else {
                    let ripple = action == Action::RippleDelete;
                    let clips = std::mem::take(&mut self.selected);
                    self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        for clip in clips {
                            // A linked clip may already have gone with an earlier one.
                            let exists = d
                                .project()
                                .active()
                                .is_some_and(|s| s.tracks().any(|t| t.clips.iter().any(|c| c.id == clip)));
                            if exists {
                                d.apply(Command::RemoveClip {
                                    sequence,
                                    clip,
                                    ripple,
                                })?;
                            }
                        }
                        Ok(())
                    })
                }
            }
            Action::ClearSelection => {
                self.selected.clear();
                Ok(())
            }
            Action::ZoomToFit => {
                self.fitted = false;
                Ok(())
            }
            Action::SelectTool => {
                self.timeline.tool = ui_kit::Tool::Select;
                Ok(())
            }
            Action::BladeTool
            | Action::RippleTool
            | Action::RollTool
            | Action::SlipTool
            | Action::SlideTool => {
                self.timeline.tool = match action {
                    Action::BladeTool => ui_kit::Tool::Blade,
                    Action::RippleTool => ui_kit::Tool::Ripple,
                    Action::RollTool => ui_kit::Tool::Roll,
                    Action::SlipTool => ui_kit::Tool::Slip,
                    _ => ui_kit::Tool::Slide,
                };
                Ok(())
            }
            Action::ToggleSnapping => {
                self.timeline.snapping = !self.timeline.snapping;
                self.status = if self.timeline.snapping {
                    "Snapping on"
                } else {
                    "Snapping off"
                }
                .into();
                Ok(())
            }
            Action::PlayPause => {
                match self.playback.take() {
                    // Pause: back to where playback started, to watch it again.
                    Some(playback) => self.viewer.playhead = playback.started_at(),
                    None => self.start_playback(),
                }
                self.after_play_change();
                Ok(())
            }
            Action::Stop => {
                if let Some(playback) = self.playback.take() {
                    self.viewer.playhead = self.snapped(playback.position());
                }
                self.after_play_change();
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
            Action::Import => self.with_document(|_| Ok(())).map(|()| {
                self.import_requested = true;
            }),
            Action::PreviousFrame | Action::NextFrame | Action::GoToStart | Action::GoToEnd => {
                if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                    let (rate, length) = (seq.settings.frame_rate, seq.length());
                    match action {
                        Action::PreviousFrame => self.viewer.step(-1, rate, length),
                        Action::NextFrame => self.viewer.step(1, rate, length),
                        Action::GoToStart => self.viewer.go_to(Time::ZERO, rate, length),
                        _ => self.viewer.go_to(length, rate, length),
                    }
                }
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
                .fix("Open a project from the launcher.")),
        }
    }

    /// Where cuts, splits and labels happen: the edit needle, or the playhead
    /// when they are merged (Settings).
    fn edit_point(&self) -> Time {
        let seq = self.document.as_ref().and_then(|d| d.project().active());
        match seq {
            Some(s) if !self.settings.edit_at_playhead => s.edit_needle,
            Some(s) => s.settings.frame_rate.snap(self.viewer.playhead),
            None => Time::ZERO,
        }
    }

    /// Adds a timeline label at `at`, named by its number.
    fn add_label(&mut self, at: Time) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let (sequence, number) = (seq.id, seq.labels.len() + 1);
            d.apply(Command::AddLabel {
                sequence,
                label: TimelineLabel {
                    id: LabelId(0),
                    name: format!("Label {number}"),
                    at: seq.settings.frame_rate.snap(at),
                    color: LabelColor::Blue,
                },
            })
        })
    }

    /// The open timeline's region.
    fn region(&self) -> Option<project::Region> {
        self.document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|s| s.region)
    }

    /// Sets (or clears) the region, on frames.
    fn set_region(&mut self, seconds_range: Option<(f64, f64)>) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let rate = seq.settings.frame_rate;
            let region = seconds_range
                .and_then(|(a, b)| project::Region::between(rate.snap(seconds(a)), rate.snap(seconds(b))));
            if region == seq.region {
                return Ok(());
            }
            let sequence = seq.id;
            d.apply(Command::SetRegion { sequence, region })
        })
    }

    /// Moves the playhead; while playing, playback carries on from there.
    fn seek(&mut self, seconds: f64) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return;
        };
        let (rate, length) = (seq.settings.frame_rate, seq.length());
        let at = Time::from_ratio((seconds * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO);
        let was_playing = self.playback.take().is_some();
        self.viewer.go_to(at, rate, length);
        if was_playing {
            self.start_playback();
        }
        self.after_play_change();
    }

    fn timeline_event(&mut self, event: ui_kit::TimelineEvent) {
        use ui_kit::TimelineEvent as E;
        match event {
            E::Seek(t) => self.seek(t),
            E::MoveNeedle(t) => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let at =
                        Time::from_ratio((t * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO);
                    let at = seq.settings.frame_rate.snap(at);
                    let sequence = seq.id;
                    d.apply(Command::SetEditNeedle { sequence, at })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::Select { clip, add } => timeline_panel::select(&mut self.selected, project::ClipId(clip), add),
            E::ClearSelection => self.selected.clear(),
            E::Move { clip, by } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let clip = project::ClipId(clip);
                    let Some(start) = seq
                        .tracks()
                        .flat_map(|t| t.clips.iter())
                        .find(|c| c.id == clip)
                        .map(|c| c.start)
                    else {
                        return Ok(());
                    };
                    let target = seq.settings.frame_rate.snap(start + seconds(by)).max(Time::ZERO);
                    let sequence = seq.id;
                    d.apply(Command::MoveLinked {
                        sequence,
                        clip,
                        by: target - start,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::Trim { clip, edge, to } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let to = seq.settings.frame_rate.snap(seconds(to));
                    let edge = match edge {
                        ui_kit::ClipEdge::Start => project::Edge::Start,
                        ui_kit::ClipEdge::End => project::Edge::End,
                    };
                    let sequence = seq.id;
                    d.apply(Command::Trim {
                        sequence,
                        clip: project::ClipId(clip),
                        edge,
                        to,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::TrackMix {
                track,
                muted,
                solo,
                gain_db,
            } => {
                let result = self.with_document(|d| {
                    let sequence = d.project().active_sequence;
                    d.apply(Command::SetTrackMix {
                        sequence,
                        track: project::TrackId(track),
                        muted,
                        solo,
                        gain_db,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
                // Playing on with the new mix.
                if let Some(playback) = self.playback.take() {
                    let at = playback.position();
                    drop(playback);
                    self.viewer.playhead = at;
                    self.start_playback();
                }
            }
            E::RippleTrim { clip, edge, to } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let to = seq.settings.frame_rate.snap(seconds(to));
                    let edge = match edge {
                        ui_kit::ClipEdge::Start => project::Edge::Start,
                        ui_kit::ClipEdge::End => project::Edge::End,
                    };
                    let sequence = seq.id;
                    d.apply(Command::RippleTrim {
                        sequence,
                        clip: project::ClipId(clip),
                        edge,
                        to,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::Roll { clip, to } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let to = seq.settings.frame_rate.snap(seconds(to));
                    let sequence = seq.id;
                    d.apply(Command::Roll {
                        sequence,
                        clip: project::ClipId(clip),
                        to,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::Slip { clip, by } | E::Slide { clip, by } => {
                let slide = matches!(event, E::Slide { .. });
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    // Whole frames only, so cuts stay on frames.
                    let step = seq.settings.frame_rate.snap(seconds(by.abs()));
                    if step == Time::ZERO {
                        return Ok(());
                    }
                    let by = if by < 0.0 { Time::ZERO - step } else { step };
                    let (sequence, clip) = (seq.id, project::ClipId(clip));
                    d.apply(if slide {
                        Command::Slide { sequence, clip, by }
                    } else {
                        Command::Slip { sequence, clip, by }
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::SetRegion(range) => {
                if let Err(err) = self.set_region(range) {
                    self.errors.push(&err);
                }
            }
            E::EditLabel(id) => {
                self.label_editor = self
                    .document
                    .as_ref()
                    .and_then(|d| d.project().active())
                    .and_then(|s| s.labels.iter().find(|l| l.id.0 == id))
                    .map(crate::label_editor::LabelEditor::new);
            }
            E::AddLabel(at) => {
                if let Err(err) = self.add_label(seconds(at)) {
                    self.errors.push(&err);
                }
            }
            E::SplitAt { at, clip } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let at = seq.settings.frame_rate.snap(seconds(at));
                    let sequence = seq.id;
                    d.apply(Command::Split {
                        sequence,
                        at,
                        clip: clip.map(project::ClipId),
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::Blade { clip, at } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let at = seq.settings.frame_rate.snap(seconds(at));
                    let sequence = seq.id;
                    d.apply(Command::Split {
                        sequence,
                        at,
                        clip: Some(project::ClipId(clip)),
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
        }
    }

    fn start_playback(&mut self) {
        let Some(document) = &self.document else { return };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        let length = seq.length();
        if length == Time::ZERO {
            return;
        }
        // From the end, play from the start again.
        let last = (length - seq.settings.frame_rate.frame_duration()).max(Time::ZERO);
        let from = if self.viewer.playhead >= last {
            Time::ZERO
        } else {
            self.viewer.playhead
        };
        let from = crate::looping::start_at(from, seq.region, self.looping);
        self.viewer.playhead = from;
        self.playback = Some(engine::Playback::start(p.clone(), seq.id, from));
    }

    fn after_play_change(&mut self) {
        self.playing = self.playback.is_some();
        self.jobs.set_playing(self.playing);
        let interval = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .map(|s| 1.0 / s.settings.frame_rate.as_f64());
        self.viewer
            .set_playing(if self.playing { interval } else { None });
    }

    /// `at` on a frame of the open timeline.
    fn snapped(&self, at: Time) -> Time {
        self.document
            .as_ref()
            .and_then(|d| d.project().active())
            .map_or(at, |s| s.settings.frame_rate.snap(at))
    }

    /// While playing: the playhead follows the clock, and playback stops at the end.
    fn follow_playback(&mut self, ctx: &egui::Context) {
        let Some(playback) = &self.playback else { return };
        let seq = self.document.as_ref().and_then(|d| d.project().active());
        let (length, rate) = seq.map_or((Time::ZERO, project::Rate::FPS_60), |s| {
            (s.length(), s.settings.frame_rate)
        });
        let at = playback.position();
        let region = seq.and_then(|s| s.region);
        if let Some(back) = crate::looping::wrap(at, region, self.looping, length) {
            self.playback = None;
            self.viewer.playhead = back;
            self.start_playback();
            ctx.request_repaint();
        } else if at >= length {
            self.playback = None;
            self.viewer.go_to(length, rate, length);
            self.after_play_change();
        } else {
            self.viewer.playhead = at;
            ctx.request_repaint();
        }
    }

    /// Starts imports asked for this frame and adds finished ones to the project.
    fn imports(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.import_requested) {
            self.importer.choose_files(ctx);
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            match self.with_document(|_| Ok(())) {
                Ok(()) => self.importer.read(&self.jobs, ctx, dropped),
                Err(err) => self.errors.push(&err),
            }
        }
        for read in self.importer.poll(&self.jobs, ctx) {
            let result = match read {
                Read::Ready(media) => {
                    let line = import::summary(&media);
                    let mut settled = None;
                    self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        let was_automatic = d.project().active().is_some_and(|s| s.automatic);
                        d.apply(Command::Import { sequence, media })?;
                        if was_automatic && let Some(s) = d.project().active().filter(|s| !s.automatic) {
                            settled = Some(s.settings);
                        }
                        Ok(())
                    })
                    .map(|()| {
                        // Show the whole timeline again with the new file in it.
                        self.fitted = false;
                        self.status = match settled {
                            Some(s) => format!(
                                "Imported {line}. The timeline is now {} at {} kHz to match it (Timeline settings to change).",
                                import::settings_text(&s),
                                s.sample_rate as f32 / 1000.0
                            ),
                            None => format!("Imported {line}"),
                        };
                    })
                }
                Read::Failed(path, err) => {
                    tracing::info!(path = %path.display(), "import failed: {}", err.report());
                    Err(err)
                }
            };
            if let Err(err) = result {
                self.errors.push(&err);
            }
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
            match document.finish_save(*seq, result) {
                Ok(()) => self.thumbnail_due = Thumbnail::Now,
                Err(err) => self.errors.push(&err),
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

    /// Writes the launcher's picture of the project on a worker when due:
    /// the frame at the playhead, or the first clip's in a gap.
    fn thumbnails(&mut self) {
        if let Some(handle) = &self.thumbnail
            && let Some(result) = handle.try_result()
        {
            if let Err(err) = result {
                tracing::info!("no project picture: {}", err.report());
            }
            self.thumbnail = None;
        }
        let Some(document) = &self.document else { return };
        if self.thumbnail.is_some() || self.thumbnail_due == Thumbnail::No || document.is_read_only() {
            return;
        }
        let only_if_missing = self.thumbnail_due == Thumbnail::IfMissing;
        self.thumbnail_due = Thumbnail::No;
        let (project, out, at) = (
            document.project().clone(),
            project::thumbnail_path(document.path()),
            self.viewer.playhead,
        );
        self.thumbnail = Some(self.jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "project picture",
            move |_| {
                if only_if_missing && out.exists() {
                    return Ok(false);
                }
                engine::thumbnail::write(&project, project.active_sequence, at, &out)
            },
        ));
    }

    /// Opens the save dialog when asked, and reports finished saves.
    fn frame_grabs(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.save_frame_requested)
            && let Some(document) = &self.document
            && let Some(seq) = document.project().active()
        {
            let at = seq.settings.frame_rate.snap(self.viewer.playhead);
            let name = crate::frame_grab::default_name(
                &document.project().name,
                &at.timecode(seq.settings.frame_rate),
            );
            self.frame_grab.start(ctx, document.project(), at, name);
        }
        match self.frame_grab.poll(&self.jobs, ctx) {
            Some(crate::frame_grab::Saved::Done(path)) => {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.status = format!("Saved the frame as {name}.");
            }
            Some(crate::frame_grab::Saved::Failed(err)) => self.errors.push(&err),
            None => {}
        }
    }

    /// The window for one timeline label, and what it asks for.
    fn label_window(&mut self, ctx: &egui::Context) {
        use crate::label_editor::Asked;
        let Some(editor) = self.label_editor.as_mut() else {
            return;
        };
        let asked = editor.ui(ctx, &self.theme);
        let result = match asked {
            Asked::Nothing => return,
            Asked::Close => Ok(()),
            Asked::Save { id, name, color } => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::EditLabel {
                    sequence,
                    label: id,
                    name,
                    color,
                })
            }),
            Asked::Remove(label) => self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::RemoveLabel { sequence, label })
            }),
        };
        match result {
            Ok(()) => self.label_editor = None,
            Err(err) => self.errors.push(&err),
        }
    }

    /// The Remove silence window, and the cut it asks for.
    fn silence_window(&mut self, ctx: &egui::Context) {
        if !self.silence.open {
            return;
        }
        let Some(document) = &self.document else {
            self.silence.open = false;
            return;
        };
        let (jobs, waveforms) = (&self.jobs, &mut self.waveforms);
        let ready = self.silence.refresh(document.project(), |m, stream| {
            waveforms.get(jobs, ctx, m, stream).cloned()
        });
        let asked = self.silence.ui(ctx, &self.theme, document.project(), ready);
        if let crate::silence_panel::Asked::Remove(ranges) = asked {
            let count = ranges.len();
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::RemoveRanges { sequence, ranges })
            });
            match result {
                Ok(()) => {
                    self.selected.clear();
                    self.fitted = false;
                    self.status = format!(
                        "Removed {count} quiet stretch{}.",
                        if count == 1 { "" } else { "es" }
                    );
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// The tab on the left edge and the media drawer it opens.
    fn drawer_panels(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("edge-tab")
            .exact_size(layout::EDGE_TAB_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                if ui_kit::drawer::edge_tab(ui, &self.theme, "Media", self.drawer.open).clicked() {
                    self.drawer.open = !self.drawer.open;
                }
            });
        let mut picked = None;
        let mut open = self.drawer.open;
        egui::Panel::left("media-drawer")
            .exact_size(layout::DRAWER_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)))
            .show_collapsible(ui, &mut open, |ui| {
                if let Some(document) = &self.document {
                    picked = self
                        .drawer
                        .ui(ui, &self.theme, document.project(), &self.relink.missing);
                }
            });
        self.drawer.open = open;
        if let Some(media) = picked {
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::PlaceMedia { sequence, media })
            });
            match result {
                Ok(()) => self.fitted = false,
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// Notices media that moved, and points the project at files found again.
    fn relinking(&mut self, ctx: &egui::Context) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        self.relink.watch(&self.jobs, ctx, document.project());
        if self.relink.searching().is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let Some(found) = self.relink.poll(&self.jobs, document.project()) else {
            return;
        };
        for (media, path) in &found.files {
            let command = Command::Relink {
                media: *media,
                path: path.clone(),
            };
            if let Err(err) = document.apply(command) {
                self.errors.push(&err);
                return;
            }
            self.waveforms.forget(*media);
            self.pictures.forget(*media);
        }
        self.status = crate::relink::outcome(&found);
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
                let settings = if s.automatic {
                    "Automatic".to_owned()
                } else {
                    import::settings_text(&s.settings)
                };
                format!("{} · {settings} · {saved}", s.name)
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
        if self.page == Page::Edit && self.document.is_some() {
            self.edit_page(ui);
            return;
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

    /// The Edit page until the viewer and the timeline widget arrive: import,
    /// and a plain list of what is on the timeline.
    fn edit_page(&mut self, ui: &mut egui::Ui) {
        self.drawer_panels(ui);
        let mut transport_click = None;
        let mut timeline_events = Vec::new();
        let mut toolbar_click = None;
        let shortcut = self
            .shortcuts
            .shortcut_for(Action::Import)
            .map_or_else(String::new, |s| s.label());
        ui.horizontal(|ui| {
            let button = ui_kit::accent_button(ui, &self.theme, "Import media…").on_hover_text(&shortcut);
            if button.clicked() {
                self.run_action(Action::Import);
            }
            if ui.button("Timeline settings…").clicked() {
                self.show_timeline_settings = true;
            }
            if let Some(document) = &self.document
                && !self.relink.missing.is_empty()
            {
                ui.add_space(16.0);
                ui_kit::status_dot(ui, ui_kit::status::WARNING);
                let text = crate::relink::summary(document.project(), &self.relink.missing);
                ui.label(egui::RichText::new(text).color(self.theme.text_secondary));
                match self.relink.searching() {
                    Some(folder) => {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(format!("Looking in {}…", folder.display()))
                                .color(self.theme.muted),
                        );
                    }
                    None => {
                        if ui
                            .button("Find them…")
                            .on_hover_text("Pick a folder: the files are found in it or its subfolders, even if renamed.")
                            .clicked()
                        {
                            self.relink.choose_folder(ui.ctx());
                        }
                    }
                }
            }
            let busy = self.importer.busy();
            if busy > 0 {
                ui.spinner();
                ui.label(
                    egui::RichText::new(format!(
                        "Reading {busy} file{}…",
                        if busy == 1 { "" } else { "s" }
                    ))
                    .color(self.theme.text_secondary),
                );
            }
        });
        ui.add_space(12.0);
        let Some(document) = &self.document else { return };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        let rate = seq.settings.frame_rate;
        if seq.length() == project::Time::ZERO {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nothing here yet").family(ui_kit::fonts::semibold()).size(18.0));
                ui.label(
                    egui::RichText::new(format!(
                        "Drop recordings on the window, or press {shortcut} to import them. Each file goes at the end of the timeline, with every sound stream on its own track."
                    ))
                    .color(self.theme.muted),
                );
            });
        } else {
            let quality_before = self.settings.preview_quality;
            let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
            let viewer_height = (ui.available_height() * 0.55).max(220.0);
            transport_click = self.viewer.ui(
                ui,
                &self.theme,
                p,
                &mut self.settings.preview_quality,
                needle,
                viewer_height,
            );
            if self.settings.preview_quality != quality_before
                && let Err(err) = self.settings.save(&self.data_dir)
            {
                self.errors.push(&err);
            }
            ui.add_space(4.0);
            let tip = |a: Action| {
                let key = self
                    .shortcuts
                    .shortcut_for(a)
                    .map_or_else(String::new, |s| format!(" ({})", s.label()));
                format!("{}{key}", a.label())
            };
            let tips = [
                tip(Action::SelectTool),
                tip(Action::BladeTool),
                tip(Action::RippleTool),
                tip(Action::RollTool),
                tip(Action::SlipTool),
                tip(Action::SlideTool),
                tip(Action::ToggleSnapping),
                tip(Action::ZoomToFit),
            ];
            toolbar_click = ui_kit::timeline::toolbar(
                ui,
                &self.theme,
                &self.timeline,
                [
                    &tips[0], &tips[1], &tips[2], &tips[3], &tips[4], &tips[5], &tips[6], &tips[7],
                ],
            );
            timeline_panel::keep_existing(&mut self.selected, seq);
            let (jobs, waveforms, pictures, ctx) = (
                &self.jobs,
                &mut self.waveforms,
                &mut self.pictures,
                ui.ctx().clone(),
            );
            let missing = &self.relink.missing;
            let drawn = timeline_panel::drawn(
                p,
                seq,
                &self.selected,
                missing,
                |m, stream| waveforms.get(jobs, &ctx, m, stream).cloned(),
                |m, stream| pictures.get(jobs, &ctx, m, stream),
            );
            let length = seq.length().as_secs_f64();
            let width = ui.available_width();
            if !self.fitted {
                self.timeline.fit(length, width - ui_kit::timeline::HEADER_WIDTH);
                self.fitted = true;
            }
            let data = ui_kit::TimelineData {
                video: &drawn.video,
                audio: &drawn.audio,
                length,
                fps: rate.as_f64(),
                playhead: self.viewer.playhead.as_secs_f64(),
                needle: needle.map(Time::as_secs_f64),
                region: seq.region.map(|r| (r.start.as_secs_f64(), r.end.as_secs_f64())),
                markers: &drawn.markers,
                highlights: &self.silence.found,
                follow: self.playback.is_some(),
            };
            let status_height = if self.status.is_empty() { 0.0 } else { 24.0 };
            let size = egui::vec2(width, (ui.available_height() - status_height).max(160.0));
            timeline_events = self.timeline.show(ui, &self.theme, &data, size);
        }
        for event in timeline_events {
            self.timeline_event(event);
        }
        if let Some(click) = toolbar_click {
            self.run_action(match click {
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Select) => Action::SelectTool,
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Blade) => Action::BladeTool,
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Ripple) => Action::RippleTool,
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Roll) => Action::RollTool,
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Slip) => Action::SlipTool,
                ui_kit::timeline::ToolbarClick::Tool(ui_kit::Tool::Slide) => Action::SlideTool,
                ui_kit::timeline::ToolbarClick::Snapping => Action::ToggleSnapping,
                ui_kit::timeline::ToolbarClick::Fit => Action::ZoomToFit,
            });
        }
        if let Some(click) = transport_click {
            self.run_action(match click {
                ui_kit::TransportClick::FrameBack => Action::PreviousFrame,
                ui_kit::TransportClick::FrameForward => Action::NextFrame,
                ui_kit::TransportClick::PlayPause => Action::PlayPause,
                ui_kit::TransportClick::Stop => Action::Stop,
            });
        }
        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.status).color(self.theme.text_secondary));
        }
    }

    /// Picture size, frame rate and sound rate of the open timeline: Automatic,
    /// a template, or the same as one of the project's videos.
    fn timeline_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_timeline_settings {
            return;
        }
        let Some(document) = &self.document else {
            self.show_timeline_settings = false;
            return;
        };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        let current = if seq.automatic {
            "Automatic: the first video you import decides.".to_owned()
        } else {
            format!(
                "{} · {} kHz",
                import::settings_text(&seq.settings),
                seq.settings.sample_rate as f32 / 1000.0
            )
        };
        let mut choices: Vec<(String, project::SequenceSettings)> = project::SequenceSettings::TEMPLATES
            .iter()
            .map(|(name, s)| (format!("{name} · {}", import::settings_text(s)), *s))
            .collect();
        for media in p.media.values() {
            if let (project::MediaKind::Video, Some(v)) = (media.kind, &media.info.video) {
                let s = project::SequenceSettings::matching(v, &media.info.audio);
                let name = media
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                choices.push((format!("Same as {name} · {}", import::settings_text(&s)), s));
            }
        }
        let sequence = seq.id;
        let mut chosen = None;
        let mut open = true;
        egui::Window::new("Timeline settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(current).family(ui_kit::fonts::semibold()));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Change to").color(self.theme.text_secondary));
                for (label, settings) in &choices {
                    if ui.button(label).clicked() {
                        chosen = Some(*settings);
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Clips of another size or frame rate are fitted, never changed.")
                        .size(11.5)
                        .color(self.theme.muted),
                );
            });
        if let Some(settings) = chosen {
            if let Err(err) = self.with_document(|d| d.apply(Command::SetSettings { sequence, settings })) {
                self.errors.push(&err);
            }
            self.show_timeline_settings = false;
        } else {
            self.show_timeline_settings = open;
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
        use crate::search::Hit;
        let mut open = self.show_search;
        let project = self.document.as_ref().map(Document::project);
        let seq = project.and_then(|p| p.active());
        let fps = seq.map_or(60.0, |s| s.settings.frame_rate.as_f64());
        let hits = crate::search::results(&self.search_text, &self.shortcuts, seq, |clip| {
            project
                .and_then(|p| p.media.get(&clip.media))
                .and_then(|m| m.path.file_name())
                .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned())
        });
        let mut chosen = None;
        egui::Window::new("Search")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let field = ui.text_edit_singleline(&mut self.search_text);
                field.request_focus();
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = hits.first().cloned();
                }
                ui.label(
                    egui::RichText::new(
                        "Commands, settings, timeline labels and clips. Enter picks the first.",
                    )
                    .color(self.theme.muted),
                );
                for hit in hits.iter().take(crate::search::SHOWN) {
                    if ui.selectable_label(false, hit.text(fps)).clicked() {
                        chosen = Some(hit.clone());
                    }
                }
                if hits.len() > crate::search::SHOWN {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} more: type more of the name.",
                            hits.len() - crate::search::SHOWN
                        ))
                        .color(self.theme.muted),
                    );
                }
            });
        self.show_search = open && chosen.is_none();
        match chosen {
            Some(Hit::Command { action, .. }) => self.run_action(action),
            Some(Hit::Label { at, .. }) => self.seek(at),
            Some(Hit::Clip { id, at, .. }) => {
                self.selected = vec![id];
                self.seek(at);
            }
            Some(Hit::Settings) => self.show_settings = true,
            Some(Hit::TimelineSettings) => self.show_timeline_settings = true,
            None => {}
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
                probe.ui_times.push(ui_time);
                probe.frames += 1;
                let done = probe.job.as_ref().and_then(JobHandle::try_result).is_some();
                if done {
                    let elapsed = probe.started.map_or(Duration::ZERO, |s| now - s);
                    let fps = probe.frames as f64 / elapsed.as_secs_f64().max(0.001);
                    // One hiccup from the operating system (common on shared test
                    // machines) shouldn't fail it: 99 frames in 100 must take under
                    // 16 ms, and none over 50 ms.
                    probe.ui_times.sort();
                    let p99 = probe
                        .ui_times
                        .get(probe.ui_times.len() * 99 / 100)
                        .copied()
                        .unwrap_or_default();
                    let failed =
                        p99 > Duration::from_millis(16) || probe.slowest_ui > Duration::from_millis(50);
                    let message = format!(
                        "Responsiveness test {}: {} frames in {:.1} s ({fps:.0} per second), interface work {:.1} ms for 99 frames in 100 and {:.1} ms at most, longest gap between frames {:.1} ms.",
                        if failed { "FAILED" } else { "passed" },
                        probe.frames,
                        elapsed.as_secs_f64(),
                        p99.as_secs_f64() * 1000.0,
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
        self.imports(&ctx);
        if self.play_when_ready && self.importer.busy() == 0 && self.document.is_some() {
            self.play_when_ready = false;
            self.run_action(Action::PlayPause);
        }
        self.follow_playback(&ctx);
        self.waveforms.poll();
        self.pictures.poll(&ctx);
        self.relinking(&ctx);
        self.frame_grabs(&ctx);
        if let Some(err) = self.viewer.poll(&ctx, self.settings.preview_quality)
            && !self.relink.explains_errors()
        {
            self.errors.push(&err);
        }
        self.autosave(now, idle);
        self.thumbnails();

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
        self.timeline_settings_window(&ctx);
        self.search_window(&ctx);
        self.silence_window(&ctx);
        self.label_window(&ctx);
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
        {
            match document.save() {
                Ok(()) => self.thumbnail_due = Thumbnail::Now,
                Err(err) => tracing::error!("saving on exit failed: {}", err.report()),
            }
        }
        if self.thumbnail_due == Thumbnail::Now
            && let Some(document) = &self.document
        {
            // One small frame on the way out, so the launcher shows the latest.
            let p = document.project();
            let out = project::thumbnail_path(document.path());
            if let Err(err) = engine::thumbnail::write(p, p.active_sequence, self.viewer.playhead, &out) {
                tracing::info!("no project picture: {}", err.report());
            }
        }
    }
}

/// Whether the launcher's picture of the project needs writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Thumbnail {
    No,
    /// On opening: only for a project that has none yet.
    IfMissing,
    Now,
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

/// Seconds (as the timeline widget gives them) as exact time, to the microsecond.
fn seconds(s: f64) -> Time {
    Time::from_ratio((s * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO)
}

/// A project named on the command line, and what to do with unsaved work from a crash.
#[derive(Debug, Clone)]
pub struct ProjectToOpen {
    pub path: PathBuf,
    /// The launcher asks; the editor on its own always recovers.
    pub recovery: Recovery,
    /// Media files to import once it is open.
    pub import: Vec<PathBuf>,
    /// Where the playhead starts, in seconds.
    pub playhead: Option<f64>,
    /// Start playing once imports are done.
    pub play: bool,
    /// Start with the media drawer open.
    pub media_drawer: bool,
}

/// Opens a project, or creates it if it doesn't exist.
fn open_or_create(path: &Path, recovery: Recovery) -> diag::Result<Document> {
    if path.exists() {
        Document::open(path, recovery)
    } else {
        let name = path
            .file_stem()
            .map_or("Untitled".into(), |s| s.to_string_lossy().into_owned());
        Document::create(path, project::Project::new_automatic(name))
    }
}

fn save_png(image: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    image::save_buffer(path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("The screenshot could not be saved: {e}"))
}
