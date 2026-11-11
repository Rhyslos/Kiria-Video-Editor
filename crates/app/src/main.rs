//! The editor: window, pages and the start-up sequence.
//!
//! Usage: `editor [project file] [--open-last-save] [--import <media file>]... [--playhead <seconds>] [--play] [--smoke-test | --responsiveness-test | --screenshot <file.png>]`.
//! The extra options are for automated checks; normal use needs none.

// Release builds on Windows open no console window next to the editor.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod editor;
mod frame_grab;
mod import;
mod looping;
mod media_drawer;
mod relink;
mod run_mode;
mod search;
mod settings;
mod silence_panel;
mod timeline_panel;
mod viewer;
mod waveforms;

use editor::{Editor, ProjectToOpen, RunReport};
use run_mode::{Options, RunMode};
use settings::Settings;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    // Test builds made with BUILD_CRASH_TEST crash before the window opens, to
    // check that installed copies roll back to the previous version by themselves.
    if option_env!("BUILD_CRASH_TEST").is_some() {
        let _ = diag::start(brand::VERSION);
        panic!("This is a deliberately broken test build: it crashes on start.");
    }

    let diagnostics = match diag::start(brand::VERSION) {
        Ok(diagnostics) => Some(diagnostics),
        Err(err) => {
            eprintln!("{}", err.report());
            None
        }
    };
    let data_dir = diagnostics
        .as_ref()
        .map_or_else(std::env::temp_dir, |d| d.dirs.data.clone());
    let (settings, settings_problem) = Settings::load(&data_dir);
    let mut problems: Vec<diag::AppError> = settings_problem.into_iter().collect();
    if diagnostics.as_ref().is_some_and(|d| d.previous_run_crashed) {
        problems.push(
            diag::AppError::new("The editor did not close properly last time")
                .why("It crashed, was forced to quit, or the computer lost power.")
                .fix("Open your project: unsaved changes are recovered automatically. A crash report may be in the crash-reports folder."),
        );
    }

    let project = options.project.clone().map(|path| ProjectToOpen {
        path,
        recovery: if options.open_last_save {
            project::Recovery::OpenLastSave
        } else {
            project::Recovery::Recover
        },
        import: options.import.clone(),
        playhead: options.playhead,
        play: options.play,
        media_drawer: options.media_drawer,
    });
    // If this run crashes, the launcher offers to recover this project.
    if let (Some(diagnostics), Some(project)) = (&diagnostics, &project)
        && let Err(err) = diagnostics.note_open_project(&project.path)
    {
        tracing::warn!("{}", err.report());
    }

    let report = Arc::new(Mutex::new(RunReport::default()));
    let window_options = eframe::NativeOptions {
        viewport: ui_kit::icon::with_window_icon(egui::ViewportBuilder::default())
            .with_title(brand::APP_NAME)
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let automated = options.mode != RunMode::Normal;
    let result = {
        let report = Arc::clone(&report);
        eframe::run_native(
            brand::APP_NAME,
            window_options,
            Box::new(move |cc| {
                Ok(Box::new(Editor::new(
                    cc,
                    settings,
                    data_dir,
                    project,
                    options.mode,
                    report,
                    problems,
                )))
            }),
        )
    };

    let mut code = ExitCode::SUCCESS;
    if let Err(err) = result {
        let err = diag::AppError::new("The editor window could not open")
            .why(format!("The graphics system reported: {err}"))
            .fix("Update your graphics driver. If that doesn't help, report it with the log file.");
        tracing::error!("{}", err.report());
        eprintln!("{}", err.report());
        code = ExitCode::FAILURE;
    }
    if automated {
        let report = report.lock().unwrap_or_else(|p| p.into_inner());
        println!("{}", report.message);
        tracing::info!("{}", report.message);
        if report.failed || report.message.is_empty() {
            code = ExitCode::FAILURE;
        }
    }
    if let Some(d) = diagnostics
        && let Err(err) = d.finish()
    {
        eprintln!("{}", err.report());
    }
    code
}
