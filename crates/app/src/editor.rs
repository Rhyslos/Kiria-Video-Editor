//! The editor window.

mod audio_ui;
mod clip_clipboard;
mod keyframe_clipboard;
mod library_ui;
mod region_cuts;
mod timelines;
mod undo_says;

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
    /// Where proxies are kept (the app's cache folder).
    pub proxy_dir: PathBuf,
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
    /// Full saves handed to a worker: the journal entry each includes, and
    /// whether it was asked for (Ctrl+S) rather than an autosave.
    pending_saves: Vec<PendingSave>,
    /// Writes `settings.ron` off this thread, once a change has settled.
    settings_keeper: crate::settings::SettingsKeeper,
    /// The launcher's picture of the project, being written after a save.
    thumbnail: Option<JobHandle<bool>>,
    /// A save finished (or the project has no picture yet): write one.
    thumbnail_due: Thumbnail,
    importer: Importer,
    import_requested: bool,
    /// What the timeline's settings settled on during the import under way.
    import_settled: Option<String>,
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
    /// The timeline shown last frame, to notice another one opening.
    shown_sequence: Option<project::SequenceId>,
    /// Where the playhead was in each timeline left this session.
    playheads: std::collections::HashMap<project::SequenceId, Time>,
    /// The timeline was fitted to the project once already.
    fitted: bool,
    waveforms: crate::waveforms::Waveforms,
    pictures: crate::clip_pictures::ClipPictures,
    /// Proxies for the project's video files, built in the background.
    proxies: crate::proxies::ProxyBuilds,
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
    /// `--library-tab`: the tab to open once the project is in.
    library_tab: Option<String>,
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
    /// Ask it as soon as an export is unfinished (`--close-request`, for
    /// screenshots of the question).
    close_request: bool,
    /// The Audio page: its tool, track, filter, mixer and counting.
    audio_page: crate::audio_page::AudioPage,
    /// What the command line asked of the Audio page and is still to do
    /// (its track, the demo tracks once imports are in).
    audio_start: crate::audio_page::Start,
    /// The frame the `--audio-tracks` demo was made on: the responsiveness
    /// test starts counting a few frames after it, not during its set-up.
    audio_demo_made: Option<u64>,
    /// The master's meter as last read while playing, shown after.
    audio_held_master: Option<audio::dsp::loudness::Measured>,
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
    master: ui_kit::master::MasterColumn,
    versions: crate::versions_window::VersionsWindow,
    templates: crate::templates_window::TemplatesWindow,
    /// Prune: the files no timeline uses, to take out of the project.
    prune: crate::prune_window::PruneWindow,
    /// Package project: the project and its media, copied into one folder.
    package: crate::package_window::PackageWindow,
    /// `--package-to`: open Package project with this folder, once.
    package_to: Option<PathBuf>,
    /// Package project was asked for; the folder dialog opens on the next frame.
    package_requested: bool,
    shortcuts_page: Option<ui_kit::keyboard::ShortcutsPage>,
    /// The system's keyboard layout, followed for the shortcuts page and
    /// for writing keys in menus and tips.
    system_keyboard: crate::system_keyboard::Follower,
    cheat_sheet: crate::cheat_sheet::CheatSheet,
    /// Commands asked for on the command line, run on the first frames.
    startup_actions: Vec<Action>,
    /// Commands asked for on the command line, run once the scripted
    /// pointer has done its part (`--then`).
    actions_after_pointer: Vec<Action>,
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
    /// `--select-first-clip`: still to be done, once there is a clip.
    select_first_clip: bool,
    /// `--sync-by-sound`: still to be done, once the imports are in.
    sync_when_ready: bool,
    /// Save this frame was asked for; the dialog opens on the next frame.
    save_frame_requested: bool,
    /// `--play`: start playing as soon as the imports are in.
    play_when_ready: bool,
    show_timeline_settings: bool,
    show_settings: bool,
    /// Settings › Cache: what the cache takes, and clearing it.
    cache_cleanup: crate::cache_cleanup::CacheCleanup,
    /// The Settings window was open last frame (the cache is measured
    /// when it opens).
    settings_were_open: bool,
    show_diagnostics: bool,
    show_search: bool,
    search_text: String,
    /// The Settings section search asked for: marked while the window is
    /// open, and scrolled to over its first few frames (its size settles).
    settings_found: Option<(crate::settings_sections::Section, u8)>,
    /// The proxy changes Settings › Cache was last measured at.
    cache_seen: u64,
    status: String,
    /// Where the playhead was before a clip began to be slipped: the viewer
    /// shows the clip's first frame while it is, then the playhead goes back.
    slip_playhead: Option<Time>,
    /// The status line's age: it fades out a few seconds after it last changed.
    status_fade: crate::status_line::Fading,
    /// How strongly the status line is drawn this frame (1 full).
    status_strength: f32,
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
/// Room between the viewer and the inspector beside it.
const INSPECTOR_GAP: f32 = 12.0;

/// How long each frame may spend adding imported files to the project.
const IMPORT_BUDGET: Duration = Duration::from_millis(2);

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
            proxy_dir,
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
        let find = project.as_ref().and_then(|p| p.find.clone()).unwrap_or_default();
        let watch_folders = project
            .as_ref()
            .map(|p| p.watch_folders.clone())
            .unwrap_or_default();
        let export_queue = project.as_ref().is_some_and(|p| p.export_queue);
        let close_request = project.as_ref().is_some_and(|p| p.close_request);
        let start_page = match project.as_ref() {
            Some(p) if p.export_page || p.export_queue => Page::Export,
            Some(p) => p.page.unwrap_or_default(),
            None => Page::default(),
        };
        let library_tab = project.as_ref().and_then(|p| p.library_tab.clone());
        let library_page = crate::library_page::LibraryPage::selecting(
            project.as_ref().and_then(|p| p.library_select.clone()),
        );
        let full_screen = project.as_ref().is_some_and(|p| p.full_screen);
        let audio_start = project.as_ref().map(|p| p.audio).unwrap_or_default();
        let mut settings = settings;
        if let Some((bottom, way)) = audio_start.view {
            settings.audio_bottom = bottom;
            if let Some(way) = way {
                settings.audio_split = way;
            }
        }
        let mut audio_page = crate::audio_page::AudioPage::default();
        audio_page.tool = audio_start.tool.unwrap_or_default();
        let select_first_clip = project.as_ref().is_some_and(|p| p.select_first_clip);
        let sync_when_ready = project.as_ref().is_some_and(|p| p.sync_by_sound);
        let hold_reading = project.as_ref().and_then(|p| p.hold_reading);
        let hold_proxy = project.as_ref().and_then(|p| p.hold_proxy);
        let package_to = project.as_ref().and_then(|p| p.package_to.clone());
        let hold_package = project.as_ref().and_then(|p| p.hold_package);
        let convert = crate::video_picture::conversion(
            cc.wgpu_render_state.as_ref(),
            project.as_ref().is_some_and(|p| p.cpu_colour),
        );
        let decoding =
            crate::device_decoding::WindowDecoding::new(&graphics, cc.wgpu_render_state.as_ref(), convert);
        // The usual shortcuts, with the ones changed on the shortcuts page.
        let mut shortcuts = ShortcutTable::restored(&settings.shortcuts);
        let system_keyboard = crate::system_keyboard::Follower::start();
        shortcuts.set_layout(settings.keyboard.resolve(system_keyboard.now()));
        let shortcuts_page =
            shortcuts_page.map(|state| ui_kit::keyboard::ShortcutsPage::preview(state, &shortcuts));
        let named = |names: &[String]| -> Vec<Action> {
            names
                .iter()
                .filter_map(|name| {
                    let found = Action::from_name(name);
                    if found.is_none() {
                        tracing::warn!("--action {name}: no such command");
                    }
                    found
                })
                .collect()
        };
        let startup_actions = project.as_ref().map(|p| named(&p.actions)).unwrap_or_default();
        let actions_after_pointer = project
            .as_ref()
            .map(|p| named(&p.then_actions))
            .unwrap_or_default();
        let document = project.and_then(|p| match open_or_create(&p.path, p.recovery) {
            Ok(doc) => {
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
        let mut viewer = Viewer::new(
            &cc.egui_ctx,
            start_at,
            convert,
            decoding,
            cc.wgpu_render_state
                .as_ref()
                .map(|state| (state.device.clone(), state.queue.clone())),
        );
        let proxies = crate::proxies::ProxyBuilds::new(
            proxy_dir,
            settings.proxies,
            jobs.budgets().disk_cache_gb,
            hold_proxy,
        );
        // Before the split view's Source is made: it takes the shelf from here.
        viewer.set_proxies(Some(proxies.shelf()));
        viewer.set_key_frame_first(settings.jump_picture.key_frame_first());
        let display = crate::display::ViewerDisplay::new(settings.split_view, &viewer);
        let global_labels = crate::global_labels::Everywhere::load(&data_dir, &mut errors);
        let mut export_page =
            crate::export_page::ExportPage::new(&data_dir, settings.export_pace, &mut errors);
        if export_queue {
            export_page.demo_queue();
        }
        let settings_keeper = crate::settings::SettingsKeeper::new(&data_dir, &settings, Instant::now());
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
            pending_saves: Vec::new(),
            settings_keeper,
            thumbnail: None,
            thumbnail_due: Thumbnail::IfMissing,
            importer,
            import_requested: false,
            import_settled: None,
            viewer,
            display,
            playback: None,
            timeline,
            view_keeper,
            selected: Vec::new(),
            tabs: Default::default(),
            shown_sequence: None,
            playheads: Default::default(),
            fitted: false,
            waveforms: crate::waveforms::Waveforms::holding_at(hold_reading),
            pictures: crate::clip_pictures::ClipPictures::holding_at(hold_reading),
            proxies,
            track_paces: timeline_panel::TrackPaces::default(),
            relink: crate::relink::Relinker::default(),
            frame_grab: crate::frame_grab::FrameGrab::default(),
            silence: crate::silence_panel::SilencePanel::default(),
            sync: crate::sync_panel::SyncPanel::default(),
            label_editor: None,
            master: ui_kit::master::MasterColumn::default(),
            versions: crate::versions_window::VersionsWindow::default(),
            templates: crate::templates_window::TemplatesWindow::default(),
            prune: crate::prune_window::PruneWindow::default(),
            package: crate::package_window::PackageWindow::holding_at(hold_package),
            package_to,
            package_requested: false,
            shortcuts_page,
            system_keyboard,
            cheat_sheet: crate::cheat_sheet::CheatSheet::default(),
            startup_actions,
            actions_after_pointer,
            copied_keyframes: None,
            copied_clips: None,
            clipboard_text: None,
            full_screen,
            full_screen_changed: full_screen,
            select_first_clip,
            sync_when_ready,
            save_frame_requested: false,
            global_labels,
            media_folders: crate::media_folders::MediaFolders::new(watch_folders),
            library: {
                let ctx = cc.egui_ctx.clone();
                library::Live::start(move || ctx.request_repaint())
            },
            drawer: crate::media_drawer::MediaDrawer::new(drawer_open, find),
            library_page,
            library_tab,
            shown_page: start_page,
            inspector: crate::inspector::Inspector::default(),
            picture_handles: crate::picture_handles::PictureHandles::default(),
            export_page,
            close_prompt: Default::default(),
            close_request,
            audio_page,
            audio_start,
            audio_held_master: None,
            audio_demo_made: None,
            lines_on: false,
            lines: ui_kit::LinesShown::default(),
            play_when_ready,
            show_timeline_settings: false,
            show_settings: false,
            cache_cleanup: Default::default(),
            settings_were_open: false,
            show_diagnostics: false,
            show_search: false,
            search_text: String::new(),
            settings_found: None,
            cache_seen: 0,
            status: String::new(),
            slip_playhead: None,
            status_fade: Default::default(),
            status_strength: 1.0,
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
        // The Library page's player takes the transport's keys and marking.
        if self.page == Page::Library && self.document.is_some() && self.library_page.command(action) {
            return;
        }
        let result = match action {
            Action::Undo => self.step_history(false),
            Action::Redo => self.step_history(true),
            Action::Save => self.save_now(),
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
            Action::RemoveRegion => self.cut_region(ui_kit::RegionCut::Remove),
            Action::LiftRegion => self.cut_region(ui_kit::RegionCut::Lift),
            Action::KeepRegion => self.cut_region(ui_kit::RegionCut::Keep),
            Action::EditAtPlayhead => {
                self.settings.edit_at_playhead = !self.settings.edit_at_playhead;
                self.status = if self.settings.edit_at_playhead {
                    "Edit at the playhead: cuts, splits and pastes happen at the playhead."
                } else {
                    "The edit needle is back: cuts, splits and pastes happen at the red needle."
                }
                .into();
                self.settings.save(&self.data_dir)
            }
            Action::MarkIn | Action::MarkOut => self.mark(action == Action::MarkIn),
            Action::RemoveSilence => self.with_document(|_| Ok(())).map(|()| self.silence.open = true),
            Action::SaveFrame => self
                .with_document(|_| Ok(()))
                .map(|()| self.save_frame_requested = true),
            Action::Split => {
                let at = self.edit_point();
                let result = self.split_at(at);
                // Nothing at the edit needle (often off screen): offer the
                // playhead, and say where the needle is and how to merge them.
                match result {
                    Err(err) if !self.settings.edit_at_playhead && self.playhead_on_frame() != at => Err(err
                        .fix(format!(
                            "Split at the playhead instead, or move the edit needle (the red line, at {}) over a clip. To always cut at the playhead, turn on Display \u{203a} Edit at the playhead{}.",
                            ui_kit::time_text::duration(at.as_secs_f64() as u64),
                            match self.shortcuts.keys_for(Action::EditAtPlayhead) {
                                keys if keys.is_empty() => String::new(),
                                keys => format!(" ({keys})"),
                            }
                        ))
                        .action(diag::FixAction::new("Split at the playhead instead", SPLIT_AT_PLAYHEAD))),
                    other => other,
                }
            }
            Action::Delete | Action::RippleDelete => {
                if self.selected.is_empty()
                    && let Some((start, end)) = self.shown_view().selected_gap()
                {
                    // A selected gap: closed on every track, whichever key.
                    self.close_gap(start, end)
                } else if self.selected.is_empty() {
                    self.status = "Click a clip, or the gap between two clips, on the timeline first.".into();
                    Ok(())
                } else {
                    self.remove_selected(action == Action::RippleDelete)
                }
            }
            Action::SelectAll => {
                if let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) {
                    self.selected = timeline_panel::all_clips(seq);
                    let count = self.selected.len();
                    let delete = self.shortcuts.keys_or_name(Action::Delete);
                    self.status = match count {
                        0 => "Nothing on this timeline to select".into(),
                        n => format!("{n} clips selected: {delete} deletes them all as one step"),
                    };
                }
                Ok(())
            }
            Action::BuildProxies => self.with_document(|_| Ok(())).map(|()| {
                let count = self.proxies.build_all();
                self.status = match count {
                    0 => "Every video file of the project has its proxy, or one being built".into(),
                    1 => "Building 1 proxy in the background".into(),
                    n => format!("Building {n} proxies in the background, one at a time"),
                };
            }),
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
            Action::Prune => self.with_document(|_| Ok(())).map(|()| self.prune.open()),
            Action::PackageProject => self
                .with_document(|_| Ok(()))
                .map(|()| self.package_requested = true),
            Action::Templates | Action::SaveTemplate => self
                .with_document(|_| Ok(()))
                .map(|()| self.templates.show(action == Action::SaveTemplate)),
            Action::KeyboardShortcuts => {
                self.shortcuts_page.get_or_insert_with(Default::default);
                Ok(())
            }
            Action::VolumeLines => {
                self.lines_on = !self.lines_on;
                self.status = if self.lines_on {
                    "Lines: click an audio track to add a point, drag one to move it, double-click it to remove it"
                } else {
                    "Lines off: the pointer works clips again"
                }
                .into();
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
                Ok(())
            }
            Action::CompactTracks => {
                self.settings.compact_tracks = !self.settings.compact_tracks;
                self.timeline.compact = self.settings.compact_tracks;
                self.status = if self.settings.compact_tracks {
                    "Compact tracks: every track a thin strip. The heights you gave them come back when you turn it off"
                } else {
                    "Tracks at their own heights again"
                }
                .into();
                Ok(())
            }
            Action::TrackDivider => {
                self.settings.one_track_list = !self.settings.one_track_list;
                self.timeline.divider = !self.settings.one_track_list;
                self.status = if self.settings.one_track_list {
                    "No divider: video and audio tracks are one list that scrolls together"
                } else {
                    "The divider between video and audio tracks is back where it was"
                }
                .into();
                Ok(())
            }
            Action::ClickMovesNeedle => {
                self.settings.click_moves = match self.settings.click_moves {
                    ui_kit::ClickMoves::Playhead => ui_kit::ClickMoves::EditNeedle,
                    ui_kit::ClickMoves::EditNeedle => ui_kit::ClickMoves::Playhead,
                };
                self.timeline.click_moves = self.settings.click_moves;
                self.status =
                    click_moves_says(self.settings.click_moves, self.settings.edit_at_playhead).into();
                Ok(())
            }
            // With the viewer in its own window, that window fills its screen.
            Action::FullScreenViewer if self.display.window_open() => {
                self.display.switch_window_full_screen();
                Ok(())
            }
            Action::FullScreenViewer => {
                self.set_full_screen(!self.full_screen);
                Ok(())
            }
            Action::SplitView => {
                let on = !self.display.split();
                self.display.set_split(on, &self.viewer);
                self.settings.split_view = on;
                self.status = if on {
                    "Split view: the selected clip's own picture (Source) beside the timeline's (Program)"
                } else {
                    "Split view off"
                }
                .into();
                Ok(())
            }
            Action::ViewerWindow => self.switch_viewer_window(),
            Action::ViewerZoomFit | Action::ViewerZoom100 | Action::ViewerZoomIn | Action::ViewerZoomOut => {
                self.viewer.zoom_to(ui_kit::viewer::ZoomPick::Command(action));
                Ok(())
            }
            Action::NewTimeline => self.new_timeline(false),
            Action::DuplicateTimeline => self.new_timeline(true),
            Action::NestSelection => self.nest_selection(),
            Action::SyncBySound => {
                self.start_sync();
                Ok(())
            }
            Action::AddAdjustmentLayer => {
                let at = self.viewer.playhead;
                self.add_adjustment(at, None)
            }
            Action::AddVideoTrack => self.add_track(project::TrackKind::Video),
            Action::AddAudioTrack => self.add_track(project::TrackKind::Audio),
            Action::ZoomToFit => {
                self.fitted = false;
                self.audio_page.fitted = false;
                Ok(())
            }
            Action::ZoomIn
            | Action::ZoomOut
            | Action::TallerTracks
            | Action::ShorterTracks
            | Action::UsualTrackHeights => {
                if self.settings.compact_tracks && action != Action::ZoomIn && action != Action::ZoomOut {
                    self.status = format!(
                        "Compact tracks are all one height: turn them off ({}) to change track heights",
                        self.shortcuts.keys_or_name(Action::CompactTracks)
                    );
                }
                self.shown_view().step(match action {
                    Action::ZoomIn => ui_kit::ViewStep::ZoomIn,
                    Action::ZoomOut => ui_kit::ViewStep::ZoomOut,
                    Action::TallerTracks => ui_kit::ViewStep::TallerTracks,
                    Action::ShorterTracks => ui_kit::ViewStep::ShorterTracks,
                    _ => ui_kit::ViewStep::UsualTrackHeights,
                });
                Ok(())
            }
            Action::SelectTool => {
                self.shown_view().tool = ui_kit::Tool::Select;
                Ok(())
            }
            Action::BladeTool
            | Action::RippleTool
            | Action::RollTool
            | Action::SlipTool
            | Action::SlideTool => {
                self.shown_view().tool = match action {
                    Action::BladeTool => ui_kit::Tool::Blade,
                    Action::RippleTool => ui_kit::Tool::Ripple,
                    Action::RollTool => ui_kit::Tool::Roll,
                    Action::SlipTool => ui_kit::Tool::Slip,
                    _ => ui_kit::Tool::Slide,
                };
                Ok(())
            }
            Action::PictureHandles => {
                self.settings.picture_handles = !self.settings.picture_handles;
                self.status = if self.settings.picture_handles {
                    "Handles on the picture: drag the selected clip to move it, a corner to resize it, a side to crop it, the round handle to turn it"
                } else {
                    "Handles on the picture off"
                }
                .into();
                Ok(())
            }
            Action::ToggleSnapping => {
                let view = self.shown_view();
                view.snapping = !view.snapping;
                self.status = if view.snapping {
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
                // A fresh search each time it opens.
                if !self.show_search {
                    self.search_text.clear();
                }
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
            Action::PreviousKeyframe | Action::NextKeyframe => {
                self.jump_to_keyframe(action == Action::NextKeyframe);
                Ok(())
            }
            Action::Copy => self.copy(),
            Action::Cut => self.cut(),
            Action::Paste => self.paste(),
        };
        if let Err(err) = result {
            self.errors.push(&err);
        }
    }

    /// The timeline the page shown has: the Audio page's own, or the Edit page's.
    fn shown_view(&mut self) -> &mut ui_kit::TimelineView {
        match self.page {
            Page::Audio => &mut self.audio_page.timeline,
            _ => &mut self.timeline,
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

    /// Splits at `at`: the first selected clip (with its linked clips), or
    /// every clip there when none is selected.
    fn split_at(&mut self, at: Time) -> diag::Result<()> {
        let clip = self.selected.first().copied();
        self.with_document(|d| {
            let sequence = d.project().active_sequence;
            d.apply(Command::Split { sequence, at, clip })
        })
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

    /// Mark in (`start`) or out at the playhead: the region's start or end
    /// there, its other end kept when it is on the right side, else the
    /// timeline's start or end.
    fn mark(&mut self, start: bool) -> diag::Result<()> {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return Ok(());
        };
        let length = seq.length().as_secs_f64();
        let at = self.viewer.playhead.as_secs_f64();
        let (from, to) = seq
            .region
            .map_or((0.0, length), |r| (r.start.as_secs_f64(), r.end.as_secs_f64()));
        let region = if start {
            (at, if to > at { to } else { length })
        } else {
            (if from < at { from } else { 0.0 }, at)
        };
        self.set_region(Some(region))
    }

    /// Moves the playhead; while playing, playback carries on from there.
    fn seek(&mut self, seconds: f64) {
        let at = Time::from_ratio((seconds * 1_000_000.0).round() as i64, 1_000_000).unwrap_or(Time::ZERO);
        self.go_to(at);
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
            E::Line { owner, kind, points } => self.line_event(owner, kind, &points),
            E::TrackRole { track, role } => self.role_event(track, role),
            E::Select { clip, add } => timeline_panel::select(&mut self.selected, project::ClipId(clip), add),
            E::ClearSelection => self.selected.clear(),
            E::RegionCut(cut) => {
                if let Err(err) = self.cut_region(cut) {
                    self.errors.push(&err);
                }
            }
            E::CloseGap { start, end } => {
                if let Err(err) = self.close_gap(start, end) {
                    self.errors.push(&err);
                }
            }
            E::Move {
                clip,
                by,
                track,
                new_track,
            } => {
                let mut says = None;
                let selected = self.selected.clone();
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let clip = project::ClipId(clip);
                    let together = timeline_panel::moves_together(seq, &selected, clip);
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
                    if together {
                        // One of several selected clips: they all move, one step.
                        return d.apply(Command::MoveClips {
                            sequence,
                            clips: selected,
                            by,
                        });
                    }
                    if track.is_none() && !new_track {
                        return d.apply(Command::MoveLinked { sequence, clip, by });
                    }
                    // To another track, or past the last one to a new
                    // track: linked sound finds a free place by itself.
                    let before = seq.clone();
                    d.apply(Command::MoveBetweenTracks {
                        sequence,
                        clip,
                        track: track.map(project::TrackId),
                        by,
                    })?;
                    says = d
                        .project()
                        .active()
                        .map(|after| timeline_panel::moved_says(&before, after, clip));
                    Ok(())
                });
                match result {
                    Ok(()) => self.status = says.unwrap_or_default(),
                    Err(err) => self.errors.push(&err),
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
                // Heard within a block while playing: `follow_playback` sends it.
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
                let at = self
                    .document
                    .as_ref()
                    .and_then(|d| d.project().active())
                    .map_or(Time::ZERO, |seq| seq.settings.frame_rate.snap(seconds(at)));
                let (media, track) = (project::MediaId(media), track.map(project::TrackId));
                if insert {
                    // Ctrl held: everything from there on moves later.
                    let result = self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        d.apply(Command::DropMedia {
                            sequence,
                            media,
                            at,
                            track,
                            insert,
                        })
                    });
                    if let Err(err) = result {
                        self.errors.push(&err);
                    }
                } else {
                    self.lay_media(media, at, track);
                }
            }
            E::OpenNested { clip } => self.open_nested(project::ClipId(clip)),
            E::FitNested { clip } => self.fit_nested(project::ClipId(clip)),
            E::AddAdjustment { at, track } => {
                if let Err(err) = self.add_adjustment(seconds(at), track.map(project::TrackId)) {
                    self.errors.push(&err);
                }
            }
            E::NestSelected => {
                if let Err(err) = self.nest_selection() {
                    self.errors.push(&err);
                }
            }
            E::SyncBySound => self.start_sync(),
            E::DropTimeline { timeline, at, track } => {
                let result = self.place_timeline(
                    project::SequenceId(timeline),
                    Some(at),
                    track.map(project::TrackId),
                );
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
            E::ClipVolume { clip, gain_db } => {
                let result = self.with_document(|d| {
                    let command = d.project().active().and_then(|seq| {
                        timeline_panel::clip_volume_command(seq, project::ClipId(clip), gain_db)
                    });
                    command.map_or(Ok(()), |command| d.apply(command))
                });
                if let Err(err) = result {
                    self.errors.push(&err);
                }
                self.play_on_with_new_mix();
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
            E::Keyframes { clip, marks, change } => {
                let result = self.with_document(|d| {
                    let command = d
                        .project()
                        .active()
                        .and_then(|seq| timeline_panel::keyframes_command(seq, clip, &marks, change));
                    command.map_or(Ok(()), |command| d.apply(command))
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
    /// (after a change to a clip's sound, speed or length): mixed again
    /// from here at once, by a jump that keeps the sound card open.
    fn play_on_with_new_mix(&mut self) {
        let Some(project) = self.document.as_ref().map(|d| d.project()) else {
            return;
        };
        if let Some(playback) = &mut self.playback {
            let at = playback.position();
            playback.update(project);
            playback.jump(at);
            self.viewer.playhead = at;
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
    fn follow_playback(&mut self, ctx: &egui::Context) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        // Mute, solo, volume and sound changes (and their undo) are heard
        // within a block: a comparison per frame, sent only when changed.
        // What is being dragged on the Audio page (a setting, a fader, a
        // track's line) is heard as it moves.
        if let Some(document) = &self.document {
            let p = document.project();
            let shown = self.audio_page.shown(p);
            let base = shown.as_ref().unwrap_or(p);
            let view = if self.page == Page::Audio {
                &self.audio_page.timeline
            } else {
                &self.timeline
            };
            let lined = view
                .line_being_set()
                .and_then(|line| crate::audio_page::lines::with_line_being_set(base, line));
            playback.follow(lined.as_ref().unwrap_or(base));
        }
        let seq = self.document.as_ref().and_then(|d| d.project().active());
        let (length, rate) = seq.map_or((Time::ZERO, project::Rate::FPS_60), |s| {
            (s.length(), s.settings.frame_rate)
        });
        let at = playback.position();
        let region = seq.and_then(|s| s.region);
        if let Some(back) = crate::looping::wrap(at, region, self.looping, length) {
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
            let Some((read, batch)) = self.importer.next_read() else {
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
                        self.fitted = false;
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
                        self.import_settled = settled.or(self.import_settled.take());
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
            if let Some(batch) = batch.filter(|b| b.files > 1) {
                self.status = format!("Imported {} of {} files", batch.imported(), batch.files);
                if let Some(said) = self.import_settled.take() {
                    self.status = format!("{}. {said}", self.status);
                }
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

    /// Ctrl+S: a snapshot now (cheap: the project shares its data), written
    /// on a worker. It waits there behind an autosave under way, and an
    /// older snapshot never replaces it (`project::document`). The top bar
    /// says "saving…" until it is done; the status line says how it went.
    fn save_now(&mut self) -> diag::Result<()> {
        let mut job = None;
        self.with_document(|d| {
            if d.is_read_only() {
                return d.save(); // refused, with the reason
            }
            job = Some(d.prepare_save());
            Ok(())
        })?;
        let Some(job) = job else {
            return Ok(());
        };
        self.submit_save(job, true);
        self.status = "Saving…".into();
        Ok(())
    }

    fn submit_save(&mut self, job: project::document::SaveJob, asked: bool) {
        let seq = job.journal_seq();
        // Asked for: it runs even while playing. An autosave waits.
        let priority = if asked {
            Priority::Interactive
        } else {
            Priority::Background
        };
        let name = if asked { "save" } else { "autosave" };
        let handle = self
            .jobs
            .submit(PoolKind::Export, priority, name, move |_| job.run());
        self.pending_saves.push(PendingSave { seq, asked, handle });
    }

    /// Hands due saves to a worker and collects finished ones.
    fn autosave(&mut self, now: Instant, idle: bool) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let mut finished = Vec::new();
        self.pending_saves.retain(|save| match save.handle.try_result() {
            Some(result) => {
                finished.push((save.seq, save.asked, result));
                false
            }
            None => true,
        });
        for (seq, asked, result) in finished {
            match document.finish_save(seq, result) {
                Ok(()) => {
                    self.thumbnail_due = Thumbnail::Now;
                    if asked {
                        self.status = "Saved.".into();
                    }
                }
                Err(err) => {
                    if asked {
                        self.status = "The project could not be saved.".into();
                    }
                    self.errors.push(&err);
                }
            }
        }
        if let Some(job) = document.tick(now, idle) {
            self.submit_save(job, false);
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
                        &mut self.settings.keyboard,
                        self.system_keyboard.now(),
                    );
                });
            if !open {
                self.shortcuts_page = None;
            }
            if asked.changed {
                self.settings.shortcuts = self.shortcuts.saved();
            }
            if asked.cheat_sheet {
                self.cheat_sheet
                    .start(ctx, &self.shortcuts, self.shortcuts.layout());
            }
        }
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

    /// The Prune dialog, and the files it takes out of the project.
    fn prune_window(&mut self, ctx: &egui::Context) {
        if !self.prune.is_open() {
            return;
        }
        let Some(document) = &self.document else {
            return;
        };
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .prune
            .ui(ctx, &self.theme, document.project(), &self.relink.missing, &undo);
        if let crate::prune_window::Asked::Prune { media, says } = asked {
            let gone = media.clone();
            match self.with_document(|d| d.apply(Command::RemoveMedia { media })) {
                Ok(()) => {
                    for media in gone {
                        self.waveforms.forget(media);
                        self.pictures.forget(media);
                    }
                    self.status = says;
                }
                Err(err) => self.errors.push(&err),
            }
        }
    }

    /// Package project: the dialog, and what it says when done.
    fn package_window(&mut self, ctx: &egui::Context) {
        let Some(document) = &self.document else {
            return;
        };
        if std::mem::take(&mut self.package_requested) {
            self.package.open(ctx);
        }
        if let Some(into) = self.package_to.take() {
            self.package
                .open_into(&self.jobs, document.project(), document.path(), into);
        }
        if self.package.busy() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        use crate::package_window::Asked;
        match self
            .package
            .ui(ctx, &self.theme, &self.jobs, document.project(), document.path())
        {
            Asked::Nothing => {}
            Asked::Said(says) => self.status = says,
            Asked::Failed(err) => self.errors.push(&err),
        }
    }

    /// The Templates window, and what it asks for.
    fn templates_window(&mut self, ctx: &egui::Context) {
        use crate::templates_window::Asked;
        if !self.templates.open {
            return;
        }
        let Some(document) = &self.document else {
            self.templates.open = false;
            return;
        };
        let dir = project::templates::folder(&self.data_dir);
        match self
            .templates
            .ui(ctx, &self.theme, &self.jobs, document.project(), &dir)
        {
            Asked::Nothing => {}
            Asked::Said(text) => self.status = text,
            Asked::Insert { name, template, spot } => {
                let edit_point = self.edit_point();
                let result = self.with_document(|d| {
                    let Some(seq) = d.project().active() else {
                        return Ok(());
                    };
                    let (sequence, at) = (seq.id, spot.time(edit_point, seq.length()));
                    d.apply(Command::InsertTemplate {
                        sequence,
                        template,
                        at,
                    })
                });
                match result {
                    Ok(()) => {
                        self.fitted = false;
                        self.status = format!(
                            "Inserted the template \"{name}\" {}. {} takes it out again.",
                            spot.said(),
                            self.shortcuts.keys_or_name(Action::Undo)
                        );
                    }
                    Err(err) => {
                        self.templates.failed(&err);
                        self.errors.push(&err);
                    }
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

    /// Starts Sync by sound on the selected clips, or says why it can't.
    fn start_sync(&mut self) {
        let Some(document) = &self.document else { return };
        if let Some(says) = self.sync.start(document.project(), &self.selected, &self.jobs) {
            self.status = says;
        }
    }

    /// The Sync by sound window, and the move it asks for.
    fn sync_window(&mut self, ctx: &egui::Context) {
        if !self.sync.open {
            return;
        }
        let Some(document) = &self.document else {
            self.sync.open = false;
            return;
        };
        self.sync.poll(ctx);
        let undo = self.shortcuts.keys_or_name(Action::Undo);
        let asked = self
            .sync
            .ui(ctx, &self.theme, document.project(), &self.jobs, &undo);
        if let crate::sync_panel::Asked::Move(moves) = asked {
            let count = moves.len();
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::LineUp { sequence, moves })
            });
            match result {
                Ok(()) => {
                    self.status = format!(
                        "Lined up {count} clip{} by sound. {undo} undoes it.",
                        if count == 1 { "" } else { "s" }
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
        // Full screen is for watching: no handles on the picture. A change
        // still being made when it was entered stays on show.
        let being_changed = self.inspector.shown(p);
        let command = self.viewer.ui(
            ui,
            &self.theme,
            being_changed.as_ref().unwrap_or(p),
            &mut quality,
            None,
            height,
            &self.shortcuts,
            self.display_state(true),
            crate::viewer::Stage::default(),
            |_, _, _| {},
        );
        if let Some(command) = command {
            self.run_action(command);
        }
    }

    /// What the Display menu shows as on: the viewer's ways of showing the
    /// picture and the timeline's ways of looking at its tracks.
    fn display_state(&self, full_screen: bool) -> ui_kit::viewer::Display {
        ui_kit::viewer::Display {
            compact_tracks: self.settings.compact_tracks,
            track_divider: !self.settings.one_track_list,
            click_moves_needle: self.settings.click_moves == ui_kit::ClickMoves::EditNeedle,
            rough_cut: self.settings.rough_cut,
            edit_at_playhead: self.settings.edit_at_playhead,
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

    /// The tab on the left edge, beside the toolbar and the viewer (`tab`),
    /// and the media drawer it opens, floating over the page (`page`) so
    /// nothing underneath moves. A click on the page away from it closes it;
    /// it steps aside while a file dragged out of it is over the timeline,
    /// so the whole timeline can take the drop.
    fn media_drawer(&mut self, ui: &mut egui::Ui, page: egui::Rect, tab: egui::Rect) {
        let mut edge = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(tab)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        if ui_kit::drawer::edge_tab(&mut edge, &self.theme, "Media", self.drawer.open).clicked() {
            self.drawer.open = !self.drawer.open;
        }
        let ctx = ui.ctx().clone();
        if self.drawer.open && ui_kit::drawer::clicked_away(&ctx, tab) {
            self.drawer.open = false;
        }
        let rect = ui_kit::drawer::drawer_rect(page);
        let dragging_out = egui::DragAndDrop::has_payload_of_type::<ui_kit::DraggedMedia>(&ctx);
        let keep = ui_kit::drawer::keep_while_dragging(rect, tab);
        let named = self.document.as_ref().and_then(|d| {
            self.drawer
                .dialog(&ctx, &self.theme, d.project(), &self.global_labels.labels)
        });
        let shown = ui_kit::drawer::shown(self.drawer.open, dragging_out, ctx.pointer_latest_pos(), keep);
        if !shown && named.is_none() {
            return;
        }
        let mut picked = named;
        let answer = self.library.answer(crate::media_drawer::ASKER);
        let found = answer.as_ref().map(|a| a.media.as_slice());
        let new_files = match &self.document {
            Some(document) => self.media_folders.new_files(document.project()),
            None => Vec::new(),
        };
        let importer = &self.importer;
        let new_files = crate::new_files::NewFiles {
            files: &new_files,
            checking: &|path| importer.is_reading(path),
        };
        let (drawer, theme, missing) = (&mut self.drawer, &self.theme, &self.relink.missing);
        let (proxies, global_labels) = (&self.proxies, &self.global_labels.labels);
        let proxy = crate::media_drawer::ProxyInfo {
            status: &|media| proxies.status(media),
            mode: self.settings.proxies,
        };
        if let Some(document) = &self.document
            && shown
        {
            picked = ui_kit::drawer::overlay(&ctx, theme, egui::Id::unique("media-drawer"), rect, |ui| {
                let about = crate::media_drawer::About {
                    found,
                    missing,
                    proxy,
                    global: global_labels,
                    new_files,
                };
                drawer.ui(ui, theme, document.project(), &about)
            })
            .inner
            .or(picked);
        }
        use crate::media_drawer::Picked;
        let picked = match picked {
            Some(Picked::Place(media, placement)) => Some((media, placement)),
            Some(Picked::BuildProxy(media)) => {
                self.proxies.build_now(media);
                None
            }
            Some(Picked::DeleteProxy(media)) => {
                self.proxies.delete(&self.jobs, media);
                None
            }
            Some(Picked::AddFound(paths)) => {
                self.importer.add(&self.jobs, &ctx, paths);
                None
            }
            Some(Picked::Run(action)) => {
                self.run_action(action);
                None
            }
            Some(Picked::Dismiss(path)) => {
                self.media_folders.dismiss(path);
                None
            }
            Some(Picked::Edit(command)) => {
                if let Err(err) = self.with_document(|d| d.apply(*command)) {
                    self.errors.push(&err);
                }
                None
            }
            Some(Picked::Everywhere(change)) => {
                match self.global_labels.change(&self.data_dir, change) {
                    Ok(line) => self.status = line,
                    Err(err) => self.errors.push(&err),
                }
                None
            }
            None => None,
        };
        if let Some((media, placement)) = picked {
            use crate::media_drawer::Placement;
            let at = self.edit_point();
            let placed = match placement {
                // As a drop on the first video track does it.
                Placement::Overwrite => self.lay_media(media, at, None),
                Placement::End | Placement::Insert => {
                    let result = self.with_document(|d| {
                        let sequence = d.project().active_sequence;
                        d.apply(match placement {
                            Placement::End => Command::PlaceMedia { sequence, media },
                            _ => Command::InsertMedia { sequence, media, at },
                        })
                    });
                    if let Err(err) = &result {
                        self.errors.push(err);
                    }
                    result.is_ok()
                }
            };
            if placed {
                self.fitted = false;
            }
        }
    }

    /// Lays a file of the project over the timeline at `at`, its picture on
    /// the video track `track` (the first when `None`): what a drop on a
    /// track and the drawer's "Overwrite at the edit point" both do. Its
    /// sound finds free audio tracks, and the status line says where it
    /// went when that is not the usual place. Returns whether it was done.
    fn lay_media(&mut self, media: project::MediaId, at: Time, track: Option<project::TrackId>) -> bool {
        let mut says = None;
        let result = self.with_document(|d| {
            let Some(before) = d.project().active().cloned() else {
                return Ok(());
            };
            d.apply(Command::LayMedia {
                sequence: before.id,
                media,
                at,
                track,
            })?;
            says = d
                .project()
                .active()
                .map(|after| timeline_panel::laid_says(&before, after, media, at));
            Ok(())
        });
        match result {
            Ok(()) => {
                self.status = says.unwrap_or_default();
                true
            }
            Err(err) => {
                self.errors.push(&err);
                false
            }
        }
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

    /// The right-hand panel of the Edit page: the selected clip's settings.
    /// A finished change is applied as one undo step.
    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        if !crate::inspector::shows_a_clip(self.document.as_ref().map(|d| d.project()), &self.selected) {
            egui::ScrollArea::vertical()
                .id_salt("timeline details")
                .auto_shrink([false, true])
                .show(ui, |ui| self.timeline_details(ui));
        }
        self.inspector.on_picture = self.settings.picture_handles;
        // A clip's level line being dragged on the timeline shows here as it moves.
        self.inspector.volume_from_timeline = self
            .timeline
            .level_being_set()
            .map(|(clip, db)| (project::ClipId(clip), timeline_panel::gain_mdb(db)));
        self.inspector.on_picture_tip = format!(
            "Show the clip's outline in the viewer, to move it, resize it by its corners, crop it by its sides and turn it by the round handle.{}",
            match self.shortcuts.keys_for(Action::PictureHandles) {
                keys if keys.is_empty() => String::new(),
                keys => format!(" ({keys})"),
            }
        );
        let Some(document) = &self.document else { return };
        let playhead = self.viewer.playhead;
        // A fixed id: its place in the panel is kept whatever comes before
        // it, so a click in it (an arrow beside a diamond) never scrolls it
        // back to the top.
        // A scroll bar with room of its own, beside the rows instead of over
        // their right end, where the keyframe arrows are (a click there went
        // to the bar and scrolled the panel to the top).
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let commands = egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.inspector
                    .ui(ui, &self.theme, document.project(), &self.selected, playhead)
            })
            .inner;
        // An arrow beside a keyframe diamond moves the playhead.
        if let Some(at) = self.inspector.seek.take() {
            self.go_to(at);
        }
        if self.inspector.on_picture != self.settings.picture_handles {
            self.run_action(Action::PictureHandles);
        }
        for command in commands {
            let speed = matches!(
                command,
                Command::SetSpeed { .. } | Command::SetRamp { .. } | Command::SetSound { .. }
            );
            if let Err(err) = self.with_document(|d| d.apply(command)) {
                self.errors.push(&err);
            }
            if speed {
                // The sound follows the clip's new length or sound.
                self.play_on_with_new_mix();
            }
        }
    }

    /// The inspector with no clip selected: what the timeline is, and its
    /// settings a click away.
    fn timeline_details(&mut self, ui: &mut egui::Ui) {
        let Some(seq) = self.document.as_ref().and_then(|d| d.project().active()) else {
            return;
        };
        let rows = crate::inspector::timeline_rows(seq);
        let summary = rows.first().map(|(_, value)| value.clone()).unwrap_or_default();
        ui.add(
            egui::Label::new(
                egui::RichText::new(&seq.name)
                    .family(ui_kit::fonts::semibold())
                    .size(15.0),
            )
            .truncate(),
        );
        ui.label(
            egui::RichText::new("The timeline")
                .color(self.theme.muted)
                .size(12.0),
        );
        ui.add_space(6.0);
        let theme = self.theme;
        let mut open_settings = false;
        ui_kit::inspector::section(ui, &theme, "Timeline", &summary, false, |ui| {
            for (label, value) in &rows {
                ui.horizontal(|ui| {
                    ui_kit::inspector::label_cell(ui, &theme, label, 86.0);
                    ui.label(egui::RichText::new(value).color(theme.text));
                });
            }
            ui.add_space(4.0);
            open_settings = ui.button("Timeline settings…").clicked();
        });
        if open_settings {
            self.show_timeline_settings = true;
        }
        ui_kit::inspector::note(
            ui,
            &theme,
            "Select a clip to place, crop and colour its picture, or change its speed.",
        );
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
        self.show_settings |= clicks.settings || clicks.usage;
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        if let Some(response) = self.errors.show_inline(ui, &self.theme) {
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
                ErrorResponse::Fix(action) => match import::whats_there_id(&action.command) {
                    Some(id) => {
                        if self.importer.whats_there(&self.jobs, ui.ctx(), id) {
                            self.status = "Importing what is there…".into();
                        }
                    }
                    None => self.status = format!("No handler for {} yet.", action.command),
                },
            }
            ui.add_space(8.0);
        }
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

    /// The Edit page until the viewer and the timeline widget arrive: import,
    /// and a plain list of what is on the timeline.
    fn edit_page(&mut self, ui: &mut egui::Ui) {
        // The toolbar and the viewer sit beside the media drawer's tab; the
        // timeline below runs from the page's left edge to its right.
        let page = ui.available_rect_before_wrap();
        let mut viewer_command = None;
        let mut picture_change = None;
        let mut master = ui_kit::master::MasterResponse::default();
        let mut timeline_events = Vec::new();
        let mut toolbar_click = None;
        let mut tabs_events = Vec::new();
        let shortcut = self.shortcuts.keys_for(Action::Import);
        let beside = egui::UiBuilder::new()
            .max_rect(ui_kit::drawer::beside_tab(page))
            .layout(egui::Layout::top_down(egui::Align::Min));
        ui.scope_builder(beside, |ui| ui.horizontal(|ui| {
            let button = ui_kit::accent_button(ui, &self.theme, "Import media…").on_hover_text(&shortcut);
            if button.clicked() {
                self.run_action(Action::Import);
            }
            if ui.button("Timeline settings…").clicked() {
                self.show_timeline_settings = true;
            }
            if ui
                .button("Templates…")
                .on_hover_text(self.shortcuts.tip(Action::Templates))
                .clicked()
            {
                self.run_action(Action::Templates);
            }
            if let Some(document) = &self.document
                && !self.relink.missing.is_empty()
            {
                ui.add_space(16.0);
                ui_kit::status_dot(ui, ui_kit::status::WARNING);
                let text = crate::relink::summary(document.project(), &self.relink.missing);
                ui.label(egui::RichText::new(text).color(self.theme.text_secondary));
                match self.relink.searching() {
                    Some(words) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(words).color(self.theme.muted));
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
                        "Checking {busy} file{}…",
                        if busy == 1 { "" } else { "s" }
                    ))
                    .color(self.theme.text_secondary),
                );
            }
        }));
        ui.add_space(12.0);
        // The inspector sits beside the viewer, as high as it is; the
        // timeline below runs the whole width of the page.
        let shared_height = ui.available_height();
        let (least_viewer, least_timeline) = (ui_kit::splitter::MIN_VIEWER, ui_kit::splitter::MIN_TIMELINE);
        let viewer_height =
            self.settings
                .viewer_split
                .top_height(shared_height, least_viewer, least_timeline);
        let top = ui_kit::drawer::beside_tab(ui.available_rect_before_wrap());
        let top = egui::Rect::from_min_size(top.min, egui::vec2(top.width(), viewer_height));
        let inspector_rect = egui::Rect::from_min_max(
            egui::pos2(top.right() - layout::RIGHT_PANEL_WIDTH, top.top()),
            top.right_bottom(),
        );
        let viewer_rect = egui::Rect::from_min_max(
            top.min,
            egui::pos2(inspector_rect.left() - INSPECTOR_GAP, top.bottom()),
        );
        if self.document.is_some() {
            let mut panel = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inspector_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            egui::Frame::new()
                .fill(self.theme.panel)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(&mut panel, |ui| {
                    // Everything stays inside the panel: what doesn't fit
                    // scrolls, and nothing is drawn past its edge.
                    let inner = inspector_rect.shrink2(egui::vec2(12.0, 10.0));
                    ui.set_min_size(inner.size());
                    ui.set_max_size(inner.size());
                    ui.set_clip_rect(ui.clip_rect().intersect(inner));
                    self.inspector_panel(ui);
                });
        }
        self.media_drawer(
            ui,
            page,
            ui_kit::drawer::tab_rect(page, top.bottom() - page.top()),
        );
        let Some(document) = &self.document else { return };
        let p = document.project();
        let Some(seq) = p.active() else { return };
        let rate = seq.settings.frame_rate;
        // A removed track's height is forgotten.
        self.timeline.heights.keep_only(&timeline_panel::track_ids(seq));
        if seq.length() == project::Time::ZERO {
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(viewer_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            left.add_space(40.0);
            left.vertical_centered(|ui| {
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
            // The page's top part is taken, so the tabs (and the status
            // line) come under it, clear of the drawer's tab: an empty
            // timeline still shows the others, to go back to.
            ui.allocate_rect(top, egui::Sense::hover());
            ui.add_space(8.0);
            let new_tip = self.shortcuts.tip(Action::NewTimeline);
            let shown_tabs = timelines::tabs(p);
            ui.horizontal(|ui| {
                tabs_events = self.tabs.show(ui, &self.theme, &shown_tabs, &new_tip);
            });
        } else {
            let quality_before = self.settings.preview_quality;
            let needle = (!self.settings.edit_at_playhead).then_some(seq.edit_needle);
            // A value being dragged in the inspector, the picture being
            // dragged in the viewer, or keyframe marks being dragged on the
            // timeline, are seen at once.
            let being_changed = self.inspector.shown(p);
            let mut being_changed =
                timeline_panel::keyframes_moved(&self.timeline, being_changed.as_ref().unwrap_or(p))
                    .or(being_changed);
            // While a side of the picture is dragged, the whole picture is
            // shown, the handles veiling what is cut off.
            if let Some(clip) = self.picture_handles.cropping() {
                let whole = crate::picture_handles::uncropped(being_changed.as_ref().unwrap_or(p), clip);
                being_changed = whole.or(being_changed);
            }
            // A clip being slipped: the viewer shows its first frame as it
            // would be (and the Source its file there), live; the playhead
            // goes back when the drag ends.
            match timeline_panel::slipped(&self.timeline, being_changed.as_ref().unwrap_or(p)) {
                Some((moved, clip)) => {
                    let start = moved
                        .active()
                        .and_then(|s| s.tracks().flat_map(|t| t.clips.iter()).find(|c| c.id == clip))
                        .map(|c| c.start);
                    if let Some(start) = start {
                        self.slip_playhead.get_or_insert(self.viewer.playhead);
                        self.viewer.playhead = start;
                    }
                    being_changed = Some(moved);
                }
                None => {
                    if let Some(back) = self.slip_playhead.take() {
                        self.viewer.playhead = back;
                    }
                }
            }
            let playhead = self.viewer.playhead;
            let snapping = self.timeline.snapping;
            let on_picture = self.settings.picture_handles;
            let display_state = self.display_state(false);
            let (handles, inspector) = (&mut self.picture_handles, &mut self.inspector);
            let (theme, selected) = (&self.theme, &self.selected);
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(viewer_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            // Split view: the selected clip raw, beside the timeline's picture.
            let (split, away) = (self.display.split(), self.display.away());
            let display = &mut self.display;
            let shown = being_changed.as_ref().unwrap_or(p);
            let mut source = |ui: &mut egui::Ui, size: egui::Vec2| {
                display.source_ui(ui, theme, size, shown, selected, playhead, quality_before);
            };
            let stage = crate::viewer::Stage {
                source: split.then_some(&mut source as &mut crate::viewer::DrawSource),
                away,
            };
            viewer_command = self.viewer.ui(
                &mut left,
                theme,
                being_changed.as_ref().unwrap_or(p),
                &mut self.settings.preview_quality,
                needle,
                viewer_height,
                &self.shortcuts,
                display_state,
                stage,
                // The selected clip's picture can be taken hold of.
                // Only when handles on the picture are switched on.
                |ui, frame, response| {
                    if on_picture {
                        picture_change = handles.ui(
                            ui, theme, frame, response, p, selected, playhead, inspector, snapping,
                        );
                    }
                },
            );
            // The viewer and the inspector take their height of the page;
            // the handle and the timeline follow, across its whole width.
            ui.allocate_rect(top, egui::Sense::hover());
            // The quality and the handle's place are settings: the
            // settings keeper writes them once the pointer is let go.
            ui_kit::splitter::splitter(
                ui,
                &self.theme,
                &mut self.settings.viewer_split,
                shared_height,
                least_viewer,
                least_timeline,
            );
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
            let new_tip = self.shortcuts.tip(Action::NewTimeline);
            let shown_tabs = timelines::tabs(p);
            ui.horizontal(|ui| {
                toolbar_click = ui_kit::timeline::toolbar(
                    ui,
                    &self.theme,
                    &self.timeline,
                    [
                        &tips[0], &tips[1], &tips[2], &tips[3], &tips[4], &tips[5], &tips[6], &tips[7],
                        &tips[8],
                    ],
                );
                ui.add_space(16.0);
                tabs_events = self.tabs.show(ui, &self.theme, &shown_tabs, &new_tip);
            });
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
            // Sound being set in the inspector shows on its clips at once.
            timeline_panel::show_sounds(&mut drawn, self.inspector.sounds_being_set());
            crate::audio_page::lines::fill(&mut drawn.audio, seq, self.lines.kind);
            self.timeline.lines = self.lines_on.then_some(self.lines);
            self.timeline.rough_cut = self.settings.rough_cut;
            self.timeline.compact = self.settings.compact_tracks;
            self.timeline.divider = !self.settings.one_track_list;
            self.timeline.click_moves = self.settings.click_moves;
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
            let roles = crate::audio_page::roles::offered();
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
                spans: &[],
                follow: self.playback.is_some(),
                cache: ui_kit::timeline::CacheBar {
                    ranges: &drawn.cache,
                    about: timeline_panel::CACHE_BAR_ABOUT,
                },
                keyframe_eases: &timeline_panel::EASES,
                roles: &roles,
            };
            let status_height = if self.status.is_empty() { 0.0 } else { 24.0 };
            let height = (ui.available_height() - status_height).max(160.0);
            let size = egui::vec2(width - layout::MASTER_WIDTH - 8.0, height);
            let levels = self.playback.as_ref().map(|p| {
                let l = p.levels();
                ui_kit::master::MeterLevels {
                    peak_db: l.peak_db,
                    true_peak_db: l.true_peak_db,
                    true_peak_max: l.true_peak_max,
                    momentary: l.momentary,
                    short_term: l.short_term,
                    integrated: l.integrated,
                }
            });
            let gain = seq.master_gain_db;
            ui.horizontal_top(|ui| {
                timeline_events = self.timeline.show(ui, &self.theme, &data, size);
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(layout::MASTER_WIDTH, height), egui::Sense::hover());
                let mix = ui_kit::master::Mix {
                    levels,
                    gain_db: gain,
                    target: self.settings.loudness_target,
                    tip: "The whole mix: drag to change its volume, double-click for 0 dB. The lines on the channels are their true peaks; the third lane is the loudness over 3 s, with the loudness aimed for across it (right-click to choose). M and S are the loudness over 0.4 s and 3 s, I since playback started, TP the highest true peak.",
                };
                master = self.master.show(ui, &self.theme, rect, mix);
            });
        }
        if master.reset
            && let Some(playback) = &self.playback
        {
            playback.reset_loudness();
        }
        if let Some(target) = master.target {
            self.settings.loudness_target = target;
        }
        if let Some(gain_db) = master.gain_db {
            let result = self.with_document(|d| {
                let sequence = d.project().active_sequence;
                d.apply(Command::SetMasterGain { sequence, gain_db })
            });
            if let Err(err) = result {
                self.errors.push(&err);
            }
        }
        for event in timeline_events {
            self.timeline_event(event);
        }
        for event in tabs_events {
            self.tabs_event(event);
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
        if let Some(command) = viewer_command {
            self.run_action(command);
        }
        // The picture was let go of in the viewer: one undo step.
        if let Some(command) = picture_change
            && let Err(err) = self.with_document(|d| d.apply(command))
        {
            self.errors.push(&err);
        }
        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&self.status)
                    .color(self.theme.text_secondary.gamma_multiply(self.status_strength)),
            );
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
        // Measured when Settings opens, and again while it is open as
        // proxies are built or deleted, so the Cache line stays true.
        let changes = self.proxies.changes();
        if self.show_settings && (!self.settings_were_open || changes != self.cache_seen) {
            self.cache_seen = changes;
            self.measure_cache();
        }
        self.settings_were_open = self.show_settings;
        let has_project = self.document.is_some();
        let mut open = self.show_settings;
        let before = self.settings.clone();
        let mut found = self.settings_found.take();
        use crate::settings_sections::Section;
        // Below the top bar at the right, under the gear that opens it; held
        // to the screen, with what doesn't fit scrolling inside.
        let area = ui_kit::layout::window_area(ctx.content_rect());
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(area.right_top())
            .default_width(ui_kit::layout::SETTINGS_WIDTH)
            .default_height(area.height())
            .max_height(area.height())
            .constrain_to(area)
            .vscroll(true)
            .show(ctx, |ui| {
                settings_heading(ui, &self.theme, Section::Look, &mut found);
                egui::ComboBox::from_label("Theme")
                    .selected_text(self.settings.theme.label())
                    .show_ui(ui, |ui| {
                        for theme in ui_kit::ThemeName::ALL {
                            ui.selectable_value(&mut self.settings.theme, theme, theme.label());
                        }
                    });
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Editing, &mut found);
                ui.checkbox(&mut self.settings.edit_at_playhead, "Edit at the playhead")
                    .on_hover_text("Merge the red edit needle into the playhead.");
                click_moves_row(ui, &mut self.settings.click_moves, self.settings.edit_at_playhead);
                ui.checkbox(&mut self.settings.cut_closes_gap, "Cut closes the gap").on_hover_text(
                    "Cut takes the clips out and moves everything after them back on every track, as \"Delete and close the gap\" does. Off: it leaves the gap.",
                );
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Audio, &mut found);
                ui_kit::master::target_row(ui, &mut self.settings.loudness_target);
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Resources, &mut found);
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
                settings_heading(ui, &self.theme, Section::Proxies, &mut found);
                ui.horizontal(|ui| {
                    for mode in cache::proxy::ProxyMode::ALL {
                        ui.radio_value(&mut self.settings.proxies, mode, mode.label())
                            .on_hover_text(mode.explained());
                    }
                });
                ui.label(egui::RichText::new(self.settings.proxies.explained()).color(self.theme.muted));
                ui.add_space(4.0);
                ui.label("When jumping in a file without a proxy");
                ui.horizontal(|ui| {
                    for choice in crate::settings::JumpPicture::ALL {
                        ui.radio_value(&mut self.settings.jump_picture, choice, choice.label())
                            .on_hover_text(choice.explained());
                    }
                });
                ui.label(egui::RichText::new(self.settings.jump_picture.explained()).color(self.theme.muted));
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Library, &mut found);
                ui.horizontal(|ui| {
                    for choice in crate::settings::LibraryMarks::ALL {
                        ui.radio_value(&mut self.settings.library_marks, choice, choice.label())
                            .on_hover_text(choice.explained());
                    }
                });
                ui.label(
                    egui::RichText::new(self.settings.library_marks.explained()).color(self.theme.muted),
                );
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Cache, &mut found);
                self.cache_cleanup.rows(ui, &self.theme, has_project);
                ui.add_space(8.0);
                settings_heading(ui, &self.theme, Section::Graphics, &mut found);
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
        self.settings_found = found.filter(|_| open);
        let project_proxies = self.project_proxies();
        let shelf = self.proxies.shelf();
        if self
            .cache_cleanup
            .question(ctx, &self.theme, &self.jobs, &shelf, project_proxies)
        {
            self.proxies.clearing();
        }
        if self.settings != before {
            if self.settings.theme != before.theme {
                self.theme = self.settings.theme.theme();
                self.theme.apply(ctx);
            }
            self.proxies.set_mode(self.settings.proxies);
            // On asks for a proxy for every video file: those cleared on
            // purpose are built again (Auto leaves them cleared).
            if self.settings.proxies == cache::proxy::ProxyMode::On && before.proxies != self.settings.proxies
            {
                let back = self.proxies.switched_on();
                if back > 0 {
                    self.status = format!(
                        "Proxies On: building the {back} cleared from the cache again, one at a time"
                    );
                }
            }
            if self.settings.library_marks != before.library_marks {
                self.library_marks_moved();
            }
            let key_frame_first = self.settings.jump_picture.key_frame_first();
            self.viewer.set_key_frame_first(key_frame_first);
            self.display.set_key_frame_first(key_frame_first);
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

    fn search_window(&mut self, ctx: &egui::Context) {
        use crate::search::Hit;
        if !self.show_search {
            return;
        }
        let project = self.document.as_ref().map(Document::project);
        let seq = project.and_then(|p| p.active());
        let fps = seq.map_or(60.0, |s| s.settings.frame_rate.as_f64());
        // The files the index found for these words, once it has answered them.
        let files: Option<Vec<(project::MediaId, String)>> = self
            .library
            .answer(crate::search::ASKER)
            .filter(|a| a.query.text == self.search_text.trim())
            .zip(project)
            .map(|(answer, p)| {
                answer
                    .media
                    .iter()
                    .filter_map(|id| p.media.get(id))
                    .map(|m| (m.id, library::file_name(&m.path)))
                    .collect()
            });
        let open_timeline = seq.map(|s| s.id);
        let timelines: Vec<(project::SequenceId, String)> = project
            .map(|p| {
                p.sequences
                    .iter()
                    .filter(|s| Some(s.id) != open_timeline)
                    .map(|s| (s.id, s.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let hits = crate::search::results(
            &self.search_text,
            &self.shortcuts,
            seq,
            &timelines,
            |clip| {
                project
                    .and_then(|p| p.media.get(&clip.media))
                    .and_then(|m| m.path.file_name())
                    .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned())
            },
            files.as_deref(),
        );
        let answer = crate::search::palette(ctx, &self.theme, &mut self.search_text, &hits, fps);
        let chosen = match answer {
            None => return,
            Some(crate::search::Answer::Closed) => None,
            Some(crate::search::Answer::Chosen(hit)) => Some(hit),
        };
        self.show_search = false;
        match chosen {
            Some(Hit::Command { action, .. }) => self.run_action(action),
            Some(Hit::Label { at, .. }) => self.seek(at),
            Some(Hit::Clip { id, at, .. }) => {
                self.selected = vec![id];
                self.seek(at);
            }
            Some(Hit::File { name, .. }) => {
                self.page = Page::Edit;
                self.drawer.open = true;
                self.drawer.tab = ui_kit::drawer::DrawerTab::Media;
                self.drawer.filter = name;
            }
            Some(Hit::Settings) => self.show_settings = true,
            Some(Hit::Setting(section)) => {
                self.show_settings = true;
                self.settings_found = Some((section, 3));
            }
            Some(Hit::Timeline { id, .. }) => {
                self.page = Page::Edit;
                self.open_timeline(id);
            }
            Some(Hit::TimelineSettings) => self.show_timeline_settings = true,
            Some(Hit::Shortcuts) => self.run_action(Action::KeyboardShortcuts),
            None => {}
        }
    }

    /// The viewer's own window, while it is open: declared every frame (or
    /// it closes), handed the picture, and what was done there run here.
    fn viewer_window(&mut self, ctx: &egui::Context) {
        let news = self
            .display
            .show_window(ctx, &mut self.viewer, self.theme, &self.shortcuts);
        for action in news.actions {
            self.run_action(action);
        }
        if news.closed && self.display.window_open() {
            self.run_action(Action::ViewerWindow);
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
                let ready = (self.importer.busy() == 0
                    && self.viewer.settled()
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
                    && !self.sync_when_ready
                    && (self.page != Page::Export || self.document.is_none() || self.export_page.settled())
                    && (self.page != Page::Audio
                        || self.document.is_none()
                        || (self.audio_start.tracks.is_none() && self.audio_page.clarity.settled())))
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
                // A project being made for the test (`--audio-tracks`) is
                // set-up, not use: counting starts a few frames after it.
                let setting_up = self.audio_start.tracks.is_some()
                    || self.audio_demo_made.is_some_and(|made| self.frames < made + 5);
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
        let (strength, again) = self.status_fade.follow(&mut self.status, now);
        self.status_strength = strength;
        if let Some(again) = again {
            ctx.request_repaint_after(again);
        }
        self.graphics.frame(&ctx, self.frames);
        if self.frames == 2 {
            for action in std::mem::take(&mut self.startup_actions) {
                self.run_action(action);
            }
        }
        // A keyboard switched on the system: keys are written for it from now
        // on (the page draws it too). The window repaints twice a second, so
        // a switch shows within a moment even without input.
        if self.system_keyboard.poll() {
            self.shortcuts
                .set_layout(self.settings.keyboard.resolve(self.system_keyboard.now()));
        }
        // While the shortcuts page waits for new keys, what is pressed is
        // its answer, not a command.
        // A file chooser that could not open says why.
        if let Some(problem) = ui_kit::file_dialog::take_problem(&ctx) {
            self.errors.push(&problem);
        }
        let listening = self.shortcuts_page.as_ref().is_some_and(|p| p.listening());
        if !listening {
            for action in self.shortcuts.pressed(&ctx) {
                self.run_action(action);
            }
        }
        if !self.actions_after_pointer.is_empty()
            && self.pointer.is_none_or(|p| self.frames >= p.settled_at())
        {
            for action in std::mem::take(&mut self.actions_after_pointer) {
                self.run_action(action);
            }
        }
        if let Some(text) = self.clipboard_text.take() {
            ctx.copy_text(text);
        }
        self.imports(&ctx);
        self.follow_open_timeline();
        if self.select_first_clip
            && self.importer.busy() == 0
            && let Some(clip) = self
                .document
                .as_ref()
                .and_then(|d| d.project().active())
                .and_then(timeline_panel::first_clip)
        {
            self.select_first_clip = false;
            self.selected = vec![clip];
        }
        if self.sync_when_ready
            && self.importer.busy() == 0
            && let Some(seq) = self.document.as_ref().and_then(|d| d.project().active())
            && seq.tracks().any(|t| !t.clips.is_empty())
        {
            self.sync_when_ready = false;
            self.selected = seq.tracks().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
            self.run_action(Action::SyncBySound);
        }
        if self.play_when_ready && self.importer.busy() == 0 && self.document.is_some() {
            self.play_when_ready = false;
            self.run_action(Action::PlayPause);
        }
        self.follow_playback(&ctx);
        self.waveforms.poll(&ctx, now);
        self.pictures.poll(&ctx, now);
        let project = self.document.as_ref().map(Document::project);
        if let Some(err) = self.proxies.poll(&self.jobs, &ctx, project, now) {
            self.errors.push(&err);
        }
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
        self.audio_demo();
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
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(self.theme.bg)
                        .inner_margin(egui::Margin::same(16)),
                )
                .show(ui, |ui| self.page(ui));
        }

        self.viewer_window(&ctx);
        self.settings_window(&ctx);
        self.timeline_settings_window(&ctx);
        self.search_window(&ctx);
        self.silence_window(&ctx);
        self.sync_window(&ctx);
        self.label_window(&ctx);
        self.versions_window(&ctx);
        self.templates_window(&ctx);
        self.prune_window(&ctx);
        self.package_window(&ctx);
        self.shortcuts_window(&ctx);
        let unfinished = self.export_page.unfinished();
        if self.close_request && unfinished.is_some() {
            self.close_request = false;
            self.close_prompt.ask();
        }
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
        // Keep the resource readout and autosave ticking without input.
        ctx.request_repaint_after(Duration::from_millis(500));
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

/// A full save running on a worker.
struct PendingSave {
    /// The newest journal entry it includes.
    seq: u64,
    /// Asked for (Ctrl+S): its end is said in the status line.
    asked: bool,
    handle: JobHandle<()>,
}

/// Whether the launcher's picture of the project needs writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Thumbnail {
    No,
    /// On opening: only for a project that has none yet.
    IfMissing,
    Now,
}

/// A section's heading in Settings; the one search asked for (`found`,
/// with the frames left to scroll to it) is in the accent colour and
/// scrolled to the top.
fn settings_heading(
    ui: &mut egui::Ui,
    theme: &ui_kit::Theme,
    section: crate::settings_sections::Section,
    found: &mut Option<(crate::settings_sections::Section, u8)>,
) {
    let mut text = egui::RichText::new(section.name()).heading();
    let Some((_, frames)) = found.as_mut().filter(|(s, _)| *s == section) else {
        ui.label(text);
        return;
    };
    text = text.color(theme.accent);
    let heading = ui.label(text);
    if *frames > 0 {
        *frames -= 1;
        heading.scroll_to_me(Some(egui::Align::TOP));
        ui.ctx().request_repaint();
    }
}

fn click_moves_row(ui: &mut egui::Ui, moves: &mut ui_kit::ClickMoves, edit_at_playhead: bool) {
    ui.horizontal(|ui| {
        ui.label("Clicking the timeline moves");
        ui.add_enabled_ui(!edit_at_playhead, |ui| {
            ui.radio_value(moves, ui_kit::ClickMoves::Playhead, "the playhead");
            ui.radio_value(moves, ui_kit::ClickMoves::EditNeedle, "the edit needle");
        })
        .response
        .on_hover_text(if edit_at_playhead {
            "With Edit at the playhead there is no separate needle: a click moves the playhead"
        } else {
            "A click or drag in the timeline's ruler moves this one; the other is moved by dragging its handle"
        });
    });
}

/// What a page shows when it has nothing to show: its name and one line.
/// Without a project every page waits for one; with one, only the Color
/// page, whose tools are still to come.
fn placeholder(page: Page, has_project: bool) -> (&'static str, &'static str) {
    if !has_project {
        return (
            page.label(),
            "Open a project, or start a new one, in the launcher; then import your recordings here.",
        );
    }
    match page {
        Page::Color => ("Color", "Colour tools arrive in a later update."),
        // The other pages always draw themselves when a project is open.
        _ => (page.label(), ""),
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
/// The fix button's command that splits at the playhead when the edit
/// needle found nothing to split.
const SPLIT_AT_PLAYHEAD: &str = "split.at-playhead";

pub(crate) fn seconds(s: f64) -> Time {
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
    /// Words to start with in the drawer's "Find a file".
    pub find: Option<String>,
    /// Folders watched for new recordings besides the launcher's media folders.
    pub watch_folders: Vec<PathBuf>,
    /// Start on the Export page.
    pub export_page: bool,
    /// Start on the Export page with its queue filled (for screenshots).
    pub export_queue: bool,
    /// Ask about closing once an export is unfinished, as if the window's
    /// close button had been pressed (for screenshots).
    pub close_request: bool,
    /// Start on this page.
    pub page: Option<Page>,
    /// Open this tab of the Library page (a type or a list's name).
    pub library_tab: Option<String>,
    /// Show the file with this name in the Library page's detail panel.
    pub library_select: Option<String>,
    /// Start with the viewer full screen.
    pub full_screen: bool,
    /// Select the first clip of the top video track that has one, once
    /// imports are done.
    pub select_first_clip: bool,
    /// Select every clip once imports are done, and run Sync by sound.
    pub sync_by_sound: bool,
    /// Commands to run once open, by name.
    pub actions: Vec<String>,
    /// Commands to run once the scripted pointer has done its part, by name.
    pub then_actions: Vec<String>,
    /// Convert the viewer's colour on the CPU even when the GPU could.
    pub cpu_colour: bool,
    /// Stop background reading at this share, for screenshots of it in progress.
    pub hold_reading: Option<f32>,
    /// Stop proxy builds at this share, for screenshots of one in progress.
    pub hold_proxy: Option<f32>,
    /// Open Package project with this folder chosen, for screenshots.
    pub package_to: Option<PathBuf>,
    /// Also start packaging and stop copying at this share.
    pub hold_package: Option<f32>,
    /// What the Audio page starts with.
    pub audio: crate::audio_page::Start,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(placeholder(Page::Color, true).1.contains("later update"));
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
