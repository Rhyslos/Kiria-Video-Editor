//! Crash reports.
//!
//! [`install_panic_hook`] writes a plain-text report to the crash-reports
//! folder whenever the app panics: what went wrong and where, the app
//! version, the system, the graphics card once known, the last lines of the
//! log, and a backtrace. The normal panic output still follows.
//!
//! A panic caught on purpose with [`catch_panic`] (a background task that
//! failed while the app carries on) is logged as an error, not reported as
//! a crash. [`prune_reports`] keeps only the newest reports.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::fs;
use std::io;
use std::panic::{PanicHookInfo, UnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// How many log lines a report includes.
const LOG_LINES_IN_REPORT: usize = 200;

static GPU_INFO: OnceLock<String> = OnceLock::new();

thread_local! {
    /// How many [`catch_panic`] calls this thread is inside.
    static CATCHING: Cell<u32> = const { Cell::new(0) };
}

/// Runs `work`, catching a panic in it like [`std::panic::catch_unwind`].
/// Such a panic is handled by the caller, so the panic hook logs it as an
/// error and writes no crash report.
pub fn catch_panic<R>(work: impl FnOnce() -> R + UnwindSafe) -> std::thread::Result<R> {
    /// Leaves the count as it was, however `work` ends.
    struct Inside;
    impl Drop for Inside {
        fn drop(&mut self) {
            let _ = CATCHING.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
    let _ = CATCHING.try_with(|depth| depth.set(depth.get() + 1));
    let _inside = Inside;
    std::panic::catch_unwind(work)
}

/// Whether the panic happening on this thread will be caught by [`catch_panic`].
fn panic_is_caught() -> bool {
    CATCHING.try_with(|depth| depth.get() > 0).unwrap_or(false)
}

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

/// Writes a report on every panic that is not caught with [`catch_panic`],
/// then runs the previous panic handler. The report's log lines are only
/// this process's own, kept in memory by the log writer
/// ([`crate::logging::recent_lines`]), not the end of the shared log file.
pub fn install_panic_hook(crash_dir: PathBuf, app_version: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".into());
        if panic_is_caught() {
            tracing::error!(%location, "a panic was caught and handled: {message}");
            previous(info);
            return;
        }
        let backtrace = Backtrace::force_capture().to_string();
        let log_tail = crate::logging::recent_lines(LOG_LINES_IN_REPORT);
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
        time = crate::stamp::now(),
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
    let stamp = crate::stamp::now().replace(':', "-");
    let mut path = dir.join(format!("crash-{stamp}.txt"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("crash-{stamp}-{n}.txt"));
    }
    fs::write(&path, report)?;
    Ok(path)
}

/// Removes all but the newest `keep` crash reports in `dir` and returns
/// how many were removed. Other files are left alone.
pub fn prune_reports(dir: &Path, keep: usize) -> io::Result<usize> {
    let mut reports: Vec<(SystemTime, PathBuf)> = fs::read_dir(dir)?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("crash-") && name.ends_with(".txt")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    // Newest first; names (which hold the time) settle equal times.
    reports.sort_by(|a, b| b.cmp(a));
    let mut removed = 0;
    for (_, path) in reports.iter().skip(keep) {
        fs::remove_file(path)?;
        removed += 1;
    }
    Ok(removed)
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

    #[test]
    fn pruning_keeps_the_newest_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let start = SystemTime::now() - std::time::Duration::from_secs(1000);
        for n in 0..25u64 {
            let path = tmp.path().join(format!("crash-{n:02}.txt"));
            fs::write(&path, "report").unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(start + std::time::Duration::from_secs(n))
                .unwrap();
        }
        fs::write(tmp.path().join("notes.txt"), "keep me").unwrap();

        assert_eq!(prune_reports(tmp.path(), 20).unwrap(), 5);
        let mut left: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), 21);
        assert_eq!(left[0], "crash-05.txt", "the oldest five are gone");
        assert!(left.contains(&"notes.txt".to_owned()));
        assert_eq!(prune_reports(tmp.path(), 20).unwrap(), 0);
    }

    #[test]
    fn catch_panic_marks_only_its_own_work() {
        assert!(!panic_is_caught());
        let inside = catch_panic(panic_is_caught).unwrap();
        assert!(inside);
        assert!(catch_panic(|| panic!("caught")).is_err());
        assert!(!panic_is_caught(), "the mark ends with the work");
    }
}
