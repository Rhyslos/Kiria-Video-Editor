//! The editor: window, pages and the start-up sequence.
//!
//! Usage: `editor [project file] [--open-last-save] [--import <media file>]... [--playhead <seconds>] [--play] [--select-first-clip] [--cpu-colour] [--graphics-warning] [--decoding-warning] [--shortcuts-page <filter|rebinding|clash>] [--cheat-sheet <file.html>] [--smoke-test | --responsiveness-test | --screenshot <file.png>]`.
//! The extra options are for automated checks; normal use needs none.

// Release builds on Windows open no console window next to the editor.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cache_cleanup;
mod cheat_sheet;
mod clip_pictures;
mod device_decoding;
mod display;
mod editor;
mod export_page;
mod fake_pointer;
mod frame_grab;
mod global_labels;
mod import;
mod inspector;
mod label_editor;
mod library_page;
mod looping;
mod media_drawer;
mod media_folders;
mod organise;
mod package_window;
mod picture_handles;
mod proxies;
mod prune_window;
mod reading;
mod relink;
mod run_mode;
mod search;
mod settings;
mod silence_panel;
mod slow_pictures;
mod system_keyboard;
mod templates_window;
mod timeline_panel;
mod versions_window;
mod video_picture;
mod view_state;
mod viewer;
mod viewer_window;
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
    // Proxies live in the cache folder, by the recording's fingerprint.
    let proxy_dir = diagnostics.as_ref().map_or_else(
        || std::env::temp_dir().join("proxies"),
        |d| d.dirs.cache.join("proxies"),
    );
    let (settings, settings_problem) = Settings::load(&data_dir);
    let mut problems: Vec<diag::AppError> = settings_problem.into_iter().collect();
    if diagnostics.as_ref().is_some_and(|d| d.previous_run_crashed) {
        problems.push(
            diag::AppError::new("The editor did not close properly last time")
                .why("It crashed, was forced to quit, or the computer lost power.")
                .fix("Open your project: unsaved changes are recovered automatically. A crash report may be in the crash-reports folder."),
        );
    }
    // `--cheat-sheet`: the sheet for the shortcuts in these settings, and nothing else.
    if let Some(path) = &options.cheat_sheet {
        let table = ui_kit::ShortcutTable::restored(&settings.shortcuts);
        let layout = settings.keyboard.resolve(&system_keyboard::asked_once());
        let written = cheat_sheet::write(path, &table, layout);
        if let Err(err) = &written {
            eprintln!("{}", err.report());
        }
        if let Some(d) = diagnostics
            && let Err(err) = d.finish()
        {
            eprintln!("{}", err.report());
        }
        return if written.is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    // Which graphics system to try, and whether the last start with Vulkan failed.
    // Automated runs only read: no marker is written and no setting changes.
    let automated = options.mode != RunMode::Normal;
    let run = if automated {
        graphics::RunKind::Automated
    } else {
        graphics::RunKind::Normal
    };
    let (mut graphics_start, graphics_problems) =
        graphics::prepare(graphics::Program::Editor, &data_dir, run);
    problems.extend(graphics_problems);
    if options.graphics_warning {
        graphics_start = graphics_start.with_warning_preview();
    }
    if options.decoding_warning {
        graphics_start = graphics_start.with_decoding_warning_preview();
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
        find: options.find.clone(),
        watch_folders: options.watch_folders.clone(),
        export_page: options.export_page,
        page: options.page,
        library_tab: options.library_tab.clone(),
        library_select: options.library_select.clone(),
        full_screen: options.full_screen,
        select_first_clip: options.select_first_clip,
        actions: options.actions.clone(),
        then_actions: options.then_actions.clone(),
        cpu_colour: options.cpu_colour,
        hold_reading: options.hold_reading,
        hold_proxy: options.hold_proxy,
        package_to: options.package_to.clone(),
        hold_package: options.hold_package,
    });
    let pointer = options.pointer;
    // If this run crashes, the launcher offers to recover this project.
    if let (Some(diagnostics), Some(project)) = (&diagnostics, &project)
        && let Err(err) = diagnostics.note_open_project(&project.path)
    {
        tracing::warn!("{}", err.report());
    }

    let report = Arc::new(Mutex::new(RunReport::default()));
    let start = editor::Start {
        settings,
        data_dir,
        proxy_dir,
        project,
        mode: options.mode,
        report: Arc::clone(&report),
        problems,
        pointer,
        shortcuts_page: options.shortcuts_page,
    };

    let mut code = ExitCode::SUCCESS;
    let window = || eframe::NativeOptions {
        viewport: ui_kit::icon::with_window_icon(egui::ViewportBuilder::default())
            .with_title(brand::APP_NAME)
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    let opened = graphics::open_window(
        &graphics_start,
        brand::APP_NAME,
        window,
        start,
        |cc, start, opened| Box::new(Editor::new(cc, start, opened)),
    );
    if let Err(err) = opened {
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
