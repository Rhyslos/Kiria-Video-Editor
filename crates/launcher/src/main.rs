//! The small program that opens first.
//!
//! With no arguments it shows the project browser: a folder tree, the
//! project grid and the version panel. Opening a project starts the editor
//! and closes the browser. With a project file as argument it starts the
//! editor with that project straight away. Either way it checks GitHub for a
//! newer version in the background and, with automatic updates on, downloads
//! and verifies it; installing it arrives in Phase 3.
//!
//! Usage: `launcher --self-test` (answers and stops), `launcher --update-step <report file>` (one start's worth of updating without a window, see `drill.rs`), or `launcher [project file] [--no-update-check] [--smoke-test | --screenshot <file.png>] [--demo <count>] [--update-window <available | downloading | ready>] [--demo-crash] [--delete-dialog] [--graphics-warning]`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod browser;
mod cards;
mod delete;
mod delete_dialog;
mod demo;
mod drill;
mod editor_start;
mod files;
mod new_project;
mod prefs;
mod recovery;
mod run_mode;
mod thumbnails;
mod updates;
mod work;

use browser::{Browser, RunReport, Start};
use run_mode::{Options, RunMode};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use updater::{CheckResult, UpdatePrefs};

pub(crate) const VERSION: &str = brand::VERSION;

fn main() -> ExitCode {
    // Asked by an older launcher before it lets this one replace it: answer and stop.
    if std::env::args().nth(1).as_deref() == Some("--self-test") {
        println!("{} launcher {VERSION}", brand::APP_NAME);
        return ExitCode::SUCCESS;
    }
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let (dirs, _log) = match diag::start_helper("launcher") {
        Ok((dirs, log)) => (Some(dirs), Some(log)),
        Err(err) => {
            eprintln!("{}", err.report());
            (None, None)
        }
    };
    tracing::info!(version = VERSION, "launcher started");

    if let Some(report) = &options.update_step {
        return drill::step(report, dirs.as_ref().map(|d| d.data.as_path()));
    }

    // A verified update downloaded last time is installed before anything opens.
    let mut start_notes = Vec::new();
    if let Some(dirs) = &dirs
        && let Some(notice) = updater::health::RollbackNotice::take(&dirs.data)
    {
        start_notes.push(Note::RolledBack(notice));
    }
    if options.demo.is_none()
        && options.mode == RunMode::Normal
        && let Some(dirs) = &dirs
    {
        match install_waiting_update(&dirs.data) {
            Ok(Installed::Nothing) => {}
            Ok(Installed::Version(version)) => start_notes.push(Note::Updated(version)),
            Ok(Installed::RestartNeeded) => return restart_launcher(true),
            Err(err) => {
                tracing::warn!("{}", err.report());
                start_notes.push(Note::Problem(err));
            }
        }
    }

    if let Some(project) = &options.project {
        return open_directly(
            project,
            dirs.as_ref().map(|d| d.data.as_path()),
            options.check_updates,
        );
    }

    // The Graphics setting and the start marker live in the real data folder,
    // also for a demo, which only reads them.
    let graphics_dir = dirs.as_ref().map_or_else(std::env::temp_dir, |d| d.data.clone());
    let data_dir = match options.demo {
        Some(count) => match demo::prepare(count, options.demo_crash) {
            Ok(dir) => dir,
            Err(err) => {
                eprintln!("{}", err.report());
                return ExitCode::FAILURE;
            }
        },
        None => graphics_dir.clone(),
    };
    browse(data_dir, &graphics_dir, options, start_notes)
}

/// What happened to a waiting update on start.
pub(crate) enum Installed {
    Nothing,
    /// Installed; the editor that opens next is the new version.
    Version(String),
    /// The launcher itself changed: start the new one and quit.
    RestartNeeded,
}

/// Something to tell the person once the window is open.
#[derive(Debug)]
pub(crate) enum Note {
    Updated(String),
    /// A version failed to start and the previous one is back.
    RolledBack(updater::health::RollbackNotice),
    Problem(diag::AppError),
}

pub(crate) fn install_waiting_update(data_dir: &Path) -> diag::Result<Installed> {
    use updater::apply::{self, InstallKind};
    let root = install_root();
    let kind = InstallKind::detect(&root);
    let current = match &kind {
        InstallKind::Folders(layout) => layout.current_version().unwrap_or_else(|| VERSION.to_owned()),
        InstallKind::AppImage(_) => VERSION.to_owned(),
        InstallKind::Development => return Ok(Installed::Nothing),
    };
    if let InstallKind::Folders(layout) = &kind {
        apply::remove_old_launcher(layout);
    }
    let downloads = data_dir.join("downloads");
    let mut installed = None;
    let mut launcher_needed = false;
    let bad = UpdatePrefs::load(data_dir).bad_versions;
    if let Some(ready) = apply::pending(&downloads, &current, &bad) {
        tracing::info!(from = %current, to = %ready.version, "installing an update");
        apply::install(&kind, &ready)?;
        launcher_needed = apply::needs_newer_launcher(&ready, VERSION);
        // Earlier versions stay until the new one has started properly (health::after_start).
        apply::forget_download(&downloads, &ready.version);
        installed = Some(ready.version);
    }
    if let InstallKind::Folders(layout) = &kind {
        apply::refresh_icon(layout);
    }
    let restart = match &kind {
        // Normally the launcher is replaced only after the new version has
        // started properly (watch_editor); a version that needs it goes first.
        InstallKind::Folders(layout) if launcher_needed => apply::replace_launcher(layout, VERSION)?,
        InstallKind::Folders(_) => false,
        // The AppImage file was replaced; the running copy is the old one.
        InstallKind::AppImage(_) => installed.is_some(),
        InstallKind::Development => false,
    };
    Ok(match (restart, installed) {
        (true, _) => Installed::RestartNeeded,
        (false, Some(version)) => Installed::Version(version),
        (false, None) => Installed::Nothing,
    })
}

/// The version in use, which updates are compared against: the installed
/// one, even while this launcher program is still the older one.
pub(crate) fn version_in_use() -> String {
    updater::apply::InstallKind::detect(&install_root()).version_in_use(VERSION)
}

/// The folder the launcher is installed in.
fn install_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Starts the (new) launcher, with this run's arguments if `same_args`, and ends this one.
pub(crate) fn restart_launcher(same_args: bool) -> ExitCode {
    let program = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| install_root().join(updater::apply::launcher_file_name()));
    tracing::info!(launcher = %program.display(), "restarting with the new version");
    let mut command = std::process::Command::new(&program);
    if same_args {
        command.args(std::env::args_os().skip(1));
    }
    match command.spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("restarting failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Shows the project browser until a project is opened or the window is closed.
fn browse(
    data_dir: std::path::PathBuf,
    graphics_dir: &Path,
    options: Options,
    mut notes: Vec<Note>,
) -> ExitCode {
    let report = Arc::new(Mutex::new(RunReport::default()));
    let handoff = Arc::new(Mutex::new(None));
    let restart = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let editor = Arc::new(Mutex::new(None));
    let automated = options.mode != RunMode::Normal;
    // Which graphics system to try, and whether the last start with Vulkan failed.
    // Test runs and demos only read: no marker is written and no setting changes.
    let run = if automated || options.demo.is_some() {
        graphics::RunKind::Automated
    } else {
        graphics::RunKind::Normal
    };
    let (mut graphics_start, problems) = graphics::prepare(graphics::Program::Launcher, graphics_dir, run);
    notes.extend(problems.into_iter().map(Note::Problem));
    if options.graphics_warning {
        graphics_start = graphics_start.with_warning_preview();
    }
    let shared = editor_settings(&data_dir);
    let start = Start {
        theme: shared.theme,
        shortcuts: shared.shortcuts,
        data_dir: data_dir.clone(),
        check_updates: options.check_updates,
        launch_editor: !automated && options.demo.is_none(),
        mode: options.mode,
        report: Arc::clone(&report),
        handoff: Arc::clone(&handoff),
        notes,
        restart: Arc::clone(&restart),
        update_preview: options.update_window,
        editor: Arc::clone(&editor),
        // Sample projects never reach the person's own recycle bin.
        bin: match options.demo {
            Some(_) => demo::bin(&data_dir),
            None => delete::Bin::System,
        },
        delete_preview: options.delete_dialog && options.demo.is_some(),
        settings_at: options.settings.clone(),
    };
    let window = || eframe::NativeOptions {
        viewport: ui_kit::icon::with_window_icon(egui::ViewportBuilder::default())
            .with_title(brand::APP_NAME)
            .with_inner_size(options.window_size.unwrap_or([1180.0, 760.0]))
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    let result = graphics::open_window(
        &graphics_start,
        brand::APP_NAME,
        window,
        start,
        |cc, start, opened| Box::new(Browser::new(cc, start, opened)),
    );

    let mut code = ExitCode::SUCCESS;
    if let Err(err) = result {
        tracing::error!("{}", err.report());
        eprintln!("{}", err.report());
        code = ExitCode::FAILURE;
    }
    // A download that was running when the window closed finishes quietly.
    let updates = handoff.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(updates) = updates {
        updates.finish_download();
    }
    // The editor was started: make sure this version actually opens.
    let child = editor.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(mut child) = child
        && watch_editor(&data_dir, &mut child).rolled_back
    {
        restart_launcher(false);
    }
    if restart.load(std::sync::atomic::Ordering::Relaxed) {
        return restart_launcher(false);
    }
    if automated {
        let report = report.lock().unwrap_or_else(|p| p.into_inner());
        println!("{}", report.message);
        tracing::info!("{}", report.message);
        if report.failed || report.message.is_empty() {
            code = ExitCode::FAILURE;
        }
    }
    code
}

/// Starts the editor with a project given on the command line, then checks for updates.
fn open_directly(project: &Path, data_dir: Option<&Path>, check_updates: bool) -> ExitCode {
    let Some(data_dir) = data_dir else {
        return ExitCode::FAILURE;
    };
    let mut child = match editor_start::start_editor(data_dir, &[project.as_os_str()]) {
        Ok(child) => child,
        Err(err) => {
            tracing::error!("{}", err.report());
            eprintln!("{}", err.report());
            return ExitCode::FAILURE;
        }
    };
    // Best effort: list it so it shows up in the browser next time.
    if let Ok(index) = project_index::Index::open(&data_dir.join(project_index::INDEX_FILE_NAME))
        && index.record(project).is_ok()
    {
        let _ = index.mark_opened(project, project_index::now());
    }
    if check_updates {
        update_in_background(data_dir);
    }
    if watch_editor(data_dir, &mut child).rolled_back {
        restart_launcher(false);
    }
    ExitCode::SUCCESS
}

/// How watching the editor start went.
pub(crate) struct Watched {
    /// `None` when this copy is not an installed one: nothing was watched.
    pub outcome: Option<updater::health::StartOutcome>,
    /// Repeated failures switched back to the previous version. The caller
    /// opens the launcher again to explain.
    pub rolled_back: bool,
}

/// Waits (up to 20 seconds, no window) for the editor to report that it
/// started. A version that works lets its launcher replace this one;
/// repeated failures switch back to the previous version.
pub(crate) fn watch_editor(data_dir: &Path, child: &mut std::process::Child) -> Watched {
    use updater::apply::InstallKind;
    use updater::health;
    let nothing = Watched {
        outcome: None,
        rolled_back: false,
    };
    let InstallKind::Folders(layout) = InstallKind::detect(&install_root()) else {
        return nothing;
    };
    let Some(version) = layout.current_version() else {
        return nothing;
    };
    let outcome = health::watch_start(data_dir, child, health::START_TIMEOUT);
    tracing::info!(?outcome, %version, "editor start");
    if outcome == health::StartOutcome::Started {
        // The version works: now its launcher may replace this one (used from the next start).
        match updater::apply::replace_launcher(&layout, VERSION) {
            Ok(true) => tracing::info!(%version, "launcher updated"),
            Ok(false) => {}
            Err(err) => tracing::warn!("{}", err.report()),
        }
    }
    let rolled_back = match health::after_start(data_dir, &layout, &version, outcome) {
        Ok(Some(notice)) => {
            tracing::warn!(failed = %notice.failed, restored = %notice.restored, "rolled back");
            true
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!("{}", err.report());
            false
        }
    };
    Watched {
        outcome: Some(outcome),
        rolled_back,
    }
}

/// Checks for a newer version and, with automatic updates on, downloads and verifies it.
fn update_in_background(data_dir: &Path) {
    let prefs = UpdatePrefs::load(data_dir);
    match updater::release::check(&version_in_use(), &prefs) {
        Ok(CheckResult::Available(release)) if prefs.automatic => {
            tracing::info!(version = %release.version, "downloading an update");
            match updater::fetch(&release, &data_dir.join("downloads")) {
                Ok(ready) => tracing::info!(version = %ready.version, "update downloaded and verified"),
                Err(err) => tracing::warn!("{}", err.report()),
            }
        }
        Ok(result) => tracing::info!(?result, "update check done"),
        Err(err) => tracing::info!("{}", err.report()),
    }
}

/// What the launcher shares with the editor's Settings, so both windows
/// look and answer alike: the theme, and the shortcuts changed there.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct EditorSettings {
    theme: ui_kit::ThemeName,
    shortcuts: ui_kit::shortcuts::SavedBindings,
}

fn editor_settings(data_dir: &Path) -> EditorSettings {
    std::fs::read_to_string(data_dir.join("settings.ron"))
        .ok()
        .and_then(|text| ron::from_str::<EditorSettings>(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_launcher_uses_the_theme_and_the_shortcuts_changed_in_the_editor() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = super::editor_settings(tmp.path());
        assert_eq!(shared.theme, ui_kit::ThemeName::default(), "no settings yet");
        assert_eq!(
            ui_kit::ShortcutTable::restored(&shared.shortcuts),
            ui_kit::ShortcutTable::default()
        );

        std::fs::write(
            tmp.path().join("settings.ron"),
            "(theme: Carbon, edit_at_playhead: true, shortcuts: [(command: \"Search\", nth: 0, keys: Some(\"Ctrl+F\"))])",
        )
        .unwrap();
        let shared = super::editor_settings(tmp.path());
        assert_eq!(shared.theme, ui_kit::ThemeName::Carbon);
        let table = ui_kit::ShortcutTable::restored(&shared.shortcuts);
        assert_eq!(table.keys_for(ui_kit::Action::Search), "Ctrl+F");
        assert_eq!(table.keys_for(ui_kit::Action::OpenSettings), "Ctrl+Comma");
    }

    use graphics::{Backend, GraphicsSettings, Platform, Program, RunKind, StartMarker};
    use updater::health::{self, StartOutcome};

    /// The two safety nets in the order they act: a crash in the Vulkan driver
    /// costs the editor one failed start, the next start avoids Vulkan and
    /// reports that it started, and that resets the count long before the
    /// version would be rolled back.
    #[test]
    fn a_vulkan_crash_in_the_editor_changes_the_graphics_not_the_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("app"));
        std::fs::create_dir_all(&data).unwrap();
        let layout = updater::Layout::at(&root);
        for version in ["0.1.0", "0.2.0"] {
            let dir = layout.version_dir(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(updater::install::editor_file_name()), "").unwrap();
        }
        updater::apply::set_current(&layout, "0.2.0").unwrap();
        let after_start = |outcome| health::after_start(&data, &layout, "0.2.0", outcome).unwrap();
        let editor_plan = || {
            let marker = StartMarker::in_dir(Program::Editor, &data);
            let choice = GraphicsSettings::load(&data).choice;
            graphics::plan(
                Platform::Windows,
                choice,
                marker.state(),
                graphics::DecodingFound::FRESH,
                RunKind::Normal,
            )
        };

        // Start 1: the editor notes that it starts with Vulkan, then the driver crashes.
        assert_eq!(editor_plan().order[0], Backend::Vulkan);
        StartMarker::in_dir(Program::Editor, &data).write().unwrap();
        assert_eq!(
            after_start(StartOutcome::Failed),
            None,
            "one failure rolls nothing back"
        );

        // Start 2: the marker keeps the editor off Vulkan, and it asks what to keep.
        let plan = editor_plan();
        assert_eq!(plan.order[0], Backend::Direct3D12);
        assert!(plan.ask);
        assert_eq!(after_start(StartOutcome::Started), None);

        // "Try Vulkan again" and another crash is one failure in a row again, never two.
        assert_eq!(after_start(StartOutcome::Failed), None);
        assert_eq!(layout.current_version().as_deref(), Some("0.2.0"));
        assert!(updater::UpdatePrefs::load(&data).bad_versions.is_empty());
    }
}
