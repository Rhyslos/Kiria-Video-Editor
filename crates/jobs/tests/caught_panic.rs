//! The crash handler is process-wide, so this test has a binary of its own.

use jobs::{BackgroundPolicy, JobSystem, PoolKind, Priority, Resolved};

#[test]
fn a_panicking_job_is_an_error_not_a_crash_report() {
    let tmp = tempfile::tempdir().unwrap();
    let crashes = tmp.path().join("crash-reports");
    diag::crash::install_panic_hook(crashes.clone(), "0.0.0");

    let jobs = JobSystem::new(Resolved {
        worker_threads: 1,
        ram_cache_mb: 64,
        disk_cache_gb: 1,
        background: BackgroundPolicy::Always,
    });
    let handle = jobs.submit(
        PoolKind::Analysis,
        Priority::Interactive,
        "breaks",
        |_| -> diag::Result<()> { panic!("deliberate job panic") },
    );
    assert!(handle.wait().is_err(), "the panic comes back as an error");

    let reports = std::fs::read_dir(&crashes).map(|dir| dir.count()).unwrap_or(0);
    assert_eq!(reports, 0, "a caught panic writes no crash report");

    // A panic nobody marked as caught still writes one.
    let _ = std::panic::catch_unwind(|| panic!("deliberate uncaught panic"));
    assert_eq!(std::fs::read_dir(&crashes).unwrap().count(), 1);
}
