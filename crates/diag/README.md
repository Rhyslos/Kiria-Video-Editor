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
| `src/lib.rs` | `start()` and `Diagnostics`: sets everything up in the right order |
| `src/error.rs` | `AppError` (what, why, fix, one-click `FixAction`, cause), `Explain`, `Result`. A `FixAction` can name the button that turns it down (`declined_with("Skip")`), shown instead of "Dismiss" |
| `src/paths.rs` | `AppDirs`: the app's data, logs and crash-reports folders, and its cache folder (proxies; under `~/.cache` on Linux, `cache` in the data folder on Windows) |
| `src/logging.rs` | Daily log files, 7 days kept; warnings also go to the terminal. `RUST_LOG` changes the level |
| `src/crash.rs` | Panic hook that writes `crash-<time>.txt`: message, place, version, system, graphics card, last 200 log lines, backtrace |
| `src/session.rs` | A marker file per running process (`sessions/<process id>.running` in the data folder) that reveals a run which did not close cleanly (crash, forced quit, power cut), with a lock file beside it that the run holds while it lasts (the system lets go of it however the process ends): at start, a marker whose lock is free is a crash, reported once; markers of runs still going are left alone, so several editors can run at once. It names the process and the open project (`note_open_project`), so the launcher can read them (`unfinished_runs`), offer recovery and clear those of crashed runs (`forget_unfinished_runs`). The one marker older versions wrote (`session.running`) is still read |
| `tests/logging_and_crash.rs` | Logging and a real panic in one test binary, because both are process-wide |

## Where files go

| | Windows | Linux |
|---|---|---|
| Data folder | `%LOCALAPPDATA%\<app name>` | `~/.local/share/<app name>` |
| Logs | `<data>\logs\app.YYYY-MM-DD.log` | `<data>/logs/app.YYYY-MM-DD.log` |
| Crash reports | `<data>\crash-reports\` | `<data>/crash-reports/` |

## Known limits

- The graphics line in crash reports says "not detected yet" until the window is open: the `graphics` crate then calls `crash::set_gpu_info` with the graphics system and card in use.
