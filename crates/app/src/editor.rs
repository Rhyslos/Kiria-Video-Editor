//! The editor window.

mod actions;
mod audio_ui;
mod clip_clipboard;
mod clip_job;
mod color_ui;
mod crop_bars;
mod edit_ui;
mod follow;
mod keyframe_clipboard;
mod library_ui;
mod live;
mod media_removal;
mod region_cuts;
mod saving;
mod script;
mod settings_ui;
mod steady;
mod timeline_events;
mod timeline_labels;
mod timelines;
mod undo_says;
mod unlinking;
mod voice_over;
mod windows;

use crate::import::{self, Importer, Read};
use crate::run_mode::RunMode;
use crate::settings::Settings;
use crate::timeline_panel;
use crate::viewer::Viewer;
use actions::toolbar_action;
use diag::AppError;
use jobs::{JobHandle, JobSystem, PoolKind, Priority, UsageMonitor};
use project::{Command, Document, LabelColor, LabelId, Recovery, Time, TimelineLabel};
use saving::{PendingSave, THUMBNAIL_ENDS_WITHIN, Thumbnail, thumbnail_settled};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ui_kit::{Action, ErrorCenter, ErrorResponse, Page, ShortcutTable, Theme, TopBar, layout};

/// How long without input before the app counts as idle (autosave runs, idle-only work starts).
const IDLE_AFTER: Duration = Duration::from_secs(5);

/// What the status line says when the lines are turned on and off.
const LINES_ON: &str =
    "Lines: click an audio track to add a point, drag one to move it, double-click it to remove it";
const LINES_OFF: &str = "Lines off: the pointer works clips again";

/// What the person is told when a fix they clicked has nothing behind it
/// (the command is logged, not shown).
fn unknown_fix(label: &str) -> diag::AppError {
    diag::AppError::new(format!("\"{label}\" couldn't be done"))
        .why("This version of the app has no way to do it yet. Nothing was changed.")
        .fix("Do it by hand for now; the app's log says which fix it was.")
}

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
    /// Where proxies are kept (the app's cache folder).
    pub proxy_dir: PathBuf,
    pub project: Option<ProjectToOpen>,
    pub mode: RunMode,
    pub report: Arc<Mutex<RunReport>>,
    /// Problems found before the window opened, shown once it has.
    pub problems: Vec<AppError>,
    /// What an automated run scripts (nothing in a normal run).
    pub automation: crate::run_mode::Automation,
}

pub struct Editor {
    /// A box being followed through its clip ("Follow it from here").
    following: Option<follow::FollowJob>,
    /// Black bars being looked for (Crop › Black bars).
    finding_bars: Option<crop_bars::FindingBars>,
    /// The Color page's Split: a viewer of the picture without its grade.
    color_before: Option<Viewer>,
    /// The Color page's own state.
    color_page: crate::color_page::ColorPage,
    /// A clip being analysed for its Stabilise effect.
    steadying: Option<steady::SteadyJob>,
    settings: Settings,
    data_dir: PathBuf,
    /// What draws the window: the first-frames check, the question after a
    /// failed start with Vulkan, and the Graphics row in Settings.
    graphics: graphics::WindowGraphics,
    theme: Theme,
    page: Page,
    shortcuts: ShortcutTable,
    errors: ErrorCenter,
    /// The first uncaught graphics error on the window's device, for the
    /// error list (`graphics_errors::watch`; the rest go to the log only).
    graphics_errors: Option<std::sync::mpsc::Receiver<AppError>>,
    jobs: JobSystem,
    usage: UsageMonitor,
    document: Option<Document>,
    /// Full saves handed to a worker: the journal entry each includes, and
    /// whether it was asked for (Ctrl+S) rather than an autosave.
    pending_saves: Vec<PendingSave>,
    /// Writes `settings.ron` off this thread, once a change has settled.
    settings_keeper: crate::settings::SettingsKeeper,
    /// The launcher's picture of the project, being written after a save.
    thumbnail: Option<JobHandle<bool>>,
    /// A save finished (or the project has no picture yet): write one.
    thumbnail_due: Thumbnail,
    /// What the launcher's picture was last written of, this session: a
    /// save that leaves it the same writes nothing.
    /// What the project's picture last written shows, and the journal's
    /// newest entry then: the same again needs no new picture.
    thumbnail_shows: Option<(engine::thumbnail::Shows, u64)>,
    importer: Importer,
    import_requested: bool,
    viewer: Viewer,
    /// Split view's Source and the viewer's own window (the Display menu).
    display: crate::display::ViewerDisplay,
    playback: Option<engine::Playback>,
    timeline: ui_kit::TimelineView,
    /// Keeps the timeline's layout (track heights, the divider between
    /// video and audio) in a small file beside the project.
    view_keeper: Option<crate::view_state::ViewKeeper>,
    /// Clips picked on the timeline (their linked clips go with them).
    selected: Vec<project::ClipId>,
    /// The tabs of the project's timelines, above the timeline.
    tabs: ui_kit::timeline_tabs::TimelineTabs,
    /// The tabs' contents, kept until a timeline changes.
    tabs_cache: timelines::TabsCache,
    /// The timeline shown last frame, to notice another one opening.
    shown_sequence: Option<project::SequenceId>,
    /// Where the playhead was in each timeline left this session.
    playheads: std::collections::HashMap<project::SequenceId, Time>,
    /// The timeline was fitted to the project once already. Cleared only
    /// by `refit`, with the Audio page's own.
    fitted: bool,
    waveforms: crate::waveforms::Waveforms,
    pictures: crate::clip_pictures::ClipPictures,
    /// Proxies for the project's video files, built in the background.
    proxies: crate::proxies::ProxyBuilds,
    /// The render cache and the pre-render under way.
    prerendering: crate::prerender::Prerendering,
    /// How fast each track's waveforms or pictures are being read, for the time left.
    track_paces: timeline_panel::TrackPaces,
    relink: crate::relink::Relinker,
    /// Label names and colours offered in every project, and their saving.
    global_labels: crate::global_labels::Everywhere,
    /// New recordings in the media folders, watched on a thread of their own.
    media_folders: crate::media_folders::MediaFolders,
    /// The project's media in the library index, on a thread of its own:
    /// what the drawer's "Find a file" and the search palette ask.
    library: library::Live,
    drawer: crate::media_drawer::MediaDrawer,
    /// The Library page: its tabs, filters, the file in its panel and its player.
    library_page: crate::library_page::LibraryPage,
    /// The page shown last frame, to notice it being left.
    shown_page: Page,
    /// The selected clip's settings, on the right of the Edit page.
    inspector: crate::inspector::Inspector,
    /// The handles on the selected clip's picture in the viewer.
    picture_handles: crate::picture_handles::PictureHandles,
    /// The Export page: what to export, and the export under way.
    export_page: crate::export_page::ExportPage,
    /// The question before closing while exports are unfinished.
    close_prompt: crate::close_prompt::ClosePrompt,
    /// The Audio page: its tool, track, filter, mixer and counting.
    audio_page: crate::audio_page::AudioPage,
    /// The master's meter as last read while playing, shown after.
    audio_held_master: Option<audio::dsp::loudness::Measured>,
    /// The status line has said that this playback is unheard (no sound
    /// output), once per playback.
    unheard_said: bool,
    /// Volume and pan lines are drawn with their points and worked by the
    /// pointer over audio tracks (the lines tool, on every timeline).
    lines_on: bool,
    /// Which lines: volume or pan, on tracks or on clips.
    lines: ui_kit::LinesShown,
    frame_grab: crate::frame_grab::FrameGrab,
    silence: crate::silence_panel::SilencePanel,
    /// Sync by sound: what it found, and the comparison running.
    sync: crate::sync_panel::SyncPanel,
    label_editor: Option<crate::label_editor::LabelEditor>,
    /// The timeline label chosen by a click: Delete removes it.
    selected_label: Option<LabelId>,
    master: ui_kit::master::MasterColumn,
    /// The columns of the tracks pinned beside the master, by track.
    pinned_columns: std::collections::HashMap<u64, ui_kit::track_column::TrackColumn>,
    /// Effects copied from a track (its menu's "Copy its effects"), to paste on another.
    track_effects: Option<project::TrackSound>,
    versions: crate::versions_window::VersionsWindow,
    templates: crate::templates_window::TemplatesWindow,
    /// Prune: the files no timeline uses, to take out of the project.
    prune: crate::prune_window::PruneWindow,
    /// "Remove from project" asked for files a timeline uses: the warning.
    removal: Option<media_removal::RemovalAsk>,
    /// Package project: the project and its media, copied into one folder.
    package: crate::package_window::PackageWindow,
    /// Package project was asked for; the folder dialog opens on the next frame.
    package_requested: bool,
    shortcuts_page: Option<ui_kit::keyboard::ShortcutsPage>,
    /// The system's keyboard layout, followed for the shortcuts page and
    /// for writing keys in menus and tips.
    system_keyboard: crate::system_keyboard::Follower,
    cheat_sheet: crate::cheat_sheet::CheatSheet,
    /// The user guide in the web browser (F1, Settings › Help, search).
    user_guide: ui_kit::user_guide::UserGuide,
    /// The user guide was asked for; it opens on the next frame.
    user_guide_requested: bool,
    /// Keyframe marks copied (Copy), to paste on a picture clip.
    copied_keyframes: Option<project::CopiedKeyframes>,
    /// Clips copied (Copy or Cut with clips selected), to paste at the
    /// edit point; only one of these and `copied_keyframes` is kept.
    copied_clips: Option<project::CopiedClips>,
    /// Text for the system clipboard, put there on the next frame.
    clipboard_text: Option<String>,
    /// The viewer fills the screen (F; Escape returns).
    full_screen: bool,
    /// Tell the window about a full-screen change on the next frame.
    full_screen_changed: bool,
    /// Notices when the window stops drawing, and logs the step it was on.
    heartbeat: crate::heartbeat::Heartbeat,
    /// A voice-over being recorded, from opening the microphone to placing
    /// the take (`editor::voice_over`).
    voice_over: Option<crate::voice_over::Take>,
    /// The banner's microphone level, falling back slowly.
    voice_over_peak: f32,
    /// The microphones the system offers, for Settings › Audio.
    microphones: crate::voice_over::Microphones,
    /// Times frames while playing, for hitches seen on someone's computer.
    playback_watch: crate::playback_watch::Watch,
    /// When the window was last asked to fill the screen or come back, until
    /// the first frame after it has been drawn (for the log).
    full_screen_asked: Option<Instant>,
    /// Save this frame was asked for; the dialog opens on the next frame.
    save_frame_requested: bool,
    show_timeline_settings: bool,
    show_settings: bool,
    /// The Settings window's tab, search and row to jump to.
    settings_view: ui_kit::settings::SettingsView,
    /// What this machine has (the limits of Settings › Resources), read at start.
    machine: jobs::Machine,
    /// Settings › Cache: what the cache takes, and clearing it.
    cache_cleanup: crate::cache_cleanup::CacheCleanup,
    /// The Settings window was open last frame (the cache is measured
    /// when it opens).
    settings_were_open: bool,
    show_diagnostics: bool,
    show_search: bool,
    search_text: String,
    /// The proxy changes Settings › Cache was last measured at.
    cache_seen: u64,
    status: String,
    /// Where the playhead was before a clip began to be slipped: the viewer
    /// shows the clip's first frame while it is, then the playhead goes back.
    slip_playhead: Option<Time>,
    /// The status line's age: it fades out a few seconds after it last changed.
    status_fade: crate::status_line::Fading,
    /// Whether the status line's message is about the timeline.
    status_about_timeline: crate::status_line::AboutTimeline,
    /// How strongly the status line is drawn this frame (1 full).
    status_strength: f32,
    playing: bool,
    /// Playback loops inside the region (L).
    looping: bool,
    last_input: Instant,
    mode: RunMode,
    /// What an automated run still has to do by itself (`script::step`).
    script: script::Script,
    frames: u64,
    probe: Probe,
    report: Arc<Mutex<RunReport>>,
}

/// How long each frame may spend adding imported files to the project.
const IMPORT_BUDGET: Duration = Duration::from_millis(2);

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
            proxy_dir,
            project,
            mode,
            report,
            problems,
            automation,
        } = start;
        // Most of what is scripted acts on the project: without one, only
        // what is about the window.
        let automation = automation.for_project(project.is_some());
        let mut settings = settings;
        script::settings(&mut settings, &automation.audio);
        ui_kit::setup(&cc.egui_ctx, settings.theme);
        let machine = jobs::Machine::detect();
        let jobs = JobSystem::new(settings.resources.resolve(&machine));
        let mut errors = ErrorCenter::default();
        for problem in &problems {
            errors.push(problem);
        }
        let graphics = graphics::WindowGraphics::new(cc, opened, &mut errors);
        let imports = project.as_ref().map(|p| p.import.clone()).unwrap_or_default();
        let mut library_page = crate::library_page::LibraryPage::selecting(automation.library_select.clone());
        let holds = automation.holds;
        let convert = crate::video_picture::conversion(cc.wgpu_render_state.as_ref(), automation.cpu_colour);
        let decoding =
            crate::device_decoding::WindowDecoding::new(&graphics, cc.wgpu_render_state.as_ref(), convert);
        if let Some(decoding) = &decoding {
            decoding.set_frames_kept(settings.resources.card_frames_mb);
            decoding.set_frames_in_memory(jobs.budgets().ram_cache_mb);
        }
        // The usual shortcuts, with the ones changed on the shortcuts page.
        let mut shortcuts = ShortcutTable::restored(&settings.shortcuts);
        let system_keyboard = crate::system_keyboard::Follower::start();
        shortcuts.set_layout(settings.keyboard.resolve(system_keyboard.now()));
        let mut unlinked_on_open = false;
        let document = project.and_then(|p| match open_or_create(&p.path, p.recovery) {
            Ok(mut doc) => {
                // Settings' "Always unlink audio and video": what is linked
                // in the project is unlinked as it opens (one undo step).
                match doc.set_always_unlinked(settings.always_unlinked) {
                    Ok(unlinked) => unlinked_on_open = unlinked,
                    Err(err) => errors.push(&err),
                }
                let undo = shortcuts.keys_or_name(Action::Undo);
                if let Some(notice) = crate::recovered::notice(doc.opened(), &undo) {
                    errors.push(&notice);
                }
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
        // The timeline that was open last time (the first one when the
        // layout doesn't say, or it is gone).
        let mut document = document;
        if let Some(d) = document.as_mut() {
            let first = d.project().sequences.front().map(|s| s.id);
            let kept = timeline.showing.map(project::SequenceId);
            if let Some(open) = kept.filter(|id| d.project().sequence(*id).is_some()).or(first) {
                d.open_sequence(open);
            }
        }
        let mut importer = Importer::default();
        importer.set_automatic(
            document
                .as_ref()
                .and_then(|d| d.project().active())
                .is_some_and(|s| s.automatic),
        );
        if !imports.is_empty() {
            importer.read(&jobs, &cc.egui_ctx, imports);
        }
        let graphics_errors = cc
            .wgpu_render_state
            .as_ref()
            .map(|state| crate::graphics_errors::watch(&state.device));
        let mut viewer = Viewer::new(
            &cc.egui_ctx,
            automation.playhead,
            convert,
            decoding,
            cc.wgpu_render_state
                .as_ref()
                .map(|state| (state.device.clone(), state.queue.clone())),
        );
        let mut pictures = crate::clip_pictures::ClipPictures::holding_at(holds.reading);
        pictures.set_exact(settings.clip_pictures == crate::settings::ClipPictureWay::Exact);
        // Waveforms are kept beside the proxies, in the app's cache folder.
        let waveforms = crate::waveforms::Waveforms::new(
            proxy_dir.parent().map(|cache| cache.join("waveforms")),
            holds.reading,
        );
        waveforms.trim(&jobs);
        // Files' loudness readings and clip pictures too.
        if let Some(cache) = proxy_dir.parent() {
            library_page.keep_loudness_in(cache.join("loudness"));
            pictures.keep_in(cache.join("pictures"), &jobs);
        }
        let prerendering = crate::prerender::Prerendering::new(
            proxy_dir
                .parent()
                .map_or_else(|| data_dir.join("renders"), |cache| cache.join("renders")),
            &jobs,
            cc.egui_ctx.clone(),
        );
        let proxies = crate::proxies::ProxyBuilds::new(
            proxy_dir,
            settings.proxies,
            jobs.budgets().disk_cache_gb,
            holds.proxy,
        );
        // Before the split view's Source is made: it takes the shelf from here.
        viewer.set_proxies(Some(proxies.shelf()));
        viewer.set_renders(Some(prerendering.store()));
        viewer.set_key_frame_first(settings.jump_picture.key_frame_first());
        let display = crate::display::ViewerDisplay::new(settings.split_view, &viewer);
        let global_labels = crate::global_labels::Everywhere::load(&data_dir, &mut errors);
        let export_page = crate::export_page::ExportPage::new(&data_dir, settings.export_pace, &mut errors);
        let settings_keeper = crate::settings::SettingsKeeper::new(&data_dir, &settings, Instant::now());
        let mut editor = Editor {
            theme: settings.theme.theme(),
            settings,
            data_dir,
            graphics,
            page: Page::default(),
            shortcuts,
            errors,
            graphics_errors,
            jobs,
            usage: UsageMonitor::start(Duration::from_secs(1)),
            document,
            pending_saves: Vec::new(),
            settings_keeper,
            thumbnail: None,
            thumbnail_due: Thumbnail::IfMissing,
            thumbnail_shows: None,
            importer,
            import_requested: false,
            viewer,
            display,
            playback: None,
            timeline,
            view_keeper,
            selected: Vec::new(),
            tabs: Default::default(),
            tabs_cache: Default::default(),
            shown_sequence: None,
            playheads: Default::default(),
            fitted: false,
            waveforms,
            pictures,
            proxies,
            prerendering,
            track_paces: timeline_panel::TrackPaces::default(),
            relink: crate::relink::Relinker::default(),
            frame_grab: crate::frame_grab::FrameGrab::default(),
            silence: crate::silence_panel::SilencePanel::default(),
            sync: crate::sync_panel::SyncPanel::default(),
            label_editor: None,
            selected_label: None,
            master: ui_kit::master::MasterColumn::default(),
            pinned_columns: Default::default(),
            track_effects: None,
            versions: crate::versions_window::VersionsWindow::default(),
            templates: crate::templates_window::TemplatesWindow::default(),
            prune: crate::prune_window::PruneWindow::default(),
            removal: None,
            package: crate::package_window::PackageWindow::holding_at(holds.package),
            package_requested: false,
            shortcuts_page: None,
            system_keyboard,
            cheat_sheet: crate::cheat_sheet::CheatSheet::default(),
            user_guide: ui_kit::user_guide::UserGuide::default(),
            user_guide_requested: false,
            copied_keyframes: None,
            copied_clips: None,
            clipboard_text: None,
            full_screen: false,
            full_screen_changed: false,
            heartbeat: crate::heartbeat::Heartbeat::start(),
            voice_over: None,
            voice_over_peak: 0.0,
            microphones: Default::default(),
            playback_watch: crate::playback_watch::Watch::default(),
            full_screen_asked: None,
            following: None,
            finding_bars: None,
            color_before: None,
            color_page: Default::default(),
            steadying: None,
            save_frame_requested: false,
            global_labels,
            media_folders: crate::media_folders::MediaFolders::new(automation.watch_folders.clone()),
            library: {
                let ctx = cc.egui_ctx.clone();
                library::Live::start(move || ctx.request_repaint())
            },
            drawer: Default::default(),
            library_page,
            shown_page: Page::default(),
            inspector: crate::inspector::Inspector::default(),
            picture_handles: crate::picture_handles::PictureHandles::default(),
            export_page,
            close_prompt: Default::default(),
            audio_page: Default::default(),
            audio_held_master: None,
            unheard_said: false,
            lines_on: false,
            lines: ui_kit::LinesShown::default(),
            show_timeline_settings: false,
            show_settings: false,
            settings_view: Default::default(),
            machine,
            cache_cleanup: Default::default(),
            settings_were_open: false,
            show_diagnostics: false,
            show_search: false,
            search_text: String::new(),
            cache_seen: 0,
            status: match unlinked_on_open {
                true => crate::editor::unlinking::UNLINKED_ON_OPEN.into(),
                false => String::new(),
            },
            slip_playhead: None,
            status_fade: Default::default(),
            status_about_timeline: Default::default(),
            status_strength: 1.0,
            playing: false,
            looping: false,
            last_input: Instant::now(),
            mode,
            script: script::Script::new(&automation),
            frames: 0,
            probe: Probe::default(),
            report,
        };
        script::arrange(&mut editor, &automation);
        editor
    }

    /// The timeline the page shown has: the Audio page's own, or the Edit page's.
    fn shown_view(&mut self) -> &mut ui_kit::TimelineView {
        view_on(self.page, &mut self.timeline, &mut self.audio_page.timeline)
    }

    /// Show the whole timeline again on every page: the Edit page's view
    /// (which the other pages share) and the Audio page's own are fitted to
    /// the timeline's length the next time each is drawn. Everything that
    /// changes what a timeline is as a whole (another one opened, files
    /// added, a version restored, quiet stretches removed) calls this.
    fn refit(&mut self) {
        self.fitted = false;
        self.audio_page.fitted = false;
    }

    fn with_document(&mut self, f: impl FnOnce(&mut Document) -> diag::Result<()>) -> diag::Result<()> {
        match self.document.as_mut() {
            Some(document) => f(document),
            None => Err(AppError::new("No project is open")
                .why("This needs a project.")
                .fix("Open a project from the launcher.")),
        }
    }

    /// Splits at `at`: the first selected clip (with its linked clips), or
    /// every clip there when none is selected.
    fn split_at(&mut self, at: Time) -> diag::Result<()> {
        let clip = self.selected.first().copied();
        self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::Split { sequence, at, clip })
        })
    }

    /// What is ready to play on `seq`, for the cache bar: proxies, and the
    /// frames kept around the picture on show.
    fn playback_ready(&self, seq: &project::Sequence) -> Vec<ui_kit::timeline::CacheRange> {
        let mut ranges =
            timeline_panel::playback_ready(seq, self.viewer.kept(), |m| self.proxies.cache_state(m));
        ranges.extend(self.prerendering.ranges(seq.id));
        ranges
    }

    /// Pre-renders the region (its right-click menu, search): its frames
    /// put together ahead of time into the render cache.
    pub(crate) fn prerender_region(&mut self) {
        let Some(region) = self.region() else {
            self.status = "Mark a region first: Shift-drag in the ruler, or I and O.".into();
            return;
        };
        let Some(devices) = self.viewer.export_devices() else {
            self.status =
                "Pre-rendering needs the graphics card the window is drawn with, and there is none here."
                    .into();
            return;
        };
        let Some(project) = self.document.as_ref().map(|d| d.project().clone()) else {
            return;
        };
        let Some(sequence) = project.active().map(|s| s.id) else {
            return;
        };
        self.prerendering.stop();
        self.prerendering
            .start(&project, sequence, (region.start, region.end), devices);
        self.status = "Pre-rendering the region…".into();
    }

    /// Looks how the pre-render goes, and what is kept.
    fn poll_prerender(&mut self) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let project = document.project();
        let Some(sequence) = project.active().map(|s| s.id) else {
            return;
        };
        let version = (document.newest_entry(), sequence);
        match self.prerendering.poll(version, project, &self.jobs) {
            Some(Ok(told)) => self.status = told,
            Some(Err(err)) => self.errors.push(&err),
            None => {
                if let Some(progress) = self.prerendering.progress_text() {
                    self.status = progress;
                }
            }
        }
    }

    /// The playhead, on the open timeline's frame grid.
    fn playhead_on_frame(&self) -> Time {
        let seq = self.document.as_ref().and_then(|d| d.project().active());
        seq.map_or(self.viewer.playhead, |s| {
            s.settings.frame_rate.snap(self.viewer.playhead)
        })
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

    /// Adds a timeline label at `at`, named by its number
    /// (`Sequence::next_label_name`, never a name in use).
    fn add_label(&mut self, at: Time) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let (sequence, name) = (seq.id, seq.next_label_name());
            d.apply(Command::AddLabel {
                sequence,
                label: TimelineLabel {
                    id: LabelId(0),
                    name,
                    at: seq.settings.frame_rate.snap(at),
                    color: LabelColor::Blue,
                    until: None,
                },
            })
        })
    }

    /// Adds an empty track after the others of its kind, named by its number.
    fn add_track(&mut self, kind: project::TrackKind) -> diag::Result<()> {
        self.shown_view().reveal_added_track();
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
    fn set_region(&mut self, range: Option<(Time, Time)>) -> diag::Result<()> {
        self.with_document(|d| {
            let Some(seq) = d.project().active() else {
                return Ok(());
            };
            let region = region_on_frames(seq.settings.frame_rate, range);
            if region == seq.region {
                return Ok(());
            }
            let sequence = seq.id;
            d.apply(Command::SetRegion { sequence, region })
        })
    }

    /// Mark in (`start`) or out at the playhead: the region's start or end
    /// there, its other end kept when it is on the right side, else the
    /// timeline's start or end.
    fn mark(&mut self, start: bool) -> diag::Result<()> {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return Ok(());
        };
        let region = marked(seq.region, seq.length(), self.viewer.playhead, start);
        self.set_region(Some(region))
    }

    /// Moves the playhead to `at` seconds (as the timeline widget gives
    /// them); while playing, playback carries on from there.
    fn seek(&mut self, at: f64) {
        self.go_to(seconds(at));
    }

    /// Moves the playhead to the next (or previous) cut between pictures.
    /// On the Color page the picture there is selected, so it is graded.
    fn jump_to_cut(&mut self, next: bool) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active().cloned()) else {
            return;
        };
        let Some(at) = cut_beside(&seq, self.viewer.playhead, next) else {
            self.status = "No cut that way: this is the end of the timeline".into();
            return;
        };
        self.go_to(at);
        if self.page == Page::Color {
            self.selected = crate::color_page::graded_clip(&seq, &[], at)
                .into_iter()
                .collect();
        }
    }

    /// Moves the playhead to the selected clip's next (or previous)
    /// keyframe of any of its numbers, or says why it can't.
    fn jump_to_keyframe(&mut self, next: bool) {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return;
        };
        let Some(seq) = project.active() else { return };
        let rate = seq.settings.frame_rate;
        let here = rate.frame_at(self.viewer.playhead);
        let clip = seq
            .video_tracks
            .iter()
            .rev()
            .flat_map(|track| track.clips.iter())
            .find(|clip| self.selected.contains(&clip.id));
        let Some(clip) = clip else {
            self.status =
                "Select a clip with a picture first: its keyframes are what this moves between.".into();
            return;
        };
        // Its look's keyframes and its speed ramp's, as the marks show them.
        let frames = timeline_panel::all_keyframe_frames(clip, rate);
        let target = match next {
            true => frames.iter().find(|frame| **frame > here),
            false => frames.iter().rev().find(|frame| **frame < here),
        };
        match target {
            Some(frame) => {
                let at = rate.frame_start(*frame);
                self.go_to(at);
            }
            None => {
                let way = if next { "after" } else { "before" };
                self.status = format!(
                    "The selected clip has no keyframe (of its look or its speed) {way} the playhead."
                );
            }
        }
    }

    /// Moves the playhead to `at` (on a frame, within the timeline), going
    /// on playing from there if it was.
    fn go_to(&mut self, at: Time) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return;
        };
        let (rate, length) = (seq.settings.frame_rate, seq.length());
        self.viewer.go_to(at, rate, length);
        self.play_on_from_playhead();
        self.after_play_change();
    }

    /// While playing, carries on with the project as it is now (after a
    /// change to a clip's sound, speed or length): what was mixed ahead of
    /// the tracks that changed is mixed again (`Playback::refeed`).
    fn play_on_with_new_mix(&mut self) {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return;
        };
        // What was mixed ahead is mixed again: heard within a few blocks,
        // with no gap.
        if let Some(playback) = &mut self.playback {
            playback.refeed(project);
        }
    }

    /// Where playing from the playhead starts: the start again from the
    /// end, inside the region when looping. `None` for an empty timeline.
    fn play_from(&self) -> Option<(project::SequenceId, Time)> {
        let seq = self.document.as_ref()?.project().active()?;
        let length = seq.length();
        if length == Time::ZERO {
            return None;
        }
        // From the end, play from the start again.
        let last = (length - seq.settings.frame_rate.frame_duration()).max(Time::ZERO);
        let from = if self.viewer.playhead >= last {
            Time::ZERO
        } else {
            self.viewer.playhead
        };
        Some((seq.id, crate::looping::start_at(from, seq.region, self.looping)))
    }

    fn start_playback(&mut self) {
        let Some((sequence, from)) = self.play_from() else {
            return;
        };
        let Some(document) = &self.document else { return };
        self.viewer.playhead = from;
        self.unheard_said = false;
        self.playback = Some(engine::Playback::start(
            document.project().clone(),
            sequence,
            from,
        ));
    }

    /// While playing, goes on from the playhead (a jump: the sound card
    /// stays open); otherwise nothing.
    fn play_on_from_playhead(&mut self) {
        let Some((sequence, from)) = self.play_from() else {
            self.playback = None;
            return;
        };
        match &mut self.playback {
            Some(playback) if playback.sequence() == sequence => {
                self.viewer.playhead = from;
                playback.jump(from);
            }
            Some(_) => self.start_playback(),
            None => {}
        }
    }

    fn after_play_change(&mut self) {
        self.playing = self.playback.is_some();
        self.jobs.set_playing(self.playing);
        let interval = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .map(|s| 1.0 / s.settings.frame_rate.as_f64());
        let interval = if self.playing { interval } else { None };
        self.viewer.set_playing(interval);
        self.display.set_playing(interval);
    }

    /// `at` on a frame of the open timeline.
    fn snapped(&self, at: Time) -> Time {
        self.document
            .as_ref()
            .and_then(|d| d.project().active())
            .map_or(at, |s| s.settings.frame_rate.snap(at))
    }

    /// While playing: the playhead follows the clock, and playback stops at the end.
    /// Times this frame for the playback watch, writing hitches and the
    /// summary of a play that ended to the log, and the summary to
    /// Diagnostics.
    fn watch_playback(&mut self) {
        let frame = crate::playback_watch::Frame {
            playing: self.playback.is_some(),
            at: self.viewer.playhead,
            picture: self.viewer.take_arrived(),
        };
        let said = self.playback_watch.frame_ended(Instant::now(), frame);
        for line in said.log {
            tracing::info!("{line}");
        }
        if let Some(summary) = said.summary {
            self.errors.set_fact(crate::playback_watch::FACT, summary);
        }
    }

    fn follow_playback(&mut self, ctx: &egui::Context) {
        if self.playback.as_ref().is_some_and(engine::Playback::unheard) && !self.unheard_said {
            self.unheard_said = true;
            self.status = format!(
                "{}: the mix is measured as it plays, but there is no sound device to hear it on.",
                ui_kit::master::UNHEARD
            );
        }
        // Mute, solo, volume and sound changes (and their undo) are heard
        // within a block: a comparison per frame, sent only when changed.
        // Whatever is being dragged, anywhere in the window, is heard as it
        // moves (`live_project`).
        let followed = self
            .document
            .as_ref()
            .filter(|_| self.playback.is_some())
            .map(|document| {
                let p = document.project();
                let live = self.live_project(p);
                let live_p = live.as_ref().unwrap_or(p);
                // The track a voice-over is being recorded on is muted while it plays.
                self.voice_over
                    .as_ref()
                    .and_then(|take| {
                        crate::voice_over::muted_while_recording(live_p, take.sequence, take.track)
                    })
                    .or(live)
            });
        let Some(playback) = &mut self.playback else {
            return;
        };
        if let Some(document) = &self.document {
            playback.follow(followed.flatten().as_ref().unwrap_or(document.project()));
        }
        let seq = self.document.as_ref().and_then(|d| d.project().active());
        let (length, rate) = seq.map_or((Time::ZERO, project::Rate::FPS_60), |s| {
            (s.length(), s.settings.frame_rate)
        });
        let at = playback.position();
        let region = seq.and_then(|s| s.region);
        // A voice-over plays on to the end: no looping while recording.
        let looping = self.looping && self.voice_over.is_none();
        if let Some(back) = crate::looping::wrap(at, region, looping, length) {
            // Back to the region's start: a jump, the sound card stays open.
            playback.jump(back);
            self.viewer.playhead = back;
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
        // Several files going onto an automatic timeline wait for each
        // other, so its settings come from what most of them share.
        let automatic = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .is_some_and(|s| s.automatic);
        self.importer.set_automatic(automatic);
        if std::mem::take(&mut self.import_requested) {
            // The Library page fills the project's media; the Edit page's
            // import also puts each file at the end of the timeline.
            self.importer.choose_files(ctx, self.page != Page::Library);
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
                Ok(()) if self.page == Page::Library => self.importer.add(&self.jobs, ctx, dropped),
                Ok(()) => self.importer.read(&self.jobs, ctx, dropped),
                Err(err) => self.errors.push(&err),
            }
        }
        self.importer.poll_dialog(&self.jobs, ctx);
        // Each file added writes the autosave journal, so only as many as
        // fit in a few milliseconds go in each frame (at least one).
        let started = Instant::now();
        while started.elapsed() < IMPORT_BUDGET {
            let Some((read, mut batch)) = self.importer.next_read() else {
                break;
            };
            if self.importer.busy() > 0 {
                ctx.request_repaint();
            }
            let result = match read {
                Read::Ready {
                    media, place: false, ..
                } => {
                    let line = import::summary(&media);
                    let end_missing = media::end_missing_words(&media)
                        .map_or_else(String::new, |words| format!(". {words}"));
                    self.with_document(|d| d.apply(Command::AddMedia { media }))
                        .map(|()| self.status = format!("Added {line} to the project's media{end_missing}"))
                }
                Read::Ready {
                    media,
                    place: true,
                    settle,
                } => {
                    let line = import::summary(&media);
                    let end_missing = media::end_missing_words(&media);
                    let mut settled = None;
                    self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        let was_automatic = d.project().active().is_some_and(|s| s.automatic);
                        d.apply(match settle {
                            // Several files at once: what most of them share.
                            Some(settings) if was_automatic => Command::ImportSettled {
                                sequence,
                                media,
                                settings,
                            },
                            _ => Command::Import { sequence, media },
                        })?;
                        if was_automatic && let Some(s) = d.project().active().filter(|s| !s.automatic) {
                            settled = Some(s.settings);
                        }
                        Ok(())
                    })
                    .map(|()| {
                        // Show the whole timeline again with the new file in it.
                        self.refit();
                        let matching = if settle.is_some() {
                            "what most of the videos imported with it share"
                        } else {
                            "it"
                        };
                        let settled = settled.map(|s| {
                            format!(
                                "The timeline is now {} at {} kHz to match {matching} (Timeline settings to change).",
                                import::settings_text(&s),
                                s.sample_rate as f32 / 1000.0
                            )
                        });
                        self.status = match &settled {
                            Some(said) => format!("Imported {line}. {said}"),
                            None => format!("Imported {line}"),
                        };
                        // Said again when the batch is summed up.
                        self.importer.note_settled(batch.as_mut(), settled);
                        if let Some(words) = end_missing {
                            self.status = format!("{}. {words}", self.status.trim_end_matches('.'));
                        }
                    })
                }
                Read::Failed {
                    path,
                    refused,
                    alone,
                    place,
                } => {
                    tracing::info!(path = %path.display(), "import refused: {}", refused.error.report());
                    match (alone, refused.refusal) {
                        // The person chooses: what is there, or skip it.
                        (true, media::Refusal::CutShort) => {
                            let id = self.importer.set_aside(vec![path], place);
                            Err(refused
                                .error
                                .action(import::whats_there_action("Import what's there", id)))
                        }
                        (true, _) => Err(refused.error),
                        (false, _) => Ok(()),
                    }
                }
            };
            if let Err(err) = result {
                self.errors.push(&err);
            }
            // Files picked or dropped together: one line for them all, and
            // one message for those refused.
            if let Some(batch) = batch
                && let Some(line) = batch.line()
            {
                self.status = line;
                let whats_there = (!batch.cut_short.is_empty())
                    .then(|| self.importer.set_aside(batch.cut_short.clone(), batch.place));
                if let Some(err) = batch.summary(whats_there) {
                    self.errors.push(&err);
                }
            }
        }
    }

    /// The recovery notice's "Open last save": the project as it was last
    /// saved before the crash, put back in one step that undo takes back.
    fn open_last_save(&mut self) {
        let last_save = match self.document.as_ref().map(Document::opened) {
            Some(project::Opened::Recovered { last_save, .. }) => last_save.clone(),
            _ => return,
        };
        let result = self.with_document(|d| d.apply(Command::Restore { project: last_save }));
        match result {
            Ok(()) => {
                self.status = format!(
                    "Opened the last save. {} puts the recovered changes back.",
                    self.shortcuts.keys_or_name(Action::Undo)
                );
            }
            Err(err) => self.errors.push(&err),
        }
    }

    /// Work that goes on behind every page, followed each frame: the
    /// export queue (its errors, and how fast it went kept in settings),
    /// the cheat sheet being written, and the user guide asked for and
    /// being opened. Kept out of the windows so that none of them, shut
    /// or returning early, stops it.
    fn follow_background(&mut self, ctx: &egui::Context) {
        for err in self.export_page.poll() {
            self.errors.push(&err);
        }
        if let Some(pace) = self.export_page.take_pace() {
            self.settings.export_pace = pace;
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
        if std::mem::take(&mut self.user_guide_requested) {
            match self.user_guide.open(ctx) {
                Ok(_) => self.status = "Opening the user guide in your web browser".into(),
                Err(err) => self.errors.push(&err),
            }
        }
        if let Some(err) = self.user_guide.poll() {
            self.errors.push(&err);
        }
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
            let devices = self.viewer.export_devices();
            self.frame_grab.start(ctx, document.project(), at, name, devices);
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

    /// The viewer alone on the whole screen, or back to the page.
    fn set_full_screen(&mut self, on: bool) {
        self.full_screen = on && self.page == Page::Edit && self.document.is_some();
        self.full_screen_changed = true;
    }

    /// What the Display menu shows as on: the viewer's ways of showing the
    /// picture and the timeline's ways of looking at its tracks.
    fn display_state(&self, full_screen: bool) -> ui_kit::viewer::Display {
        ui_kit::viewer::Display {
            compact_tracks: self.settings.compact_tracks,
            track_divider: !self.settings.one_track_list,
            click_moves_needle: self.settings.click_moves == ui_kit::ClickMoves::EditNeedle,
            zoom_to_playhead: self.settings.zoom_to_playhead,
            track_knobs: self.settings.track_knobs,
            detailed_gain: self.settings.detailed_gain,
            header_volume: !self.settings.hide_header_volume,
            rough_cut: self.settings.rough_cut,
            edit_at_playhead: self.settings.edit_at_playhead,
            clean: self.settings.clean_picture,
            ..self.display.state(full_screen)
        }
    }

    /// The viewer into its own window (for another screen), or back.
    /// Where the window was is kept in settings.
    fn switch_viewer_window(&mut self) -> diag::Result<()> {
        if self.display.window_open() {
            self.settings.viewer_window = self.display.close_window().or(self.settings.viewer_window);
            self.status = "The viewer is back in the window".into();
        } else {
            if self.full_screen {
                self.set_full_screen(false);
            }
            self.display
                .open_window(self.settings.viewer_window, self.theme, &self.shortcuts);
            self.status = format!(
                "The viewer is in its own window: drag it to another screen; {} or a double-click fills that screen",
                self.shortcuts.keys_or_name(Action::FullScreenViewer)
            );
        }
        Ok(())
    }

    /// Keeps the library index in step with the project and asks it what
    /// the drawer and the search palette show. Never waits for it.
    fn library_index(&mut self, ctx: &egui::Context) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let project = document.project();
        self.library.follow(project);
        let index_file = self.data_dir.join(project_index::INDEX_FILE_NAME);
        self.media_folders.start(ctx, document.path(), index_file);
        let new = self.media_folders.new_files(project);
        if let Some(line) = self.media_folders.announce(&new) {
            self.status = line;
        }
        if self.drawer.open {
            self.library
                .ask(crate::media_drawer::ASKER, &self.drawer.query(project));
        }
        if self.page == Page::Library {
            self.library
                .ask(crate::library_page::ASKER, &self.library_page.query(project));
        }
        if self.show_search {
            self.library.ask(
                crate::search::ASKER,
                &library::Query::text(self.search_text.trim()),
            );
        }
        if let Some(err) = self.library.take_error() {
            self.errors.push(&err);
        }
        if let Some(err) = self.global_labels.poll() {
            self.errors.push(&err);
        }
    }

    /// Notices media that moved, and points the project at files found again.
    fn relinking(&mut self, ctx: &egui::Context) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        self.relink.know(crate::relink::Known {
            project_file: document.path().to_path_buf(),
            index_file: self.data_dir.join(project_index::INDEX_FILE_NAME),
            extra: self.media_folders.extra().to_vec(),
        });
        self.relink.watch(&self.jobs, ctx, document.project());
        if self.relink.searching().is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let read_only = document.is_read_only();
        let found = match self.relink.automatic(&self.jobs, document.project(), read_only) {
            Some(found) => Some(found),
            None => self.relink.poll(&self.jobs, document.project()),
        };
        let Some(mut found) = found else {
            return;
        };
        // Only files still missing: one may have been found by hand meanwhile.
        found
            .files
            .retain(|(media, _)| self.relink.missing.contains(media));
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let says = crate::relink::outcome(document.project(), &found, &undo);
        if !found.files.is_empty() {
            let command = Command::RelinkFiles {
                files: found.files.clone(),
            };
            if let Err(err) = document.apply(command) {
                self.errors.push(&err);
                return;
            }
            for (media, _) in &found.files {
                self.waveforms.forget(*media);
                self.pictures.forget(*media);
                self.library_page.forget(*media);
            }
        }
        if let Some(says) = says {
            self.status = says;
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
            let saved = if d.is_saving() {
                "saving…"
            } else if d.has_unsaved_changes() {
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
        if clicks.search && !self.show_search {
            self.search_text.clear();
            self.show_search = true;
        }
        self.show_settings |= clicks.settings;
        if clicks.usage {
            self.open_settings_at(settings_ui::RESOURCES);
        }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        if !matches!(self.page, Page::Edit | Page::Audio) {
            self.status_about_timeline.leave_timeline(&mut self.status);
        }
        if let Some(response) = self.errors.show_over(ui, &self.theme) {
            match response {
                ErrorResponse::OpenDiagnostics => self.show_diagnostics = true,
                ErrorResponse::Fix(action) if action.command == crate::recovered::OPEN_LAST_SAVE => {
                    self.open_last_save();
                }
                ErrorResponse::Fix(action) if action.command == SPLIT_AT_PLAYHEAD => {
                    let at = self.playhead_on_frame();
                    if let Err(err) = self.split_at(at) {
                        self.errors.push(&err);
                    }
                }
                ErrorResponse::Fix(action)
                    if ui_kit::settings::open_at_id(&action.command)
                        .is_some_and(|id| self.open_settings_at(id)) => {}
                ErrorResponse::Fix(action) => match import::whats_there_id(&action.command) {
                    Some(id) => {
                        if self.importer.whats_there(&self.jobs, ui.ctx(), id) {
                            self.status = "Importing what is there…".into();
                        }
                    }
                    // A fix the app offered but has nothing behind: a
                    // mistake in the app. It goes in the log; the person,
                    // who clicked it, is told plainly that nothing was done.
                    None => {
                        tracing::warn!(command = %action.command, "no handler for a fix");
                        self.errors.push(&unknown_fix(&action.label));
                    }
                },
            }
            ui.add_space(8.0);
        }
        // The scopes read a small copy of the picture, only while shown.
        self.viewer
            .set_scope(self.page == Page::Color && self.document.is_some());
        if self.page == Page::Edit && self.document.is_some() {
            self.edit_page(ui);
            return;
        }
        if self.page == Page::Library && self.document.is_some() {
            self.library_page_ui(ui);
            return;
        }
        if self.page == Page::Audio && self.document.is_some() {
            self.audio_page_ui(ui);
            return;
        }
        if self.page == Page::Color && self.document.is_some() {
            self.color_page_ui(ui);
            return;
        }
        if self.page == Page::Export
            && let Some(document) = &self.document
        {
            let viewer = &self.viewer;
            self.export_page.aim_at(self.settings.loudness_target.lufs());
            let asked = self.export_page.ui(
                ui,
                &self.theme,
                document.project(),
                document.path(),
                &self.relink.missing,
                || viewer.export_devices(),
            );
            if let Some(err) = asked.problem {
                self.errors.push(&err);
            }
            if asked.find_missing {
                self.relink.choose_folder(ui.ctx());
            }
            return;
        }
        let (title, line) = placeholder(self.page, self.document.is_some());
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .family(ui_kit::fonts::semibold())
                    .size(22.0),
            );
            ui.label(egui::RichText::new(line).color(self.theme.muted));
        });
        ui.add_space(24.0);
        if !self.status.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&self.status)
                    .color(self.theme.text_secondary.gamma_multiply(self.status_strength)),
            );
        }
    }

    /// What the master column asked, the same on every page that shows
    /// it: its loudness counted again, another loudness aimed for (a
    /// setting, written by the settings keeper), or a new volume for the
    /// mix (one undo step).
    fn master_asks(&mut self, asks: ui_kit::master::MasterResponse) {
        if asks.reset
            && let Some(playback) = &self.playback
        {
            playback.reset_loudness();
        }
        if let Some(target) = asks.target {
            self.settings.loudness_target = target;
        }
        if let Some(gain_db) = asks.gain_db {
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::SetMasterGain { sequence, gain_db })
            });
            if let Err(err) = result {
                self.errors.push(&err);
            }
        }
    }

    /// Where the open project's proxies are (none without a project).
    fn project_proxies(&self) -> Vec<std::path::PathBuf> {
        self.document
            .as_ref()
            .map(|d| self.proxies.project_proxies(d.project()))
            .unwrap_or_default()
    }

    /// Measures the cache on a worker, for Settings › Cache.
    fn measure_cache(&mut self) {
        let (project, shelf) = (self.project_proxies(), self.proxies.shelf());
        let others = crate::cache_cleanup::OtherProjects {
            index_file: self.data_dir.join(project_index::INDEX_FILE_NAME),
            this_project: self.document.as_ref().map(|d| d.path().to_path_buf()),
        };
        self.cache_cleanup.measure(&self.jobs, &shelf, project, others);
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
                // Wait for what the screenshot is of: the imports read, the
                // viewer's picture back and the timeline's waveforms and clip
                // pictures read. Not for ever, so a file that can't be shown
                // still gets its screenshot.
                let since = *self.probe.started.get_or_insert(now);
                let ready = (self.importer.busy() == 0
                    && self.viewer.settled()
                    && self.waveforms.settled()
                    && self.pictures.settled()
                    && self.display.settled()
                    && self.proxies.settled()
                    && (!self.drawer.open || self.library.is_current(crate::media_drawer::ASKER))
                    && (self.page != Page::Library
                        || self.document.is_none()
                        || (self.library.is_current(crate::library_page::ASKER)
                            && self.library_page.settled()))
                    && self.media_folders.settled()
                    && self.relink.settled()
                    && self.package.settled()
                    && self.sync.settled()
                    && !self.script.syncing()
                    && (self.page != Page::Export || self.document.is_none() || self.export_page.settled())
                    && (self.page != Page::Audio
                        || self.document.is_none()
                        || (!self.script.making_tracks() && self.audio_page.clarity.settled())))
                    || self.script.waited_too_long(now.duration_since(since));
                // A scripted pointer has done its part, and rested long
                // enough for a tooltip to come up.
                let pointed = self.script.pointed(self.frames, now.duration_since(since));
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
                // A project being made for the test (`--audio-tracks`) is
                // set-up, not use: counting starts a few frames after it.
                let setting_up = self.script.setting_up(self.frames);
                let probe = &mut self.probe;
                if probe.job.is_none() && setting_up {
                    return;
                }
                if probe.job.is_none() {
                    probe.started = Some(now);
                    // On the export pool, which nothing else uses here, so the
                    // analysis pool's own work (waveforms, clip pictures, a
                    // proxy being built) goes on during the test, also on a
                    // machine with one thread per pool.
                    probe.job = Some(self.jobs.submit(
                        PoolKind::Export,
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
                // With files to import at start, it lasts until they are in
                // too: the interface stays smooth through a whole import.
                let done =
                    probe.job.as_ref().and_then(JobHandle::try_result).is_some() && self.importer.busy() == 0;
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
                    let median = probe
                        .ui_times
                        .get(probe.ui_times.len() / 2)
                        .copied()
                        .unwrap_or_default();
                    let failed =
                        p99 > Duration::from_millis(16) || probe.slowest_ui > Duration::from_millis(50);
                    let message = format!(
                        "Responsiveness test {}: {} frames in {:.1} s ({fps:.0} per second), interface work {:.1} ms for half the frames, {:.1} ms for 99 frames in 100 and {:.1} ms at most, longest gap between frames {:.1} ms.",
                        if failed { "FAILED" } else { "passed" },
                        probe.frames,
                        elapsed.as_secs_f64(),
                        median.as_secs_f64() * 1000.0,
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
        self.close_prompt.allow();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Editor {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(pointer) = self.script.pointer() {
            pointer.apply(self.frames, &mut raw_input.events);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.heartbeat.beat();
        let started = Instant::now();
        self.playback_watch.frame_started(started);
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_input = now;
        }
        let idle = now.duration_since(self.last_input) > IDLE_AFTER;
        self.jobs.set_idle(idle);
        let (strength, again) = self.status_fade.follow(&mut self.status, now);
        self.status_strength = strength;
        if let Some(again) = again {
            ctx.request_repaint_after(again);
        }
        self.graphics.frame(&ctx, self.frames);
        // A keyboard switched on the system: keys are written for it from now
        // on (the page draws it too). The window repaints twice a second, so
        // a switch shows within a moment even without input.
        if self.system_keyboard.poll() {
            self.shortcuts
                .set_layout(self.settings.keyboard.resolve(self.system_keyboard.now()));
        }
        // A file chooser that could not open says why.
        if let Some(problem) = ui_kit::file_dialog::take_problem(&ctx) {
            self.errors.push(&problem);
        }
        // A picture the graphics card turned down says why, once per cause.
        if let Some(state) = frame.wgpu_render_state() {
            for problem in ui_kit::video_picture::refusals(state) {
                self.errors.push(&problem);
            }
        }
        // An uncaught graphics error says so once; the log has the rest.
        if let Some(told) = &self.graphics_errors {
            for problem in told.try_iter() {
                self.errors.push(&problem);
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
        if let Some(text) = self.clipboard_text.take() {
            ctx.copy_text(text);
        }
        self.imports(&ctx);
        self.follow_news(&ctx);
        self.bars_news(&ctx);
        self.steady_news(&ctx);
        self.follow_open_timeline();
        script::step(self);
        self.follow_playback(&ctx);
        self.voice_over_poll(&ctx);
        self.viewer.looping = self.looping;
        self.waveforms.poll(&ctx, now);
        self.pictures.poll(&ctx, now);
        let project = self.document.as_ref().map(Document::project);
        if let Some(err) = self.proxies.poll(&self.jobs, &ctx, project, now) {
            self.errors.push(&err);
        }
        self.poll_prerender();
        let polled = self.cache_cleanup.poll();
        if let Some(err) = &polled.problem {
            self.errors.push(err);
        }
        if !polled.cleared.is_empty() {
            self.proxies.cleared(&polled.cleared);
        }
        if polled.measure_again {
            self.measure_cache();
        }
        if self.cache_cleanup.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.relinking(&ctx);
        self.library_index(&ctx);
        self.follow_audio_page();
        self.follow_library_page(&ctx);
        self.frame_grabs(&ctx);
        if let Some(err) = self.viewer.poll(&ctx, self.settings.preview_quality)
            && !self.relink.explains_errors()
        {
            self.errors.push(&err);
        }
        if let Some(err) = self.display.poll(&ctx, self.settings.preview_quality)
            && !self.relink.explains_errors()
        {
            self.errors.push(&err);
        }
        if let Some(slow) = self.viewer.slow_pictures() {
            self.errors.set_fact(crate::slow_pictures::FACT, slow);
        }
        self.autosave(now, idle);
        let pointer_held = ctx.input(|i| i.pointer.any_down());
        if let Some(err) = self.settings_keeper.tick(&self.settings, now, pointer_held) {
            self.errors.push(&err);
        }
        if let Some(keeper) = &mut self.view_keeper {
            keeper.tick(now, &self.timeline, &self.jobs);
        }
        self.thumbnails();

        // The frame after the window was asked to fill the screen (or come
        // back): logged with how long the switch took.
        let first_after_switch = self.full_screen_asked.take();
        if let Some(asked) = first_after_switch {
            self.heartbeat.step(crate::heartbeat::Step::FirstFullScreenFrame);
            tracing::info!(
                "Full screen {}: the first frame after the switch began {} ms after asking",
                if self.full_screen { "on" } else { "off" },
                asked.elapsed().as_millis()
            );
        }
        let asking_full_screen = std::mem::take(&mut self.full_screen_changed);
        if asking_full_screen {
            tracing::info!(
                "Asking the window to {}",
                if self.full_screen {
                    "fill the screen"
                } else {
                    "come back from full screen"
                }
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.full_screen));
            self.full_screen_asked = Some(Instant::now());
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
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(self.theme.bg)
                        .inner_margin(egui::Margin::same(16)),
                )
                .show(ui, |ui| self.page(ui));
        }

        self.viewer_window(&ctx);
        self.voice_over_banner(&ctx);
        self.settings_window(&ctx);
        self.timeline_settings_window(&ctx);
        self.search_window(&ctx);
        self.silence_window(&ctx);
        self.sync_window(&ctx);
        self.label_window(&ctx);
        self.versions_window(&ctx);
        self.templates_window(&ctx);
        self.prune_window(&ctx);
        self.removal_window(&ctx);
        self.package_window(&ctx);
        self.shortcuts_window(&ctx);
        self.follow_background(&ctx);
        let unfinished = self.export_page.unfinished();
        self.close_prompt.frame(&ctx, &self.theme, unfinished);
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
        self.watch_playback();
        // Keep the resource readout and autosave ticking without input.
        ctx.request_repaint_after(Duration::from_millis(500));
        // What comes until the next frame: the switch asked for (so a stop in
        // it is named), the window put away (no frames are drawn then, and
        // that is no stop), or the toolkit drawing this frame on screen.
        use crate::heartbeat::Step;
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        self.heartbeat.step(match () {
            () if asking_full_screen => Step::AskingFullScreen,
            () if minimized => Step::Minimized,
            () => Step::BetweenFrames,
        });
    }

    fn on_exit(&mut self) {
        // Playback and the viewer's decoding stop first and are waited for,
        // briefly: closing while playing must not leave a thread inside the
        // sound card's, a decoder's or the graphics driver's code when the
        // window's graphics go away and the program ends.
        self.playback = None;
        // Background work held back while playing (a save under way among
        // it) runs now: closing waits for that save below.
        self.jobs.closing();
        // An export under way is given up: its worker decodes and draws on
        // the window's graphics device too.
        self.export_page.stop();
        self.library_page.close();
        // So is packaging: what it copied so far is taken away again.
        self.package.stop();
        self.viewer.close();
        self.display.close();
        if let Some(place) = self.display.window_place() {
            self.settings.viewer_window = Some(place);
        }
        if let Err(err) = self.settings_keeper.close(&self.settings) {
            tracing::warn!("{}", err.report());
        }
        if !engine::playback::wait_until_quiet(AUDIO_ENDS_WITHIN) {
            tracing::warn!("the sound did not stop before the window closed");
        }
        // Background reading nobody will look at any more stops here, so
        // closing never waits for a long file.
        self.waveforms.stop();
        self.pictures.stop();
        self.proxies.stop();
        if let Some(keeper) = &mut self.view_keeper {
            keeper.close(&self.timeline);
        }
        // Saves under way finish first (they write one at a time, newest
        // last); the save below then only runs if something is still newer.
        for save in std::mem::take(&mut self.pending_saves) {
            let result = save.handle.wait();
            if let Some(document) = self.document.as_mut()
                && let Err(err) = document.finish_save(save.seq, result)
            {
                tracing::error!("a save under way failed on exit: {}", err.report());
            }
        }
        if let Some(document) = self.document.as_mut()
            && document.has_unsaved_changes()
        {
            match document.save() {
                Ok(()) => self.thumbnail_due = Thumbnail::Now,
                Err(err) => tracing::error!("saving on exit failed: {}", err.report()),
            }
        }
        // A picture still being written is waited for (briefly) and what it
        // shows recorded, so the picture is never written twice at once and
        // not again when it shows the same.
        let free = thumbnail_settled(
            self.thumbnail.take(),
            THUMBNAIL_ENDS_WITHIN,
            &mut self.thumbnail_shows,
        );
        if !free {
            tracing::warn!("the project picture was still being written when the window closed");
        }
        if free
            && self.thumbnail_due == Thumbnail::Now
            && let Some(document) = &self.document
        {
            // One small frame on the way out, so the launcher shows the
            // latest, unless it shows that already.
            let p = document.project();
            let out = project::thumbnail_path(document.path());
            let shows = engine::thumbnail::shows(p, p.active_sequence, self.viewer.playhead);
            let seq = document.newest_entry();
            let still = self
                .viewer
                .export_devices()
                .map(|devices| (p.clone(), p.active_sequence, self.viewer.playhead, devices));
            if let Some(shows) = shows.filter(|s| self.thumbnail_shows.as_ref() != Some(&(s.clone(), seq)))
                && let Err(err) = saving::write_project_picture(still, &shows, &out)
            {
                tracing::info!("no project picture: {}", err.report());
            }
        }
    }
}

/// How long closing the app waits for the sound to let go of the sound
/// card and its files.
const AUDIO_ENDS_WITHIN: Duration = Duration::from_secs(2);

/// What a page shows when it has nothing to show: its name and one line.
/// Without a project every page waits for one; with one, every page draws
/// itself.
fn placeholder(page: Page, has_project: bool) -> (&'static str, &'static str) {
    if !has_project {
        return (
            page.label(),
            "Open a project, or start a new one, in the launcher; then import your recordings here.",
        );
    }
    // Every page draws itself when a project is open.
    (page.label(), "")
}

/// What one press of the command bound to Escape (`ClearSelection`) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeDoes {
    /// Stops a box being followed or a clip being analysed, and nothing else.
    StopTask,
    LeaveFullScreen,
    LeaveLines,
    SelectNothing,
}

/// The first that applies: a task running on a thread of its own is
/// stopped, then full screen left, then the lines tool, then the selection
/// cleared. One press does one of them.
fn escape_does(task_running: bool, full_screen: bool, lines_on: bool) -> EscapeDoes {
    if task_running {
        EscapeDoes::StopTask
    } else if full_screen {
        EscapeDoes::LeaveFullScreen
    } else if lines_on {
        EscapeDoes::LeaveLines
    } else {
        EscapeDoes::SelectNothing
    }
}

impl Editor {
    /// Stops a box being followed (what it found so far is kept) and a clip
    /// being analysed (nothing set); their news says when they have ended.
    fn stop_tasks(&mut self) {
        if let Some(job) = &self.following {
            job.stop();
        }
        if let Some(job) = &self.steadying {
            job.stop();
        }
    }
}

/// " (Esc stops it)" with the key the shortcut table has for stopping a
/// task now, or nothing when no key is bound to it.
fn stops_it(shortcuts: &ShortcutTable) -> String {
    match shortcuts.keys_for(Action::ClearSelection) {
        keys if keys.is_empty() => String::new(),
        keys => format!(" ({keys} stops it)"),
    }
}

/// What the status line says when the choice of what a click moves changes.
fn click_moves_says(moves: ui_kit::ClickMoves, edit_at_playhead: bool) -> &'static str {
    match (moves, edit_at_playhead) {
        (ui_kit::ClickMoves::EditNeedle, false) => {
            "A click in the timeline's ruler now moves the edit needle; drag the playhead by its handle at the top of the ruler, or Alt+click where it should go"
        }
        (ui_kit::ClickMoves::EditNeedle, true) => {
            "A click will move the edit needle once Edit at the playhead is off; until then it moves the playhead"
        }
        (ui_kit::ClickMoves::Playhead, _) => "A click in the timeline's ruler moves the playhead",
    }
}

/// The fix button's command that splits at the playhead when the edit
/// needle found nothing to split.
const SPLIT_AT_PLAYHEAD: &str = "split.at-playhead";

/// Seconds (as the timeline widget gives them) as exact time, to the
/// microsecond. The one place the window turns the widget's seconds into
/// time; everything after works on exact time.
pub(crate) fn seconds(s: f64) -> Time {
    Time::from_ratio((s * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO)
}

/// The region from `range`, both ends put on the nearest frames of `rate`;
/// `None` when there is no range or it is empty there.
fn region_on_frames(rate: project::Rate, range: Option<(Time, Time)>) -> Option<project::Region> {
    range.and_then(|(a, b)| project::Region::between(rate.snap(a), rate.snap(b)))
}

/// The region marking in (`start`) or out at `at` gives: that end at
/// `at`, the other end of `region` kept when it is on the right side,
/// else the timeline's start or end (`length`). Exact time throughout.
fn marked(region: Option<project::Region>, length: Time, at: Time, start: bool) -> (Time, Time) {
    let (from, to) = region.map_or((Time::ZERO, length), |r| (r.start, r.end));
    if start {
        (at, if to > at { to } else { length })
    } else {
        (if from < at { from } else { Time::ZERO }, at)
    }
}

/// A project named on the command line, and what to do with unsaved work from a crash.
#[derive(Debug, Clone)]
pub struct ProjectToOpen {
    pub path: PathBuf,
    /// The launcher asks; the editor on its own always recovers.
    pub recovery: Recovery,
    /// Media files to import once it is open.
    pub import: Vec<PathBuf>,
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

/// The cut between pictures nearest `at` on one side: the first after it
/// (`later`) or the last before it, on any video track; the timeline's
/// start counts as one.
fn cut_beside(seq: &project::Sequence, at: Time, later: bool) -> Option<Time> {
    let cuts = seq
        .video_tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .flat_map(|clip| [clip.start, clip.end()])
        .chain(std::iter::once(Time::ZERO));
    match later {
        true => cuts.filter(|t| *t > at).min(),
        false => cuts.filter(|t| *t < at).max(),
    }
}

/// The timeline view `page` shows: the Audio page's own (`audio`), or the
/// Edit page's (`edit`), which the other pages share. Takes the two views
/// borrowed either way (shared or mutable), so every choice goes through here.
fn view_on<V>(page: Page, edit: V, audio: V) -> V {
    match page {
        Page::Audio => audio,
        _ => edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Marking works on exact time: a playhead just short of half a frame
    /// marks the frame before, where rounding it to the microsecond first
    /// would have crossed the half and marked the frame after.
    #[test]
    fn marking_keeps_the_exact_time() {
        let rate = project::Rate::FPS_30;
        let length = Time::seconds(10);
        // Half a frame (1/60 s) less a tenth of a microsecond.
        let at = Time::from_ratio(9_999_940, 600_000_000).expect("a time");
        let through_seconds = rate.snap(seconds(at.as_secs_f64()));
        assert_eq!(
            through_seconds,
            rate.frame_start(1),
            "the old way crossed the half"
        );
        let region = region_on_frames(rate, Some(marked(None, length, at, true))).expect("a region");
        assert_eq!((region.start, region.end), (Time::ZERO, length));
        // Mark out keeps the region's start, exactly, and the marked end lands
        // on the frame nearest the exact playhead.
        let kept = project::Region::between(rate.frame_start(3), length);
        let out = Time::from_ratio(289_999_940, 600_000_000).expect("a time");
        let region = region_on_frames(rate, Some(marked(kept, length, out, false))).expect("a region");
        assert_eq!(
            (region.start, region.end),
            (rate.frame_start(3), rate.frame_start(14))
        );
        assert_eq!(
            marked(kept, length, Time::ZERO, false),
            (Time::ZERO, Time::ZERO),
            "nothing before"
        );
    }

    /// Actions on the timeline (closing a selected gap, choosing keyframe
    /// marks) act on the view of the page on screen: on the Audio page its
    /// own, so the gap closed there is the one let go.
    #[test]
    fn the_audio_page_acts_on_its_own_timeline_view() {
        let mut edit = ui_kit::TimelineView::default();
        let mut audio = ui_kit::TimelineView::default();
        let shown = view_on(Page::Audio, &mut edit, &mut audio) as *const _;
        assert!(std::ptr::eq(shown, &audio), "the Audio page's own");
        for page in [Page::Edit, Page::Color, Page::Library, Page::Export] {
            let shown = view_on(page, &mut edit, &mut audio) as *const _;
            assert!(std::ptr::eq(shown, &edit), "{page:?}: the Edit page's");
        }
        // Following playback reads the same view through a shared borrow.
        assert!(std::ptr::eq(view_on(Page::Audio, &edit, &audio), &audio));
        assert!(std::ptr::eq(view_on(Page::Edit, &edit, &audio), &edit));
    }

    /// Every refit clears both views' fit flag through `refit`: one written
    /// by hand on its own leaves the other page zoomed on a timeline whose
    /// length changed. (The screenshot script resets only the Audio page's
    /// view, to a fresh one, on purpose.)
    #[test]
    fn every_refit_goes_through_one_place() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.join("editor.rs")];
        let parts = std::fs::read_dir(src.join("editor")).expect("the editor's folder");
        for entry in parts {
            let path = entry.expect("an entry").path();
            if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("script.rs") {
                files.push(path);
            }
        }
        let needle = ["fitted", " = false"].concat();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("a source file");
            let found = text.matches(needle.as_str()).count();
            let allowed = if file.ends_with("editor.rs") { 2 } else { 0 };
            assert_eq!(found, allowed, "{}: call `refit` instead", file.display());
        }
    }

    #[test]
    fn up_and_down_go_to_the_cut_before_and_after() {
        let mut p = project::Project::new("T", project::SequenceSettings::GAME_CLIP);
        for (start, length) in [(2, 3), (5, 4)] {
            p = Command::AddAdjustment {
                sequence: p.active_sequence,
                at: Time::seconds(start),
                length: Time::seconds(length),
                track: None,
            }
            .apply(&p)
            .expect("added");
        }
        let seq = p.active().expect("a timeline");
        let at = |s: i64| Time::seconds(s);
        assert_eq!(cut_beside(seq, at(3), true), Some(at(5)));
        assert_eq!(cut_beside(seq, at(5), true), Some(at(9)));
        assert_eq!(cut_beside(seq, at(9), true), None, "past the last");
        assert_eq!(cut_beside(seq, at(5), false), Some(at(2)));
        assert_eq!(cut_beside(seq, at(1), false), Some(Time::ZERO));
    }

    #[test]
    fn empty_pages_say_what_to_do_and_never_name_plan_phases() {
        for page in Page::ALL {
            for has_project in [false, true] {
                let (title, line) = placeholder(page, has_project);
                assert!(!title.is_empty());
                assert!(!line.contains("Phase"), "{page:?}: {line}");
            }
            assert!(placeholder(page, false).1.contains("Open a project"));
        }
    }

    /// The lines tool's hint is about the timeline: a page without one
    /// clears it, anything else stays.
    #[test]
    fn the_lines_hint_is_only_for_pages_with_a_timeline() {
        let mut about = crate::status_line::AboutTimeline::default();
        for hint in [LINES_ON, LINES_OFF] {
            let mut status = String::new();
            about.say(&mut status, hint);
            assert_eq!(status, hint);
            about.leave_timeline(&mut status);
            assert!(status.is_empty(), "{hint}");
        }
        let mut status = "Imported 3 files".to_owned();
        about.leave_timeline(&mut status);
        assert_eq!(status, "Imported 3 files");
    }

    /// A fix with nothing behind it is told plainly, by its label, never
    /// by its command.
    #[test]
    fn a_fix_with_nothing_behind_it_says_so_plainly() {
        let err = unknown_fix("Import what's there");
        assert_eq!(err.what(), "\"Import what's there\" couldn't be done");
        assert!(
            err.why_text()
                .is_some_and(|why| why.contains("Nothing was changed"))
        );
    }

    /// The command bound to Escape stops a running follow or analysis and
    /// does nothing else that press: the selection, full screen and the
    /// lines tool stay. Without one it does what it always did.
    #[test]
    fn escape_stops_a_running_task_and_nothing_else() {
        for full_screen in [false, true] {
            for lines in [false, true] {
                assert_eq!(escape_does(true, full_screen, lines), EscapeDoes::StopTask);
            }
        }
        assert_eq!(escape_does(false, false, false), EscapeDoes::SelectNothing);
        assert_eq!(escape_does(false, true, true), EscapeDoes::LeaveFullScreen);
        assert_eq!(escape_does(false, false, true), EscapeDoes::LeaveLines);
    }

    /// The hint names the key bound now, and is left out with none bound.
    #[test]
    fn the_stop_hint_names_the_key_bound_now() {
        let mut table = ShortcutTable::default();
        let usual = stops_it(&table);
        assert!(usual.ends_with("stops it)"), "{usual}");
        assert!(!usual.contains("Select nothing"), "{usual}");
        let line = table
            .lines()
            .iter()
            .position(|b| b.action == Action::ClearSelection)
            .expect("a line");
        let keys = ui_kit::Shortcut::ctrl_shift(egui::Key::Period);
        table.rebind(line, keys).expect("free keys");
        assert_eq!(stops_it(&table), format!(" ({} stops it)", table.written(keys)));
        assert_ne!(stops_it(&table), usual);
        table.clear(line);
        assert_eq!(stops_it(&table), "");
    }

    #[test]
    fn the_status_line_says_what_a_click_moves_now() {
        use ui_kit::ClickMoves::{EditNeedle, Playhead};
        assert!(click_moves_says(EditNeedle, false).contains("edit needle"));
        assert!(
            click_moves_says(EditNeedle, false).contains("handle"),
            "and how to move the playhead"
        );
        assert!(
            click_moves_says(EditNeedle, true).contains("Edit at the playhead"),
            "with no needle of its own, why nothing seems to change"
        );
        assert!(click_moves_says(Playhead, false).contains("playhead"));
    }
}
