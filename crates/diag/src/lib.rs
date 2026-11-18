//! Explained errors, logging and crash reports.
//!
//! - [`AppError`]: every error says what happened, why, and how to fix it.
//! - [`logging`]: rolling log files in the app's data folder.
//! - [`crash`]: writes a crash report when the app panics.
//! - [`session`]: notices when the previous run ended without closing cleanly.
//! - [`marker`]: markers kept per process with a lock beside each, which
//!   tell a process still going from one that crashed.
//!
//! Most programs only need [`start`] at the top of `main` and
//! [`Diagnostics::finish`] at the end.

pub mod crash;
mod error;
mod log_writer;
pub mod logging;
pub mod marker;
mod paths;
pub mod session;
mod stamp;

pub use error::{AppError, Explain, FixAction, Result};
pub use paths::AppDirs;

/// How many days of log files are kept.
pub const LOG_DAYS_KEPT: usize = 7;

/// How many crash reports are kept; older ones are removed at start.
pub const CRASH_REPORTS_KEPT: usize = 20;

/// Everything [`start`] set up. Keep it alive until the program ends.
#[derive(Debug)]
pub struct Diagnostics {
    pub dirs: AppDirs,
    /// True when the previous run ended without closing cleanly (a crash,
    /// a forced quit or a power cut).
    pub previous_run_crashed: bool,
    session: session::Session,
    _log: logging::LogGuard,
}

/// Sets up the data folders, logging, the crash handler and session tracking.
///
/// `app_version` is the caller's version, usually `brand::VERSION`.
pub fn start(app_version: &'static str) -> Result<Diagnostics> {
    let dirs = AppDirs::locate()?;
    dirs.create()?;
    let log = logging::init(&dirs.logs, LOG_DAYS_KEPT)?;
    start_crash_reports(&dirs, app_version);
    let started = session::Session::start(&dirs.data)?;
    if started.previous_crashed {
        tracing::warn!("the previous run did not close cleanly");
    }
    tracing::info!(version = app_version, "started");
    Ok(Diagnostics {
        dirs,
        previous_run_crashed: started.previous_crashed,
        session: started.session,
        _log: log,
    })
}

/// Folders, logging (its own `<name>.YYYY-MM-DD.log` files) and crash
/// reports for a helper program such as the launcher. No crash detection:
/// only the editor's own runs count as sessions.
pub fn start_helper(name: &str, app_version: &'static str) -> Result<(AppDirs, logging::LogGuard)> {
    let dirs = AppDirs::locate()?;
    dirs.create()?;
    let log = logging::init_named(&dirs.logs, LOG_DAYS_KEPT, name)?;
    start_crash_reports(&dirs, app_version);
    Ok((dirs, log))
}

/// Removes old crash reports, then installs the crash handler.
fn start_crash_reports(dirs: &AppDirs, app_version: &'static str) {
    if let Err(err) = crash::prune_reports(&dirs.crashes, CRASH_REPORTS_KEPT) {
        tracing::warn!("old crash reports could not be removed: {err}");
    }
    crash::install_panic_hook(dirs.crashes.clone(), app_version);
}

impl Diagnostics {
    /// Records which project this run has open, so a crash can offer to recover it.
    pub fn note_open_project(&self, project: &std::path::Path) -> Result<()> {
        self.session.note_project(project)
    }

    /// Marks the run as closed cleanly and flushes the log.
    pub fn finish(self) -> Result<()> {
        tracing::info!("closing");
        self.session.end()
    }
}
