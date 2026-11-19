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

    crash::install_panic_hook(dirs.crashes.clone(), "9.9.9");

    // A panic caught on purpose is logged, not reported.
    assert!(crash::catch_panic(|| panic!("deliberate caught panic")).is_err());
    assert_eq!(fs::read_dir(&dirs.crashes).unwrap().count(), 0, "no report");

    tracing::info!("hello from the test");
    tracing::info!("the last line before the panic");
    // Another editor appends to the same daily file; its lines are not
    // this process's and stay out of the report.
    let shared = logging::latest_log_file(&dirs.logs).expect("the shared log file");
    let others: String = (0..300).map(|n| format!("another editor's line {n}\n")).collect();
    fs::OpenOptions::new()
        .append(true)
        .open(&shared)
        .and_then(|mut file| std::io::Write::write_all(&mut file, others.as_bytes()))
        .unwrap();
    let result = std::panic::catch_unwind(|| panic!("deliberate test panic"));
    assert!(result.is_err());

    let reports: Vec<_> = fs::read_dir(&dirs.crashes).unwrap().flatten().collect();
    assert_eq!(reports.len(), 1, "exactly one crash report");
    let report = fs::read_to_string(reports[0].path()).unwrap();
    assert!(report.contains("deliberate test panic"));
    assert!(report.contains("9.9.9"));
    assert!(
        report.contains("the last line before the panic"),
        "the report has the log written just before the panic:\n{report}"
    );
    assert!(
        report.contains("a panic was caught and handled: deliberate caught panic"),
        "the caught panic is in the log:\n{report}"
    );
    assert!(
        !report.contains("another editor's line"),
        "the report has only this process's lines:\n{report}"
    );

    drop(guard); // flushes the background writer
    let log = logging::latest_log_file(&dirs.logs).expect("a log file");
    assert!(fs::read_to_string(log).unwrap().contains("hello from the test"));
}
