//! Explained errors, logging and crash reports.
//!
//! - [`AppError`]: every error says what happened, why, and how to fix it.
//! - [`logging`]: rolling log files in the app's data folder.
//! - [`crash`]: writes a crash report when the app panics.
//! - [`session`]: notices when the previous run ended without closing cleanly.
//!
//! Most programs only need [`start`] at the top of `main` and
//! [`Diagnostics::finish`] at the end.

pub mod crash;
mod error;
pub mod logging;
mod paths;
pub mod session;

pub use error::{AppError, Explain, FixAction, Result};
pub use paths::AppDirs;

/// How many days of log files are kept.
pub const LOG_DAYS_KEPT: usize = 7;

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
    crash::install_panic_hook(dirs.crashes.clone(), dirs.logs.clone(), app_version);
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

/// Folders and logging for a helper program such as the launcher. No crash
/// detection: only the editor's own runs count as sessions.
pub fn start_helper(name: &str) -> Result<(AppDirs, logging::LogGuard)> {
    let dirs = AppDirs::locate()?;
    dirs.create()?;
    let log = logging::init_named(&dirs.logs, LOG_DAYS_KEPT, name)?;
    Ok((dirs, log))
}

impl Diagnostics {
    /// Marks the run as closed cleanly and flushes the log.
    pub fn finish(self) -> Result<()> {
        tracing::info!("closing");
        self.session.end()
    }
}
