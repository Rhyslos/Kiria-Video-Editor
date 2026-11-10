//! The small program that opens first.
//!
//! For now it starts the editor straight away, then checks GitHub for a newer
//! version in the background and, if automatic updates are on, downloads and
//! verifies it. Installing a downloaded update (Phase 3) and the project
//! browser window (Phase 2) build on this.
//!
//! Usage: `launcher [project file] [--no-update-check]`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::process::{Command, ExitCode};
use updater::{CheckResult, Layout, UpdatePrefs};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check_updates = !args.iter().any(|a| a == "--no-update-check");
    let editor_args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let helper = diag::start_helper("launcher");
    let (dirs, _log) = match helper {
        Ok((dirs, log)) => (Some(dirs), Some(log)),
        Err(err) => {
            eprintln!("{}", err.report());
            (None, None)
        }
    };
    tracing::info!(version = VERSION, "launcher started");

    // The editor starts first: being offline or a slow network never delays it.
    let started = start_editor(&editor_args);
    if let Err(err) = &started {
        tracing::error!("{}", err.report());
        eprintln!("{}", err.report());
    }

    if check_updates && let Some(dirs) = &dirs {
        update_in_background(&dirs.data);
    }
    if started.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn start_editor(args: &[&String]) -> diag::Result<()> {
    let exe = std::env::current_exe().map_err(|err| {
        diag::AppError::new("The editor can't be found")
            .why("The launcher could not tell where it is installed.")
            .fix("Reinstall the app.")
            .source(err)
    })?;
    let root = exe.parent().unwrap_or(Path::new("."));
    let editor = Layout::at(root).editor().ok_or_else(|| {
        diag::AppError::new("The editor can't be found")
            .why(format!("No editor is installed in {}.", root.display()))
            .fix("Reinstall the app.")
    })?;
    tracing::info!(editor = %editor.display(), "starting the editor");
    Command::new(&editor).args(args).spawn().map_err(|err| {
        diag::AppError::new("The editor could not start")
            .why(format!("Starting {} failed.", editor.display()))
            .fix("Reinstall the app. If that doesn't help, report it with the launcher log.")
            .source(err)
    })?;
    Ok(())
}

/// Checks for a newer version and, with automatic updates on, downloads and verifies it.
fn update_in_background(data_dir: &Path) {
    let prefs = load_prefs(data_dir);
    match updater::release::check(VERSION, &prefs) {
        Ok(CheckResult::UpToDate) => tracing::info!("up to date"),
        Ok(CheckResult::Skipped(version)) => tracing::info!(%version, "skipped by choice"),
        Ok(CheckResult::Available(release)) if prefs.automatic => {
            tracing::info!(version = %release.version, "downloading an update");
            match updater::fetch(&release, &data_dir.join("downloads")) {
                Ok(ready) => tracing::info!(
                    version = %ready.version,
                    package = %ready.package.display(),
                    "update downloaded and verified; it installs once installing arrives (Phase 3)"
                ),
                Err(err) => tracing::warn!("{}", err.report()),
            }
        }
        Ok(CheckResult::Available(release)) => {
            tracing::info!(version = %release.version, "an update is available; automatic updates are off");
        }
        Err(err) => tracing::info!("{}", err.report()),
    }
}

fn load_prefs(data_dir: &Path) -> UpdatePrefs {
    std::fs::read_to_string(data_dir.join("updates.ron"))
        .ok()
        .and_then(|text| ron::from_str(&text).ok())
        .unwrap_or_default()
}
