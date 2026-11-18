# diag

Explained errors, logging and crash reports.

## Status

Done (Phase 0.2). Showing errors in the interface (inline messages with a fix button, the Diagnostics panel) comes with the window in Phase 0.6.

## Using it

At the top of a program's `main`:

```rust,ignore
let diagnostics = diag::start(brand::VERSION)?; // folders, logging, crash handler, session
// ... run the app ...
diagnostics.finish()?; // records a clean exit
```

Every function that can fail returns `diag::Result<T>`. Build errors with the four parts:

```rust,ignore
use diag::{AppError, Explain, FixAction};

std::fs::read(&path).or_explain(|| {
    AppError::new("The project can't be opened")
        .why("The file was moved or deleted.")
        .fix("Find the file and open it again, or pick it from Recent projects.")
        .action(FixAction::new("Find file", "project.locate"))
})?;
```

`or_explain` keeps the original error as the cause, so logs and bug reports still have the technical detail (`AppError::report()` prints the whole chain).

## Files

| File | What it does |
|---|---|
| `src/lib.rs` | `start()` and `Diagnostics`: sets everything up in the right order. `start_helper(name, version)` does the same for a helper such as the launcher (its own log files, crash reports) without session tracking. Both remove writes a crash left in the data folder a day or more ago (`disk::remove_old_temps`) |
| `src/error.rs` | `AppError` (what, why, fix, one-click `FixAction`, cause), `Explain`, `Result`. A `FixAction` can name the button that turns it down (`declined_with("Skip")`), shown instead of "Dismiss" |
| `src/paths.rs` | `AppDirs`: the app's data, logs and crash-reports folders, and its cache folder (proxies; under `~/.cache` on Linux, `cache` in the data folder on Windows) |
| `src/logging.rs` | Daily log files, 7 days kept; warnings also go to the terminal. `RUST_LOG` changes the level. `recent_lines` has the writer catch up (half a second at most) and reads the end of this process's own log; `tail` reads only the last 128 KB of a file, invalid bytes replaced rather than losing the rest |
| `src/log_writer.rs` | The thread that writes log lines to the file, so logging never waits for the disk; it can be asked to flush with a time limit, never waiting on its own thread (a panic while writing the log) |
| `src/crash.rs` | Panic hook that writes `crash-<time>.txt`: message, place, version, system, graphics card, last 200 log lines (written out first, so the lines just before the panic are there), backtrace. A panic caught with `catch_panic` (the jobs pool's, whose task fails while the app carries on) is logged as an error instead. `prune_reports` keeps the newest 20 (`CRASH_REPORTS_KEPT`), at start |
| `src/disk.rs` | `write_atomic`: the workspace's one writer of small files, whole or not at all (a temporary file of each write's own, `.name.<number>.saving`, put on the disk, then renamed over the old one; on Unix the folder synced too). `project::disk` re-exports it and the updater writes its state files with it. `temp_path` / `temp_number` name and recognise those temporary files; `remove_old_temps` removes the ones a crash left in a folder once a day old (`TEMP_LEFT_FOR`), done for the data folder by `start` and `start_helper` |
| `src/stamp.rs` | `now`, `at`: a moment as reports and markers write it, UTC to the second (`2026-11-17T11:08:57Z`), with the `time` crate the logging already brings |
| `src/marker.rs` | `MarkerSet`: markers kept per process (`<dir>/<process id>.<extension>`) with a lock file beside each that the process holds (`hold`, `HeldLock::release`); `found` tells the markers of processes still going from those left behind (lock free). Taking the lock never overwrites a marker left by an earlier process with the same id: it is set aside, still left behind. Used by `session` and by the graphics start markers |
| `src/session.rs` | A marker file per running process (`sessions/<process id>.running` in the data folder) that reveals a run which did not close cleanly (crash, forced quit, power cut), with a lock file beside it that the run holds while it lasts (the system lets go of it however the process ends): at start, a marker whose lock is free is a crash, reported once, even when its process id is the new run's own (given out again); markers of runs still going are left alone, so several editors can run at once. It names the process and the open project (`note_open_project`), so the launcher can read them (`unfinished_runs`), offer recovery and clear those of crashed runs (`forget_unfinished_runs`). The one marker older versions wrote (`session.running`) is still read |
| `tests/logging_and_crash.rs` | Logging, a caught and a real panic in one test binary, because both are process-wide |

## Where files go

| | Windows | Linux |
|---|---|---|
| Data folder | `%LOCALAPPDATA%\<app name>` | `~/.local/share/<app name>` |
| Logs | `<data>\logs\app.YYYY-MM-DD.log` | `<data>/logs/app.YYYY-MM-DD.log` |
| Crash reports | `<data>\crash-reports\` (newest 20) | `<data>/crash-reports/` (newest 20) |

## Known limits

- The graphics line in crash reports says "not detected yet" until the window is open: the `graphics` crate then calls `crash::set_gpu_info` with the graphics system and card in use.
