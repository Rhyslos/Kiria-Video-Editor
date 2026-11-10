//! Starting the editor process.

use diag::AppError;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use updater::Layout;

/// Starts the installed editor with `args` (normally one project file) and
/// returns at once; the editor runs on its own.
pub fn start_editor(args: &[&OsStr]) -> diag::Result<()> {
    let exe = std::env::current_exe().map_err(|err| {
        AppError::new("The editor can't be found")
            .why("The launcher could not tell where it is installed.")
            .fix("Reinstall the app.")
            .source(err)
    })?;
    let root = exe.parent().unwrap_or(Path::new("."));
    let editor = Layout::at(root).editor().ok_or_else(|| {
        AppError::new("The editor can't be found")
            .why(format!("No editor is installed in {}.", root.display()))
            .fix("Reinstall the app.")
    })?;
    tracing::info!(editor = %editor.display(), "starting the editor");
    Command::new(&editor).args(args).spawn().map_err(|err| {
        AppError::new("The editor could not start")
            .why(format!("Starting {} failed.", editor.display()))
            .fix("Reinstall the app. If that doesn't help, report it with the launcher log.")
            .source(err)
    })?;
    Ok(())
}

/// Opens the system file manager at `path` (a folder, or the folder holding a file).
pub fn show_in_file_manager(path: &Path) -> diag::Result<()> {
    let folder = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    Command::new(program).arg(folder).spawn().map_err(|err| {
        AppError::new("The folder can't be shown")
            .why(format!(
                "Starting the file manager for {} failed.",
                folder.display()
            ))
            .fix("Open the folder yourself; its path is shown when you hover over it.")
            .source(err)
    })?;
    Ok(())
}
