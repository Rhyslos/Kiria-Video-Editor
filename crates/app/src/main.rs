//! The editor: window, pages and the start-up sequence.
//!
//! Usage: `editor [project file] [--open-last-save] [--import <media file>]... [--playhead <seconds>] [--play] [--cpu-colour] [--smoke-test | --responsiveness-test | --screenshot <file.png>]`.
//! The extra options are for automated checks; normal use needs none.

// Release builds on Windows open no console window next to the editor.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod clip_pictures;
mod editor;
mod frame_grab;
mod graphics;
mod import;
mod label_editor;
mod looping;
mod media_drawer;
mod relink;
mod run_mode;
mod search;
mod settings;
mod silence_panel;
mod timeline_panel;
mod versions_window;
mod video_picture;
mod viewer;
mod waveforms;

use editor::{Editor, ProjectToOpen, RunReport};
use graphics::{Backend, FailedTry, StartMarker};
use run_mode::{Options, RunMode};
use settings::Settings;
use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
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
    let (mut settings, settings_problem) = Settings::load(&data_dir);
    let mut problems: Vec<diag::AppError> = settings_problem.into_iter().collect();
    if diagnostics.as_ref().is_some_and(|d| d.previous_run_crashed) {
        problems.push(
            diag::AppError::new("The editor did not close properly last time")
                .why("It crashed, was forced to quit, or the computer lost power.")
                .fix("Open your project: unsaved changes are recovered automatically. A crash report may be in the crash-reports folder."),
        );
    }
    // A start with Vulkan that never drew a frame switches to Direct3D 12.
    let marker = StartMarker::in_dir(&data_dir);
    let plan = graphics::plan(
        graphics::Platform::current(),
        settings.graphics,
        marker.is_present(),
    );
    problems.extend(graphics::apply_switch(&plan, &mut settings, &marker, &data_dir));

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
        full_screen: options.full_screen,
        actions: options.actions.clone(),
        cpu_colour: options.cpu_colour,
    });
    // If this run crashes, the launcher offers to recover this project.
    if let (Some(diagnostics), Some(project)) = (&diagnostics, &project)
        && let Err(err) = diagnostics.note_open_project(&project.path)
    {
        tracing::warn!("{}", err.report());
    }

    let report = Arc::new(Mutex::new(RunReport::default()));
    let automated = options.mode != RunMode::Normal;
    let start = editor::Start {
        settings,
        data_dir,
        project,
        mode: options.mode,
        report: Arc::clone(&report),
        problems,
    };

    let mut code = ExitCode::SUCCESS;
    if let Err(err) = open_window(plan.order, &marker, start) {
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

/// Opens the editor window with the first graphics system in `order` that
/// starts, and returns when the window has closed.
///
/// A graphics system that reports an error before the window exists is
/// followed by the next one. Vulkan can also crash or freeze instead: the
/// marker written here is what the next start finds then.
fn open_window(order: &[Backend], marker: &StartMarker, start: editor::Start) -> diag::Result<()> {
    // Handed to the window that opens; still here after a try that didn't.
    let start = Rc::new(RefCell::new(Some(start)));
    let mut failed: Vec<FailedTry> = Vec::new();
    for &backend in order {
        let guarded = backend == Backend::Vulkan;
        if guarded && let Err(err) = marker.write() {
            // Without the marker a crash would repeat at every start: skip Vulkan.
            tracing::warn!("{}", err.report());
            let reason = "not tried: the note that guards against a crash could not be written".into();
            failed.push(FailedTry { backend, reason });
            continue;
        }
        let window_options = eframe::NativeOptions {
            viewport: ui_kit::icon::with_window_icon(egui::ViewportBuilder::default())
                .with_title(brand::APP_NAME)
                .with_inner_size([1440.0, 900.0])
                .with_min_inner_size([960.0, 600.0]),
            renderer: eframe::Renderer::Wgpu,
            wgpu_options: graphics::toolkit_options(backend),
            ..Default::default()
        };
        let opened = graphics::Opened {
            backend,
            failed: failed.clone(),
            marker: guarded.then(|| marker.clone()),
        };
        let for_window = Rc::clone(&start);
        let result = eframe::run_native(
            brand::APP_NAME,
            window_options,
            Box::new(move |cc| match for_window.borrow_mut().take() {
                Some(start) => Ok(Box::new(Editor::new(cc, start, opened))),
                None => Err(diag::AppError::new("The editor window was opened twice").into()),
            }),
        );
        let window_existed = start.borrow().is_none();
        match result {
            Ok(()) => {
                if guarded && let Err(err) = marker.clear() {
                    tracing::warn!("{}", err.report());
                }
                return Ok(());
            }
            // The window was open (or opening) when it failed: nothing to fall
            // back to in this start. A marker still there makes the next one
            // use Direct3D 12.
            Err(err) if window_existed => {
                return Err(diag::AppError::new("The editor window stopped working")
                    .why(format!("The graphics system reported: {err}"))
                    .fix("Start the editor again. If it keeps happening, update your graphics driver and report it with the log file."));
            }
            Err(err) => {
                if guarded && let Err(err) = marker.clear() {
                    tracing::warn!("{}", err.report());
                }
                let failure = FailedTry::new(backend, &err);
                tracing::warn!("{} did not start: {}", backend.label(), failure.reason);
                failed.push(failure);
            }
        }
    }
    let tried: Vec<String> = failed
        .iter()
        .map(|f| format!("{}: {}", f.backend.label(), f.reason))
        .collect();
    Err(diag::AppError::new("The editor window could not open")
        .why(format!("No graphics system started. {}", tried.join("; ")))
        .fix("Update your graphics driver. If that doesn't help, report it with the log file."))
}
