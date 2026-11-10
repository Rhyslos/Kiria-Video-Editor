//! The small program that opens first.
//!
//! With no arguments it shows the project browser: a folder tree, the
//! project grid and the version panel. Opening a project starts the editor
//! and closes the browser. With a project file as argument it starts the
//! editor with that project straight away. Either way it checks GitHub for a
//! newer version in the background and, with automatic updates on, downloads
//! and verifies it; installing it arrives in Phase 3.
//!
//! Usage: `launcher [project file] [--no-update-check] [--smoke-test | --screenshot <file.png>] [--demo <count>] [--update-window <available | downloading | ready>]`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod browser;
mod cards;
mod demo;
mod editor_start;
mod files;
mod new_project;
mod prefs;
mod run_mode;
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

    // A verified update downloaded last time is installed before anything opens.
    let mut start_notes = Vec::new();
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

    let data_dir = match options.demo {
        Some(count) => match demo::prepare(count) {
            Ok(dir) => dir,
            Err(err) => {
                eprintln!("{}", err.report());
                return ExitCode::FAILURE;
            }
        },
        None => dirs.as_ref().map_or_else(std::env::temp_dir, |d| d.data.clone()),
    };
    browse(data_dir, options, start_notes)
}

/// What happened to a waiting update on start.
enum Installed {
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
    Problem(diag::AppError),
}

fn install_waiting_update(data_dir: &Path) -> diag::Result<Installed> {
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
    if let Some(ready) = apply::pending(&downloads, &current) {
        tracing::info!(from = %current, to = %ready.version, "installing an update");
        apply::install(&kind, &ready)?;
        apply::forget_download(&downloads, &ready.version);
        if let InstallKind::Folders(layout) = &kind {
            apply::prune(layout, &[&ready.version, &current]);
        }
        installed = Some(ready.version);
    }
    let restart = match &kind {
        InstallKind::Folders(layout) => apply::replace_launcher(layout, VERSION)?,
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

/// The folder the launcher is installed in.
fn install_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Starts the (new) launcher, with this run's arguments if `same_args`, and ends this one.
fn restart_launcher(same_args: bool) -> ExitCode {
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
fn browse(data_dir: std::path::PathBuf, options: Options, notes: Vec<Note>) -> ExitCode {
    let report = Arc::new(Mutex::new(RunReport::default()));
    let handoff = Arc::new(Mutex::new(None));
    let restart = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let automated = options.mode != RunMode::Normal;
    let start = Start {
        theme: saved_theme(&data_dir),
        data_dir,
        check_updates: options.check_updates,
        launch_editor: !automated && options.demo.is_none(),
        mode: options.mode,
        report: Arc::clone(&report),
        handoff: Arc::clone(&handoff),
        notes,
        restart: Arc::clone(&restart),
        update_preview: options.update_window,
    };
    let window_options = eframe::NativeOptions {
        viewport: ui_kit::icon::with_window_icon(egui::ViewportBuilder::default())
            .with_title(brand::APP_NAME)
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 560.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let result = eframe::run_native(
        brand::APP_NAME,
        window_options,
        Box::new(move |cc| Ok(Box::new(Browser::new(cc, start)))),
    );

    let mut code = ExitCode::SUCCESS;
    if let Err(err) = result {
        let err = diag::AppError::new("The launcher window could not open")
            .why(format!("The graphics system reported: {err}"))
            .fix("Update your graphics driver. If that doesn't help, report it with the launcher log.");
        tracing::error!("{}", err.report());
        eprintln!("{}", err.report());
        code = ExitCode::FAILURE;
    }
    // A download that was running when the window closed finishes quietly.
    let updates = handoff.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(updates) = updates {
        updates.finish_download();
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
    if let Err(err) = editor_start::start_editor(&[project.as_os_str()]) {
        tracing::error!("{}", err.report());
        eprintln!("{}", err.report());
        return ExitCode::FAILURE;
    }
    let Some(data_dir) = data_dir else {
        return ExitCode::SUCCESS;
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
    ExitCode::SUCCESS
}

/// Checks for a newer version and, with automatic updates on, downloads and verifies it.
fn update_in_background(data_dir: &Path) {
    let prefs = UpdatePrefs::load(data_dir);
    match updater::release::check(VERSION, &prefs) {
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

/// The theme chosen in the editor's Settings, so both windows look alike.
fn saved_theme(data_dir: &Path) -> ui_kit::ThemeName {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct EditorSettings {
        theme: ui_kit::ThemeName,
    }
    std::fs::read_to_string(data_dir.join("settings.ron"))
        .ok()
        .and_then(|text| ron::from_str::<EditorSettings>(&text).ok())
        .unwrap_or_default()
        .theme
}
