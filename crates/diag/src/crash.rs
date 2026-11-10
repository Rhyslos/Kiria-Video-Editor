//! Crash reports.
//!
//! [`install_panic_hook`] writes a plain-text report to the crash-reports
//! folder whenever the app panics: what went wrong and where, the app
//! version, the system, the graphics card once known, the last lines of the
//! log, and a backtrace. The normal panic output still follows.

use std::backtrace::Backtrace;
use std::fs;
use std::io;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// How many log lines a report includes.
const LOG_LINES_IN_REPORT: usize = 200;

static GPU_INFO: OnceLock<String> = OnceLock::new();

/// Records the graphics card description for future crash reports.
/// The first call wins; later calls are ignored.
pub fn set_gpu_info(description: impl Into<String>) {
    let _ = GPU_INFO.set(description.into());
}

/// Everything a crash report contains apart from the time.
#[derive(Debug)]
pub struct CrashDetails<'a> {
    pub message: &'a str,
    pub location: &'a str,
    pub app_version: &'a str,
    pub backtrace: &'a str,
    pub log_tail: &'a [String],
}

/// Writes a report on every panic, then runs the previous panic handler.
pub fn install_panic_hook(crash_dir: PathBuf, logs_dir: PathBuf, app_version: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = Backtrace::force_capture().to_string();
        let log_tail = crate::logging::latest_log_file(&logs_dir)
            .map(|file| crate::logging::tail(&file, LOG_LINES_IN_REPORT))
            .unwrap_or_default();
        let message = panic_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".into());
        let report = build_report(&CrashDetails {
            message: &message,
            location: &location,
            app_version,
            backtrace: &backtrace,
            log_tail: &log_tail,
        });
        match write_report(&crash_dir, &report) {
            Ok(path) => {
                tracing::error!(report = %path.display(), "crashed: {message}");
                eprintln!("A crash report was saved to {}", path.display());
            }
            Err(err) => eprintln!("The crash report could not be saved: {err}"),
        }
        previous(info);
    }));
}

/// Builds the report text.
pub fn build_report(details: &CrashDetails<'_>) -> String {
    let gpu = GPU_INFO.get().map(String::as_str).unwrap_or("not detected yet");
    let mut report = format!(
        "{name} crash report\n\
         ====================\n\
         What happened: {message}\n\
         Where:         {location}\n\
         Version:       {version}\n\
         System:        {os} ({arch})\n\
         Graphics:      {gpu}\n\
         Time:          {time}\n",
        name = brand::APP_NAME,
        message = details.message,
        location = details.location,
        version = details.app_version,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        time = humantime::format_rfc3339_seconds(SystemTime::now()),
    );
    report.push_str("\nLast log lines\n--------------\n");
    if details.log_tail.is_empty() {
        report.push_str("(no log available)\n");
    }
    for line in details.log_tail {
        report.push_str(line);
        report.push('\n');
    }
    report.push_str("\nBacktrace\n---------\n");
    report.push_str(details.backtrace);
    report.push('\n');
    report
}

/// Saves a report as `crash-<time>.txt` in `dir` and returns its path.
pub fn write_report(dir: &Path, report: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // Colons are not allowed in Windows file names.
    let stamp = humantime::format_rfc3339_seconds(SystemTime::now())
        .to_string()
        .replace(':', "-");
    let mut path = dir.join(format!("crash-{stamp}.txt"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("crash-{stamp}-{n}.txt"));
    }
    fs::write(&path, report)?;
    Ok(path)
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with no message".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details<'a>(log_tail: &'a [String]) -> CrashDetails<'a> {
        CrashDetails {
            message: "index out of bounds",
            location: "crates/media/src/lib.rs:42",
            app_version: "0.0.1",
            backtrace: "frame 0",
            log_tail,
        }
    }

    #[test]
    fn report_contains_every_part() {
        let tail = vec!["INFO started".to_string()];
        let report = build_report(&details(&tail));
        for part in [
            "index out of bounds",
            "crates/media/src/lib.rs:42",
            "0.0.1",
            std::env::consts::OS,
            "INFO started",
            "frame 0",
        ] {
            assert!(report.contains(part), "missing {part:?} in:\n{report}");
        }
    }

    #[test]
    fn report_says_when_there_is_no_log() {
        assert!(build_report(&details(&[])).contains("(no log available)"));
    }

    #[test]
    fn reports_never_overwrite_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let first = write_report(tmp.path(), "one").unwrap();
        let second = write_report(tmp.path(), "two").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first).unwrap(), "one");
        let name = second.file_name().unwrap().to_string_lossy();
        assert!(!name.contains(':'), "file names must be valid on Windows: {name}");
    }
}
