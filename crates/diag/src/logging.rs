//! Rolling log files.
//!
//! One file per day in the logs folder, oldest deleted after
//! [`crate::LOG_DAYS_KEPT`] days. Warnings and errors are also printed to the
//! terminal. Set `RUST_LOG` (for example `RUST_LOG=debug` or
//! `RUST_LOG=media=trace`) to change how much is written.

use crate::{AppError, Explain, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

const FILE_PREFIX: &str = "app";
const FILE_SUFFIX: &str = "log";

/// Keeps the background log writer running. Dropping it flushes the log.
#[derive(Debug)]
pub struct LogGuard(#[allow(dead_code)] tracing_appender::non_blocking::WorkerGuard);

/// Starts logging to daily files in `logs_dir`. Call once per process.
pub fn init(logs_dir: &Path, days_kept: usize) -> Result<LogGuard> {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
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
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let to_file = fmt::layer().with_writer(writer).with_ansi(false);
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
    Ok(LogGuard(guard))
}

/// The most recently written log file in `logs_dir`, if any.
pub fn latest_log_file(logs_dir: &Path) -> Option<PathBuf> {
    fs::read_dir(logs_dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX)
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// The last `count` lines of a text file; empty if it can't be read.
pub fn tail(path: &Path, count: usize) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].iter().map(|line| (*line).to_owned()).collect()
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
    fn latest_log_file_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "").unwrap();
        assert_eq!(latest_log_file(tmp.path()), None);
        let log = tmp.path().join("app.2026-11-10.log");
        fs::write(&log, "x").unwrap();
        assert_eq!(latest_log_file(tmp.path()), Some(log));
    }
}
