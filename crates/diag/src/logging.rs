//! Rolling log files.
//!
//! One file per day in the logs folder, oldest deleted after
//! [`crate::LOG_DAYS_KEPT`] days. Warnings and errors are also printed to the
//! terminal. Set `RUST_LOG` (for example `RUST_LOG=debug` or
//! `RUST_LOG=media=trace`) to change how much is written.
//!
//! Lines are written by a thread of their own ([`crate::log_writer`]);
//! [`recent_lines`] has it catch up first, so the crash handler reads the
//! lines written just before a panic.

use crate::log_writer::BackgroundWriter;
use crate::{AppError, Explain, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

const FILE_PREFIX: &str = "app";
const FILE_SUFFIX: &str = "log";

/// How much of the end of a log file [`tail`] reads at most.
const TAIL_BYTES: u64 = 128 * 1024;

/// How long [`recent_lines`] waits for the writer to catch up.
const FLUSH_WAIT: Duration = Duration::from_millis(500);

/// How long dropping a [`LogGuard`] waits for the writer to catch up.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

/// The log this process writes, once [`init_named`] has started it.
struct Active {
    writer: BackgroundWriter,
    dir: PathBuf,
    name: String,
}

static ACTIVE: OnceLock<Active> = OnceLock::new();

/// Keeps the log going. Dropping it writes out what is still queued
/// (waiting a second at most).
#[derive(Debug)]
pub struct LogGuard(BackgroundWriter);

impl Drop for LogGuard {
    fn drop(&mut self) {
        self.0.flush(CLOSE_WAIT);
    }
}

/// Starts logging to daily files in `logs_dir`. Call once per process.
pub fn init(logs_dir: &Path, days_kept: usize) -> Result<LogGuard> {
    init_named(logs_dir, days_kept, FILE_PREFIX)
}

/// Like [`init`], for a helper program that keeps its own log files
/// (`<name>.YYYY-MM-DD.log`), such as the launcher.
pub fn init_named(logs_dir: &Path, days_kept: usize, name: &str) -> Result<LogGuard> {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(name)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(days_kept)
        .build(logs_dir)
        .or_explain(|| {
            AppError::new("Logging can't start")
                .why(format!(
                    "The log folder {} can't be written to.",
                    logs_dir.display()
                ))
                .fix("Check that the disk is not full and that you can write to this folder.")
        })?;
    let writer = BackgroundWriter::spawn(appender).or_explain(|| {
        AppError::new("Logging can't start")
            .why("The thread that writes the log could not be started.")
            .fix("Close some programs and start the app again.")
    })?;

    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let to_file = fmt::layer().with_writer(writer.clone()).with_ansi(false);
    let to_terminal = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);

    tracing_subscriber::registry()
        .with(filter)
        .with(to_file)
        .with(to_terminal)
        .try_init()
        .or_explain(|| {
            AppError::new("Logging was started twice")
                .why("The app tried to set up logging after it was already running.")
                .fix("This is a bug in the app; please report it.")
        })?;
    let _ = ACTIVE.set(Active {
        writer: writer.clone(),
        dir: logs_dir.to_path_buf(),
        name: name.to_owned(),
    });
    Ok(LogGuard(writer))
}

/// The last `count` lines of this process's log, after waiting briefly for
/// the lines still queued to reach the file. Empty before logging starts.
/// Safe to call from a panic hook: the wait is bounded and skipped on the
/// writer's own thread.
pub fn recent_lines(count: usize) -> Vec<String> {
    let Some(active) = ACTIVE.get() else {
        return Vec::new();
    };
    active.writer.flush(FLUSH_WAIT);
    latest_log_file_named(&active.dir, &active.name)
        .map(|file| tail(&file, count))
        .unwrap_or_default()
}

/// The most recently written log file in `logs_dir`, if any.
pub fn latest_log_file(logs_dir: &Path) -> Option<PathBuf> {
    latest_log_file_named(logs_dir, FILE_PREFIX)
}

/// The most recently written `<name>.YYYY-MM-DD.log` in `logs_dir`, if any.
pub fn latest_log_file_named(logs_dir: &Path, name: &str) -> Option<PathBuf> {
    let prefix = format!("{name}.");
    fs::read_dir(logs_dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            file.starts_with(&prefix) && file.ends_with(FILE_SUFFIX)
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// The last `count` lines of a text file; empty if it can't be read.
///
/// Reads only the last [`TAIL_BYTES`] of the file, and bytes that are not
/// valid UTF-8 become `\u{FFFD}` instead of losing the rest.
pub fn tail(path: &Path, count: usize) -> Vec<String> {
    tail_within(path, count, TAIL_BYTES)
}

fn tail_within(path: &Path, count: usize, max_bytes: u64) -> Vec<String> {
    let Ok((bytes, from_start)) = read_end(path, max_bytes) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if !from_start && !lines.is_empty() {
        lines.remove(0); // cut off part way through
    }
    let start = lines.len().saturating_sub(count);
    lines[start..].iter().map(|line| (*line).to_owned()).collect()
}

/// The last `max_bytes` of a file, and whether that is the whole file.
fn read_end(path: &Path, max_bytes: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    Ok((bytes, start == 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_the_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.2026-11-10.log");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(&file, 2), vec!["two", "three"]);
        assert_eq!(tail(&file, 10), vec!["one", "two", "three"]);
        assert!(tail(&tmp.path().join("missing.log"), 5).is_empty());
    }

    #[test]
    fn tail_keeps_the_lines_around_invalid_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.2026-11-10.log");
        fs::write(&file, b"one\ntw\xffo\nthree\n").unwrap();
        assert_eq!(tail(&file, 2), vec!["tw\u{FFFD}o", "three"]);
    }

    #[test]
    fn tail_reads_only_the_end_of_a_big_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.2026-11-10.log");
        let mut text = String::new();
        for n in 0..1000 {
            text.push_str(&format!("line {n}\n"));
        }
        fs::write(&file, text).unwrap();
        // 100 bytes hold the last lines and a part of one before them,
        // which is left out.
        let lines = tail_within(&file, 50, 100);
        assert_eq!(lines.last().map(String::as_str), Some("line 999"));
        assert!(lines.len() < 20, "{lines:?}");
        assert!(lines.iter().all(|line| line.starts_with("line ")), "{lines:?}");
    }

    #[test]
    fn latest_log_file_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "").unwrap();
        assert_eq!(latest_log_file(tmp.path()), None);
        let log = tmp.path().join("app.2026-11-10.log");
        fs::write(&log, "x").unwrap();
        assert_eq!(latest_log_file(tmp.path()), Some(log.clone()));
        fs::write(tmp.path().join("launcher.2026-11-10.log"), "y").unwrap();
        let launcher = latest_log_file_named(tmp.path(), "launcher");
        assert_eq!(launcher, Some(tmp.path().join("launcher.2026-11-10.log")));
    }
}
