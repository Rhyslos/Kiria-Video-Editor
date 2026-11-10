//! Logging and the panic hook are process-wide, so they are tested together
//! in their own test binary.

use diag::{AppDirs, crash, logging};
use std::fs;

#[test]
fn logs_are_written_and_a_panic_leaves_a_crash_report() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(tmp.path());
    dirs.create().unwrap();

    let guard = logging::init(&dirs.logs, 7).unwrap();
    assert!(
        logging::init(&dirs.logs, 7).is_err(),
        "a second init must be refused"
    );

    crash::install_panic_hook(dirs.crashes.clone(), dirs.logs.clone(), "9.9.9");
    tracing::info!("hello from the test");

    let result = std::panic::catch_unwind(|| panic!("deliberate test panic"));
    assert!(result.is_err());

    let reports: Vec<_> = fs::read_dir(&dirs.crashes).unwrap().flatten().collect();
    assert_eq!(reports.len(), 1, "exactly one crash report");
    let report = fs::read_to_string(reports[0].path()).unwrap();
    assert!(report.contains("deliberate test panic"));
    assert!(report.contains("9.9.9"));

    drop(guard); // flushes the background writer
    let log = logging::latest_log_file(&dirs.logs).expect("a log file");
    assert!(fs::read_to_string(log).unwrap().contains("hello from the test"));
}
