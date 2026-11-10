//! Did a version start? Rolling back one that doesn't.
//!
//! The editor writes a small "started" file once its first window has been
//! drawn ([`diag::session::report_started`]). The launcher removes that file, starts the
//! editor, and waits up to [`START_TIMEOUT`] for it ([`watch_start`]):
//!
//! - the file appears: the version works. Old versions beyond the previous two
//!   are removed ([`prune_after_good_start`]).
//! - the editor exits first: a failed start. After [`FAILURES_BEFORE_ROLLBACK`]
//!   in a row the launcher switches back to the newest earlier version that
//!   isn't bad, marks the failed one bad so it's never offered again, and
//!   leaves a [`RollbackNotice`] for the next launcher window to explain.
//! - neither within the time limit: nothing is concluded (a slow machine).
//!
//! Counts live in `health.ron` in the data folder.

use crate::install::Layout;
use crate::prefs::UpdatePrefs;
use diag::{AppError, Explain};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

/// How long the launcher waits for the editor's first window.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);
/// Failed starts in a row before switching back to the previous version.
pub const FAILURES_BEFORE_ROLLBACK: u32 = 2;
/// Earlier versions kept for rolling back.
pub const VERSIONS_KEPT: usize = 2;

const HEALTH_FILE: &str = "health.ron";
const STARTED_FILE: &str = diag::session::STARTED_FILE;
const NOTICE_FILE: &str = "rollback.ron";

/// What happened to one start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    /// The editor ended before its first window appeared.
    Failed,
    /// Still starting when the time ran out; nothing concluded.
    Unknown,
}

/// The launcher's side: waits for `child` (the editor) to report that it
/// started. Remove any old report with [`clear_started`] before starting it.
pub fn watch_start(data_dir: &Path, child: &mut Child, timeout: Duration) -> StartOutcome {
    let deadline = Instant::now() + timeout;
    let expected_pid = child.id().to_string();
    loop {
        if let Ok(text) = std::fs::read_to_string(data_dir.join(STARTED_FILE))
            && text.split_whitespace().nth(1) == Some(expected_pid.as_str())
        {
            return StartOutcome::Started;
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                // It may have reported just before closing.
                let reported = std::fs::read_to_string(data_dir.join(STARTED_FILE))
                    .is_ok_and(|t| t.split_whitespace().nth(1) == Some(expected_pid.as_str()));
                return if reported {
                    StartOutcome::Started
                } else {
                    StartOutcome::Failed
                };
            }
            Ok(None) => {}
            Err(_) => return StartOutcome::Unknown,
        }
        if Instant::now() >= deadline {
            return StartOutcome::Unknown;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Removes an old "started" report before starting the editor again.
pub fn clear_started(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(STARTED_FILE));
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct VersionHealth {
    failed_in_a_row: u32,
    started_ok: bool,
}

/// Start results per version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    versions: BTreeMap<String, VersionHealth>,
}

impl Health {
    pub fn load(data_dir: &Path) -> Health {
        std::fs::read_to_string(data_dir.join(HEALTH_FILE))
            .ok()
            .and_then(|t| ron::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("The update safety record can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full.")
        };
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(explain)?;
        std::fs::write(data_dir.join(HEALTH_FILE), text).or_explain(explain)
    }

    /// Records a start; returns the failed starts in a row for that version.
    pub fn record(&mut self, version: &str, outcome: StartOutcome) -> u32 {
        let entry = self.versions.entry(version.to_owned()).or_default();
        match outcome {
            StartOutcome::Started => {
                entry.failed_in_a_row = 0;
                entry.started_ok = true;
            }
            StartOutcome::Failed => entry.failed_in_a_row += 1,
            StartOutcome::Unknown => {}
        }
        entry.failed_in_a_row
    }
}

/// Explains a rollback to the person, the next time the launcher opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackNotice {
    /// The version that failed to start.
    pub failed: String,
    /// The version now in use.
    pub restored: String,
}

impl RollbackNotice {
    /// Reads and removes the notice, so it is shown once.
    pub fn take(data_dir: &Path) -> Option<RollbackNotice> {
        let path = data_dir.join(NOTICE_FILE);
        let notice = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| ron::from_str(&t).ok());
        let _ = std::fs::remove_file(path);
        notice
    }

    fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let text =
            ron::ser::to_string(self).or_explain(|| AppError::new("The rollback note can't be written"))?;
        std::fs::write(data_dir.join(NOTICE_FILE), text).or_explain(|| {
            AppError::new("The rollback note can't be written").fix("Check that the disk is not full.")
        })
    }
}

/// The newest installed version, other than `failed`, that isn't marked bad.
fn rollback_target(layout: &Layout, failed: &str, bad: &[String]) -> Option<String> {
    installed_versions(layout)
        .into_iter()
        .filter(|v| v.to_string() != failed && !bad.contains(&v.to_string()))
        .max()
        .map(|v| v.to_string())
}

/// Every version folder, by version number.
fn installed_versions(layout: &Layout) -> Vec<Version> {
    std::fs::read_dir(layout.root().join(crate::install::VERSIONS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(crate::install::editor_file_name()).is_file())
        .filter_map(|e| Version::parse(&e.file_name().to_string_lossy()).ok())
        .collect()
}

/// Records how the start of `version` went, and rolls back after repeated
/// failures. Returns the notice if it rolled back.
pub fn after_start(
    data_dir: &Path,
    layout: &Layout,
    version: &str,
    outcome: StartOutcome,
) -> diag::Result<Option<RollbackNotice>> {
    let mut health = Health::load(data_dir);
    let failures = health.record(version, outcome);
    health.save(data_dir)?;
    match outcome {
        StartOutcome::Started => {
            prune_after_good_start(layout, version);
            Ok(None)
        }
        StartOutcome::Failed if failures >= FAILURES_BEFORE_ROLLBACK => {
            let mut prefs = UpdatePrefs::load(data_dir);
            let Some(target) = rollback_target(layout, version, &prefs.bad_versions) else {
                tracing::warn!(
                    version,
                    "the editor keeps failing to start and there is no earlier version"
                );
                return Ok(None);
            };
            tracing::warn!(failed = version, restored = %target, "rolling back");
            crate::apply::set_current(layout, &target)?;
            if !prefs.bad_versions.iter().any(|v| v == version) {
                prefs.bad_versions.push(version.to_owned());
            }
            prefs.save(data_dir)?;
            let notice = RollbackNotice {
                failed: version.to_owned(),
                restored: target,
            };
            notice.save(data_dir)?;
            Ok(Some(notice))
        }
        StartOutcome::Failed | StartOutcome::Unknown => Ok(None),
    }
}

/// After `current` started fine: keeps it and the [`VERSIONS_KEPT`] newest
/// earlier versions, removes the rest (and any newer leftovers that aren't current).
pub fn prune_after_good_start(layout: &Layout, current: &str) {
    let Ok(current_v) = Version::parse(current) else {
        return;
    };
    let mut earlier: Vec<Version> = installed_versions(layout)
        .into_iter()
        .filter(|v| *v < current_v)
        .collect();
    earlier.sort();
    let keep: Vec<String> = earlier
        .iter()
        .rev()
        .take(VERSIONS_KEPT)
        .map(ToString::to_string)
        .chain([current.to_owned()])
        .collect();
    let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
    crate::apply::prune(layout, &keep);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::editor_file_name;

    fn install(layout: &Layout, versions: &[&str]) {
        for v in versions {
            let dir = layout.version_dir(v);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(editor_file_name()), "").unwrap();
        }
    }

    fn installed(layout: &Layout) -> Vec<String> {
        let mut v: Vec<String> = installed_versions(layout)
            .iter()
            .map(ToString::to_string)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn two_failed_starts_roll_back_and_mark_the_version_bad() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("app"));
        std::fs::create_dir_all(&data).unwrap();
        let layout = Layout::at(&root);
        install(&layout, &["0.1.0", "0.2.0"]);
        crate::apply::set_current(&layout, "0.2.0").unwrap();

        assert_eq!(
            after_start(&data, &layout, "0.2.0", StartOutcome::Failed).unwrap(),
            None
        );
        assert_eq!(
            layout.current_version().as_deref(),
            Some("0.2.0"),
            "one failure is not enough"
        );

        let notice = after_start(&data, &layout, "0.2.0", StartOutcome::Failed)
            .unwrap()
            .unwrap();
        assert_eq!(notice.failed, "0.2.0");
        assert_eq!(notice.restored, "0.1.0");
        assert_eq!(layout.current_version().as_deref(), Some("0.1.0"));
        assert_eq!(UpdatePrefs::load(&data).bad_versions, ["0.2.0"]);
        assert_eq!(RollbackNotice::take(&data), Some(notice));
        assert_eq!(RollbackNotice::take(&data), None, "shown once");
    }

    #[test]
    fn a_good_start_resets_the_count() {
        let mut health = Health::default();
        assert_eq!(health.record("0.2.0", StartOutcome::Failed), 1);
        assert_eq!(health.record("0.2.0", StartOutcome::Started), 0);
        assert_eq!(health.record("0.2.0", StartOutcome::Failed), 1);
        assert_eq!(health.record("0.2.0", StartOutcome::Unknown), 1);
    }

    #[test]
    fn no_rollback_without_an_earlier_good_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("app"));
        std::fs::create_dir_all(&data).unwrap();
        let layout = Layout::at(&root);
        install(&layout, &["0.1.0", "0.2.0"]);
        crate::apply::set_current(&layout, "0.2.0").unwrap();
        UpdatePrefs {
            bad_versions: vec!["0.1.0".into()],
            ..UpdatePrefs::default()
        }
        .save(&data)
        .unwrap();
        after_start(&data, &layout, "0.2.0", StartOutcome::Failed).unwrap();
        assert_eq!(
            after_start(&data, &layout, "0.2.0", StartOutcome::Failed).unwrap(),
            None
        );
        assert_eq!(layout.current_version().as_deref(), Some("0.2.0"));
    }

    #[test]
    fn a_good_start_keeps_the_two_previous_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        install(&layout, &["0.1.0", "0.1.1", "0.2.0", "0.3.0", "0.4.0-beta.1"]);
        prune_after_good_start(&layout, "0.3.0");
        assert_eq!(installed(&layout), ["0.1.1", "0.2.0", "0.3.0"]);
    }

    #[test]
    fn the_started_report_is_matched_to_the_process() {
        let tmp = tempfile::tempdir().unwrap();
        clear_started(tmp.path());
        // A process that exits at once without reporting: a failed start.
        let mut quick = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert_eq!(
            watch_start(tmp.path(), &mut quick, Duration::from_secs(20)),
            StartOutcome::Failed
        );

        // A report from that process id counts.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(tmp.path().join(STARTED_FILE), format!("0.2.0 {}\n", child.id())).unwrap();
        assert_eq!(
            watch_start(tmp.path(), &mut child, Duration::from_secs(20)),
            StartOutcome::Started
        );
    }
}
