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

/// What `main` hands the window to start with.
#[derive(Debug)]
pub struct Start {
    pub settings: Settings,
    pub data_dir: PathBuf,
    pub project: Option<ProjectToOpen>,
    pub mode: RunMode,
    pub report: Arc<Mutex<RunReport>>,
    /// Problems found before the window opened, shown once it has.
    pub problems: Vec<AppError>,
    /// A scripted pointer, for screenshots of pointing and dragging.
    pub pointer: Option<crate::fake_pointer::FakePointer>,
    /// Open the keyboard shortcuts page in this state, for screenshots.
    pub shortcuts_page: Option<ui_kit::keyboard::Preview>,
}

pub struct Editor {
    settings: Settings,
    data_dir: PathBuf,
    /// What draws the window: the first-frames check, the question after a
    /// failed start with Vulkan, and the Graphics row in Settings.
    graphics: graphics::WindowGraphics,
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
    /// Keeps the timeline's layout (track heights, the divider between
    /// video and audio) in a small file beside the project.
    view_keeper: Option<crate::view_state::ViewKeeper>,
    /// Clips picked on the timeline (their linked clips go with them).
    selected: Vec<project::ClipId>,
    /// The timeline was fitted to the project once already.
    fitted: bool,
    waveforms: crate::waveforms::Waveforms,
    pictures: crate::clip_pictures::ClipPictures,
    /// How fast each track's waveforms or pictures are being read, for the time left.
    track_paces: timeline_panel::TrackPaces,
    relink: crate::relink::Relinker,
    drawer: crate::media_drawer::MediaDrawer,
    /// The selected clip's settings, on the right of the Edit page.
    inspector: crate::inspector::Inspector,
    /// The Export page: what to export, and the export under way.
    export_page: crate::export_page::ExportPage,
    frame_grab: crate::frame_grab::FrameGrab,
    silence: crate::silence_panel::SilencePanel,
    label_editor: Option<crate::label_editor::LabelEditor>,
    master: ui_kit::master::MasterColumn,
    versions: crate::versions_window::VersionsWindow,
    shortcuts_page: Option<ui_kit::keyboard::ShortcutsPage>,
    cheat_sheet: crate::cheat_sheet::CheatSheet,
    /// Commands asked for on the command line, run on the first frames.
    startup_actions: Vec<Action>,
    /// The viewer fills the screen (F; Escape returns).
    full_screen: bool,
    /// Tell the window about a full-screen change on the next frame.
    full_screen_changed: bool,
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
    /// `--pointer`: a scripted pointer for screenshots.
    pointer: Option<crate::fake_pointer::FakePointer>,
    frames: u64,
    probe: Probe,
    report: Arc<Mutex<RunReport>>,
}

/// The longest a screenshot waits for imports and the viewer's picture.
const SCREENSHOT_WAIT: Duration = Duration::from_secs(20);
/// How long a scripted pointer rests before the screenshot, so tooltips are up.
const POINTER_REST: Duration = Duration::from_millis(1500);

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
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start, opened: graphics::Opened) -> Editor {
        let Start {
            settings,
            data_dir,
            project,
            mode,
            report,
            problems,
            pointer,
            shortcuts_page,
        } = start;
        ui_kit::setup(&cc.egui_ctx, settings.theme);
        let jobs = JobSystem::new(settings.resources.resolve(&jobs::Machine::detect()));
        let mut errors = ErrorCenter::default();
        for problem in &problems {
            errors.push(problem);
        }
        let graphics = graphics::WindowGraphics::new(cc, opened, &mut errors);
        let imports = project.as_ref().map(|p| p.import.clone()).unwrap_or_default();
        let start_at = project.as_ref().and_then(|p| p.playhead);
        let play_when_ready = project.as_ref().is_some_and(|p| p.play);
        let drawer_open = project.as_ref().is_some_and(|p| p.media_drawer);
        let start_page = if project.as_ref().is_some_and(|p| p.export_page) {
            Page::Export
        } else {
            Page::default()
        };
        let full_screen = project.as_ref().is_some_and(|p| p.full_screen);
        let hold_reading = project.as_ref().and_then(|p| p.hold_reading);
        let convert = crate::video_picture::conversion(
            cc.wgpu_render_state.as_ref(),
            project.as_ref().is_some_and(|p| p.cpu_colour),
        );
        let decoding =
            crate::device_decoding::WindowDecoding::new(&graphics, cc.wgpu_render_state.as_ref(), convert);
        // The usual shortcuts, with the ones changed on the shortcuts page.
        let shortcuts = ShortcutTable::restored(&settings.shortcuts);
        let shortcuts_page =
            shortcuts_page.map(|state| ui_kit::keyboard::ShortcutsPage::preview(state, &shortcuts));
        let startup_actions: Vec<Action> = project
            .as_ref()
            .map(|p| {
                p.actions
                    .iter()
                    .filter_map(|name| {
                        let found = Action::from_name(name);
                        if found.is_none() {
                            tracing::warn!("--action {name}: no such command");
                        }
                        found
                    })
                    .collect()
            })
            .unwrap_or_default();
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
        let mut timeline = ui_kit::TimelineView::default();
        let view_keeper = document.as_ref().map(|d| {
            crate::view_state::ViewKeeper::open(
                project::view_path(d.path()),
                !d.is_read_only(),
                &mut timeline,
            )
        });
        let mut importer = Importer::default();
        if !imports.is_empty() {
            importer.read(&jobs, &cc.egui_ctx, imports);
        }
        Editor {
            theme: settings.theme.theme(),
            settings,
            data_dir,
            graphics,
            page: start_page,
            shortcuts,
            errors,
            jobs,
            usage: UsageMonitor::start(Duration::from_secs(1)),
            document,
            pending_save: None,
            thumbnail: None,
            thumbnail_due: Thumbnail::IfMissing,
            importer,
            import_requested: false,
            viewer: Viewer::new(
                &cc.egui_ctx,
                start_at,
                convert,
                decoding,
                cc.wgpu_render_state
                    .as_ref()
                    .map(|state| (state.device.clone(), state.queue.clone())),
            ),
            playback: None,
            timeline,
            view_keeper,
            selected: Vec::new(),
            fitted: false,
            waveforms: crate::waveforms::Waveforms::holding_at(hold_reading),
            pictures: crate::clip_pictures::ClipPictures::holding_at(hold_reading),
            track_paces: timeline_panel::TrackPaces::default(),
            relink: crate::relink::Relinker::default(),
            frame_grab: crate::frame_grab::FrameGrab::default(),
            silence: crate::silence_panel::SilencePanel::default(),
            label_editor: None,
            master: ui_kit::master::MasterColumn::default(),
            versions: crate::versions_window::VersionsWindow::default(),
            shortcuts_page,
            cheat_sheet: crate::cheat_sheet::CheatSheet::default(),
            startup_actions,
            full_screen,
            full_screen_changed: full_screen,
            save_frame_requested: false,
            drawer: crate::media_drawer::MediaDrawer {
                open: drawer_open,
                ..Default::default()
            },
            inspector: crate::inspector::Inspector::default(),
            export_page: crate::export_page::ExportPage::default(),
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
            pointer,
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
                if self.full_screen {
                    self.set_full_screen(false);
                } else {
                    self.selected.clear();
                }
                Ok(())
            }
            Action::Versions => self.with_document(|_| Ok(())).map(|()| {
                self.versions.open = true;
                self.versions.show_again();
            }),
            Action::KeyboardShortcuts => {
                self.shortcuts_page.get_or_insert_with(Default::default);
                Ok(())
            }
            Action::RoughCut => {
                self.settings.rough_cut = !self.settings.rough_cut;
                self.timeline.rough_cut = self.settings.rough_cut;
                self.status = if self.settings.rough_cut {
                    "Rough cut view: each clip's sound is drawn inside it"
                } else {
                    "Full timeline view"
                }
                .into();
                self.settings.save(&self.data_dir)
            }
            Action::FullScreenViewer => {
                self.set_full_screen(!self.full_screen);
                Ok(())
            }
            Action::AddVideoTrack => self.add_track(project::TrackKind::Video),
            Action::AddAudioTrack => self.add_track(project::TrackKind::Audio),
            Action::ZoomToFit => {
                self.fitted = false;
                Ok(())
            }
            Action::ZoomIn
            | Action::ZoomOut
            | Action::TallerTracks
            | Action::ShorterTracks
            | Action::UsualTrackHeights => {
                self.timeline.step(match action {
                    Action::ZoomIn => ui_kit::ViewStep::ZoomIn,
                    Action::ZoomOut => ui_kit::ViewStep::ZoomOut,
                    Action::TallerTracks => ui_kit::ViewStep::TallerTracks,
                    Action::ShorterTracks => ui_kit::ViewStep::ShorterTracks,
                    _ => ui_kit::ViewStep::UsualTrackHeights,
                });
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

    /// The few shortcuts worth knowing first, as they are now: "M adds a
    /// label, Ctrl+Z undoes, …". Commands without a shortcut are left out.
    fn key_hints(&self) -> String {
        let hints: Vec<String> = [
            (Action::AddLabel, "adds a label"),
            (Action::Undo, "undoes"),
            (Action::Redo, "redoes"),
            (Action::Save, "saves"),
        ]
        .iter()
        .filter_map(|(action, does)| {
            Some(format!(
                "{} {does}",
                self.shortcuts.shortcut_for(*action)?.label()
            ))
        })
        .collect();
        if hints.is_empty() {
            String::new()
        } else {
            format!("{}.", hints.join(", "))
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

    /// Adds an empty track after the others of its kind, named by its number.
    fn add_track(&mut self, kind: project::TrackKind) -> diag::Result<()> {
        let mut added = None;
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let (sequence, name) = (seq.id, seq.next_track_name(kind));
            d.apply(Command::AddTrack {
                sequence,
                kind,
                name: name.clone(),
            })?;
            added = Some(name);
            Ok(())
        })?;
        if let Some(name) = added {
            self.status = crate::timeline_panel::added_track_says(&name, kind, self.settings.rough_cut);
        }
        Ok(())
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
            E::Move { clip, by, track } => {
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
                    let (sequence, by) = (seq.id, target - start);
                    d.apply(match track {
                        Some(track) => Command::MoveToTrack {
                            sequence,
                            clip,
                            track: project::TrackId(track),
                            by,
                        },
                        None => Command::MoveLinked { sequence, clip, by },
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
                self.play_on_with_new_mix();
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
            E::DropMedia {
                media,
                at,
                insert,
                track,
            } => {
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let at = seq.settings.frame_rate.snap(seconds(at));
                    d.apply(Command::DropMedia {
                        sequence: seq.id,
                        media: project::MediaId(media),
                        at,
                        track: track.map(project::TrackId),
                        insert,
                    })
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::AddTrack { audio } => {
                let kind = if audio {
                    project::TrackKind::Audio
                } else {
                    project::TrackKind::Video
                };
                if let Err(err) = self.add_track(kind) {
                    self.errors.push(&err);
                }
            }
            E::RemoveTrack { .. } | E::HideTrack { .. } | E::RenameTrack { .. } => {
                let result = self.with_document(|d| {
                    match timeline_panel::track_command(d.project().active_sequence, event) {
                        Some(command) => d.apply(command),
                        None => Ok(()),
                    }
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
            }
            E::SetSpeed { clip, num, den } => {
                let result = project::Speed::new(num, den).and_then(|speed| {
                    self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        d.apply(Command::SetSpeed {
                            sequence,
                            clip: project::ClipId(clip),
                            speed,
                        })
                    })
                });
                if let Err(err) = result {
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

    /// While playing, carries on from here with the project as it is now
    /// (after a change to the mix).
    fn play_on_with_new_mix(&mut self) {
        if let Some(playback) = self.playback.take() {
            let at = playback.position();
            drop(playback);
            self.viewer.playhead = at;
            self.start_playback();
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

    /// The keyboard shortcuts page, in its own window. Shortcuts changed
    /// there are in use at once and kept in settings.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if let Some(page) = self.shortcuts_page.as_mut() {
            let mut open = true;
            let mut asked = ui_kit::keyboard::PageResponse::default();
            egui::Window::new("Keyboard shortcuts")
                .open(&mut open)
                .collapsible(false)
                .default_pos([48.0, 64.0])
                .default_size([1040.0, 800.0])
                .show(ctx, |ui| {
                    asked = page.show(
                        ui,
                        &self.theme,
                        &mut self.shortcuts,
                        &mut self.settings.keyboard_layout,
                    );
                });
            if !open {
                self.shortcuts_page = None;
            }
            if asked.changed {
                self.settings.shortcuts = self.shortcuts.saved();
                if let Err(err) = self.settings.save(&self.data_dir) {
                    self.errors.push(&err);
                }
            }
            if asked.cheat_sheet {
                self.cheat_sheet
                    .start(ctx, &self.shortcuts, self.settings.keyboard_layout);
            }
        }
        if let Some(err) = self.export_page.poll() {
            self.errors.push(&err);
        }
        match self.cheat_sheet.poll(&self.jobs, ctx) {
            Some(crate::cheat_sheet::Saved::Done(path)) => {
                self.status = format!(
                    "Saved the cheat sheet as {}. Open it to print it.",
                    path.display()
                );
            }
            Some(crate::cheat_sheet::Saved::Failed(err)) => self.errors.push(&err),
            None => {}
        }
    }

    /// The Saved versions window, and what it asks for.
    fn versions_window(&mut self, ctx: &egui::Context) {
        use crate::versions_window::Asked;
        if !self.versions.open {
            return;
        }
        let Some(document) = &self.document else {
            self.versions.open = false;
            return;
        };
        let file = document.path().to_path_buf();
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        match self
            .versions
            .ui(ctx, &self.theme, &self.jobs, document.project(), &file, &undo)
        {
            Asked::Nothing => {}
            Asked::Saved(name) => self.status = format!("Saved the version \"{name}\"."),
            Asked::Restore(name, project) => {
                match self.with_document(|d| d.apply(Command::Restore { project })) {
                    Ok(()) => {
                        self.selected.clear();
                        self.fitted = false;
                        self.status = format!(
                            "Restored \"{name}\". {} returns to before.",
                            self.shortcuts.keys_or_name(Action::Undo)
                        );
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
            Asked::Failed(err) => self.errors.push(&err),
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
            waveforms.whole(jobs, ctx, m, stream)
        });
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .silence
            .ui(ctx, &self.theme, document.project(), ready, &undo);
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

    /// The viewer alone on the whole screen, or back to the page.
    fn set_full_screen(&mut self, on: bool) {
        self.full_screen = on && self.page == Page::Edit && self.document.is_some();
        self.full_screen_changed = true;
    }

    /// The viewer and its transport, filling the window.
    fn full_screen_viewer(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else { return };
        let p = document.project();
        let mut quality = self.settings.preview_quality;
        let height = ui.available_height() - 28.0;
        let click = self
            .viewer
            .ui(ui, &self.theme, p, &mut quality, None, height, &self.shortcuts);
        if let Some(click) = click {
            self.run_action(match click {
                ui_kit::TransportClick::FrameBack => Action::PreviousFrame,
                ui_kit::TransportClick::FrameForward => Action::NextFrame,
                ui_kit::TransportClick::PlayPause => Action::PlayPause,
                ui_kit::TransportClick::Stop => Action::Stop,
            });
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
        if let Some((media, placement)) = picked {
            use crate::media_drawer::Placement;
            let at = self.edit_point();
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(match placement {
                    Placement::End => Command::PlaceMedia { sequence, media },
                    Placement::Insert => Command::InsertMedia { sequence, media, at },
                    Placement::Overwrite => Command::OverwriteMedia { sequence, media, at },
                })
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

    /// The right-hand panel of the Edit page: the selected clip's settings.
    /// A finished change is applied as one undo step.
    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else { return };
        let commands = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.inspector
                    .ui(ui, &self.theme, document.project(), &self.selected)
            })
            .inner;
        for command in commands {
            let speed = matches!(command, Command::SetSpeed { .. });
            if let Err(err) = self.with_document(|d| d.apply(command)) {
                self.errors.push(&err);
            }
            if speed {
                // The sound follows the clip's new length.
                self.play_on_with_new_mix();
            }
        }
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
            search_keys: &self.shortcuts.keys_for(Action::Search),
            settings_keys: &self.shortcuts.keys_for(Action::OpenSettings),
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
        if self.page == Page::Export
            && let Some(document) = &self.document
        {
            let viewer = &self.viewer;
            let problem = self
                .export_page
                .ui(ui, &self.theme, document.project(), document.path(), || {
                    viewer.export_devices()
                });
            if let Some(err) = problem {
                self.errors.push(&err);
            }
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
            ui.label(egui::RichText::new(self.key_hints()).color(self.theme.muted));
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
        let mut master_gain = None;
        let mut timeline_events = Vec::new();
        let mut toolbar_click = None;
        let shortcut = self.shortcuts.keys_for(Action::Import);
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
        // A removed track's height is forgotten.
        self.timeline.heights.keep_only(&timeline_panel::track_ids(seq));
        if seq.length() == project::Time::ZERO {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nothing here yet").family(ui_kit::fonts::semibold()).size(18.0));
                ui.label(
                    egui::RichText::new(format!(
                        "Drop recordings on the window, or {} to import them. Each file goes at the end of the timeline, with every sound stream on its own track.",
                        if shortcut.is_empty() {
                            "click Import media".to_owned()
                        } else {
                            format!("press {shortcut}")
                        }
                    ))
                    .color(self.theme.muted),
                );
            });
        } else {
            let quality_before = self.settings.preview_quality;
            let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
            // The viewer and everything under it share this height; the
            // handle between them moves the boundary.
            let shared_height = ui.available_height();
            let (least_viewer, least_timeline) =
                (ui_kit::splitter::MIN_VIEWER, ui_kit::splitter::MIN_TIMELINE);
            let viewer_height =
                self.settings
                    .viewer_split
                    .top_height(shared_height, least_viewer, least_timeline);
            // A value being dragged in the inspector is seen at once.
            let being_changed = self.inspector.shown(p);
            transport_click = self.viewer.ui(
                ui,
                &self.theme,
                being_changed.as_ref().unwrap_or(p),
                &mut self.settings.preview_quality,
                needle,
                viewer_height,
                &self.shortcuts,
            );
            if self.settings.preview_quality != quality_before
                && let Err(err) = self.settings.save(&self.data_dir)
            {
                self.errors.push(&err);
            }
            let handle = ui_kit::splitter::splitter(
                ui,
                &self.theme,
                &mut self.settings.viewer_split,
                shared_height,
                least_viewer,
                least_timeline,
            );
            if handle.settled
                && let Err(err) = self.settings.save(&self.data_dir)
            {
                self.errors.push(&err);
            }
            let tip = |a: Action| self.shortcuts.tip(a);
            let tips = [
                tip(Action::SelectTool),
                tip(Action::BladeTool),
                tip(Action::RippleTool),
                tip(Action::RollTool),
                tip(Action::SlipTool),
                tip(Action::SlideTool),
                tip(Action::ToggleSnapping),
                tip(Action::ZoomToFit),
                tip(Action::RoughCut),
            ];
            toolbar_click = ui_kit::timeline::toolbar(
                ui,
                &self.theme,
                &self.timeline,
                [
                    &tips[0], &tips[1], &tips[2], &tips[3], &tips[4], &tips[5], &tips[6], &tips[7], &tips[8],
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
            let mut drawn = timeline_panel::drawn(
                p,
                seq,
                &self.selected,
                missing,
                |m, stream| waveforms.get(jobs, &ctx, m, stream),
                |m, stream| pictures.get(jobs, &ctx, m, stream),
            );
            self.timeline.rough_cut = self.settings.rough_cut;
            if self.settings.rough_cut {
                timeline_panel::fold_sound(&mut drawn);
            }
            let now = Instant::now();
            self.track_paces.time(&mut drawn.video, now);
            self.track_paces.time(&mut drawn.audio, now);
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
                cache: ui_kit::timeline::CacheBar {
                    ranges: &drawn.cache,
                    about: timeline_panel::CACHE_BAR_ABOUT,
                },
            };
            let status_height = if self.status.is_empty() { 0.0 } else { 24.0 };
            let height = (ui.available_height() - status_height).max(160.0);
            let size = egui::vec2(width - layout::MASTER_WIDTH - 8.0, height);
            let levels = self.playback.as_ref().map(|p| {
                let l = p.levels();
                ui_kit::master::MeterLevels {
                    peak_db: l.peak_db,
                    momentary: l.momentary,
                    short_term: l.short_term,
                }
            });
            let gain = seq.master_gain_db;
            ui.horizontal_top(|ui| {
                timeline_events = self.timeline.show(ui, &self.theme, &data, size);
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(layout::MASTER_WIDTH, height), egui::Sense::hover());
                master_gain = self.master.show(
                    ui,
                    &self.theme,
                    rect,
                    levels,
                    gain,
                    "The whole mix: drag to change its volume, double-click for 0 dB. M and S are its loudness over 0.4 s and 3 s (LUFS).",
                );
            });
        }
        if let Some(gain_db) = master_gain {
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::SetMasterGain { sequence, gain_db })
            });
            if let Err(err) = result {
                self.errors.push(&err);
            }
            self.play_on_with_new_mix();
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
                ui_kit::timeline::ToolbarClick::RoughCut => Action::RoughCut,
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
                ui.heading("Graphics");
                if let Some(err) = self.graphics.settings_row(ui, &self.theme) {
                    self.errors.push(&err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Keyboard shortcuts…").clicked() {
                        self.shortcuts_page.get_or_insert_with(Default::default);
                    }
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
            Some(Hit::Shortcuts) => self.run_action(Action::KeyboardShortcuts),
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
                // Wait for what the screenshot is of: the imports read and the
                // viewer's picture back. Not for ever, so a file that can't
                // be shown still gets its screenshot.
                let since = *self.probe.started.get_or_insert(now);
                let ready = (self.importer.busy() == 0 && self.viewer.settled())
                    || now.duration_since(since) > SCREENSHOT_WAIT;
                // A scripted pointer has done its part, and rested long
                // enough for a tooltip to come up.
                let pointed = self.pointer.is_none_or(|p| {
                    self.frames >= p.settled_at() && now.duration_since(since) > POINTER_REST
                });
                if self.frames >= 10 && ready && pointed && !self.probe.screenshot_requested {
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
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(pointer) = &self.pointer {
            pointer.apply(self.frames, &mut raw_input.events);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_input = now;
        }
        let idle = now.duration_since(self.last_input) > IDLE_AFTER;
        self.jobs.set_idle(idle);
        self.graphics.frame(&ctx, self.frames);
        if self.frames == 2 {
            for action in std::mem::take(&mut self.startup_actions) {
                self.run_action(action);
            }
        }
        // While the shortcuts page waits for new keys, what is pressed is
        // its answer, not a command.
        let listening = self.shortcuts_page.as_ref().is_some_and(|p| p.listening());
        if !listening {
            for action in self.shortcuts.pressed(&ctx) {
                self.run_action(action);
            }
        }
        self.imports(&ctx);
        if self.play_when_ready && self.importer.busy() == 0 && self.document.is_some() {
            self.play_when_ready = false;
            self.run_action(Action::PlayPause);
        }
        self.follow_playback(&ctx);
        self.waveforms.poll(&ctx, now);
        self.pictures.poll(&ctx, now);
        self.relinking(&ctx);
        self.frame_grabs(&ctx);
        if let Some(err) = self.viewer.poll(&ctx, self.settings.preview_quality)
            && !self.relink.explains_errors()
        {
            self.errors.push(&err);
        }
        if let Some(slow) = self.viewer.slow_pictures() {
            self.errors.set_fact(crate::slow_pictures::FACT, slow);
        }
        self.autosave(now, idle);
        if let Some(keeper) = &mut self.view_keeper {
            keeper.tick(now, &self.timeline, &self.jobs);
        }
        self.thumbnails();

        if std::mem::take(&mut self.full_screen_changed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.full_screen));
        }
        if self.full_screen {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ui, |ui| self.full_screen_viewer(ui));
        } else {
            egui::Panel::top("top-bar")
                .exact_size(layout::TOP_BAR_HEIGHT)
                .frame(egui::Frame::new().fill(self.theme.panel))
                .show(ui, |ui| self.top_bar(ui));
            if self.page == Page::Edit && self.document.is_some() {
                egui::Panel::right("inspector")
                    .exact_size(layout::RIGHT_PANEL_WIDTH)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(self.theme.panel)
                            .inner_margin(egui::Margin::symmetric(12, 10)),
                    )
                    .show(ui, |ui| self.inspector_panel(ui));
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(self.theme.bg)
                        .inner_margin(egui::Margin::same(16)),
                )
                .show(ui, |ui| self.page(ui));
        }

        self.settings_window(&ctx);
        self.timeline_settings_window(&ctx);
        self.search_window(&ctx);
        self.silence_window(&ctx);
        self.label_window(&ctx);
        self.versions_window(&ctx);
        self.shortcuts_window(&ctx);
        if let Some(err) = self.graphics.show_warning(&ctx, &self.theme) {
            self.errors.push(&err);
        }
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
        // Playback and the viewer's decoding stop first and are waited for,
        // briefly: closing while playing must not leave a thread inside the
        // sound card's, a decoder's or the graphics driver's code when the
        // window's graphics go away and the program ends.
        self.playback = None;
        // An export under way is given up: its worker decodes and draws on
        // the window's graphics device too.
        self.export_page.stop();
        self.viewer.close();
        if !engine::playback::wait_until_quiet(AUDIO_ENDS_WITHIN) {
            tracing::warn!("the sound did not stop before the window closed");
        }
        // Background reading nobody will look at any more stops here, so
        // closing never waits for a long file.
        self.waveforms.stop();
        self.pictures.stop();
        if let Some(keeper) = &mut self.view_keeper {
            keeper.close(&self.timeline);
        }
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

/// How long closing the app waits for the sound to let go of the sound
/// card and its files.
const AUDIO_ENDS_WITHIN: Duration = Duration::from_secs(2);

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
    /// Start on the Export page.
    pub export_page: bool,
    /// Start with the viewer full screen.
    pub full_screen: bool,
    /// Commands to run once open, by name.
    pub actions: Vec<String>,
    /// Convert the viewer's colour on the CPU even when the GPU could.
    pub cpu_colour: bool,
    /// Stop background reading at this share, for screenshots of it in progress.
    pub hold_reading: Option<f32>,
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
