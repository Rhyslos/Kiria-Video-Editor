//! One start's worth of updating, without a window: `--update-step <report>`.
//!
//! The release workflow's update drill installs the previous release on a
//! clean machine and runs this a few times. Each run does what a normal
//! start does about updates, in the same order and with the same code:
//!
//! 1. installs an update that was downloaded earlier,
//! 2. asks whether there is a newer version and downloads and checks it,
//! 3. starts the editor and waits for it to report that it works, which is
//!    also the moment the launcher itself is replaced by the new version's.
//!
//! The report says where things stand afterwards. The workflow refuses to
//! publish a release the previous one does not end up on, up to date.
//!
//! The report is `key=value` lines:
//!
//! ```text
//! launcher=0.1.1-beta.105        this launcher program's own version
//! installed=0.1.1-beta.107       what step 1 installed (empty: nothing waiting)
//! in_use=0.1.1-beta.107          the version the next start runs
//! check=up to date               or "downloaded <version>", "skipped <version>", "failed: <why>"
//! editor=started                 or "failed", "no answer", "not started: <why>"
//! ```

use crate::{Installed, VERSION, editor_start};
use std::ffi::OsStr;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use updater::health::StartOutcome;
use updater::{CheckResult, UpdatePrefs};

/// How long the editor may take to close after its start-up check.
const EDITOR_CLOSES_WITHIN: Duration = Duration::from_secs(60);

/// Where things stand after one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub launcher: String,
    pub installed: Option<String>,
    pub in_use: String,
    pub check: Check,
    pub editor: EditorStart,
}

/// How asking for a newer version went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    UpToDate,
    /// A newer version was found, downloaded and verified; the next start installs it.
    Downloaded(String),
    Skipped(String),
    Failed(String),
}

/// How starting the editor went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorStart {
    Started,
    Failed,
    NoAnswer,
    NotStarted(String),
}

/// Problems are reported on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Report {
    pub fn text(&self) -> String {
        let check = match &self.check {
            Check::UpToDate => "up to date".to_owned(),
            Check::Downloaded(version) => format!("downloaded {version}"),
            Check::Skipped(version) => format!("skipped {version}"),
            Check::Failed(why) => format!("failed: {}", one_line(why)),
        };
        let editor = match &self.editor {
            EditorStart::Started => "started".to_owned(),
            EditorStart::Failed => "failed".to_owned(),
            EditorStart::NoAnswer => "no answer".to_owned(),
            EditorStart::NotStarted(why) => format!("not started: {}", one_line(why)),
        };
        format!(
            "launcher={}\ninstalled={}\nin_use={}\ncheck={check}\neditor={editor}\n",
            self.launcher,
            self.installed.as_deref().unwrap_or_default(),
            self.in_use,
        )
    }

    /// Writes the report in one go, so whoever waits for the file never
    /// reads half of it.
    fn write(&self, to: &Path) -> std::io::Result<()> {
        let part = to.with_extension("part");
        std::fs::write(&part, self.text())?;
        std::fs::rename(&part, to)
    }
}

/// Runs one step and writes the report to `report`.
pub fn step(report: &Path, data_dir: Option<&Path>) -> ExitCode {
    let Some(data_dir) = data_dir else {
        eprintln!("the update step needs the data folder, which could not be opened");
        return ExitCode::FAILURE;
    };
    let installed = match crate::install_waiting_update(data_dir) {
        Ok(Installed::Nothing) => Ok(None),
        Ok(Installed::Version(version)) => Ok(Some(version)),
        // The new launcher does this step again from the top and reports.
        Ok(Installed::RestartNeeded) => return crate::restart_launcher(true),
        Err(err) => Err(err.report()),
    };
    let check = match &installed {
        Ok(_) => check(data_dir),
        Err(why) => Check::Failed(format!("installing the waiting update failed: {why}")),
    };
    let editor = start_editor(data_dir);
    let result = Report {
        launcher: VERSION.to_owned(),
        installed: installed.ok().flatten(),
        in_use: crate::version_in_use(),
        check,
        editor,
    };
    tracing::info!(?result, "update step done");
    match result.write(report) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("the report could not be written to {}: {err}", report.display());
            eprintln!("the report could not be written to {}: {err}", report.display());
            ExitCode::FAILURE
        }
    }
}

/// Asks for a newer version and, whatever the automatic setting says,
/// downloads and verifies it.
fn check(data_dir: &Path) -> Check {
    let prefs = UpdatePrefs::load(data_dir);
    match updater::release::check(&crate::version_in_use(), &prefs) {
        Ok(CheckResult::UpToDate) => Check::UpToDate,
        Ok(CheckResult::Skipped(version)) => Check::Skipped(version.to_string()),
        Ok(CheckResult::Available(release)) => match updater::fetch(&release, &data_dir.join("downloads")) {
            Ok(ready) => Check::Downloaded(ready.version),
            Err(err) => Check::Failed(err.report()),
        },
        Err(err) => Check::Failed(err.report()),
    }
}

/// Starts the editor for its start-up check, watches it the way the
/// launcher always does, and waits for it to close again.
fn start_editor(data_dir: &Path) -> EditorStart {
    let mut child = match editor_start::start_editor(data_dir, &[OsStr::new("--smoke-test")]) {
        Ok(child) => child,
        Err(err) => return EditorStart::NotStarted(err.report()),
    };
    let watched = crate::watch_editor(data_dir, &mut child);
    let deadline = Instant::now() + EDITOR_CLOSES_WITHIN;
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match watched.outcome {
        Some(StartOutcome::Started) => EditorStart::Started,
        Some(StartOutcome::Failed) => EditorStart::Failed,
        Some(StartOutcome::Unknown) => EditorStart::NoAnswer,
        None => EditorStart::NotStarted("this copy is not an installed one".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_is_one_fact_per_line() {
        let report = Report {
            launcher: "0.1.1-beta.105".into(),
            installed: Some("0.1.1-beta.107".into()),
            in_use: "0.1.1-beta.107".into(),
            check: Check::UpToDate,
            editor: EditorStart::Started,
        };
        assert_eq!(
            report.text(),
            "launcher=0.1.1-beta.105\ninstalled=0.1.1-beta.107\nin_use=0.1.1-beta.107\ncheck=up to date\neditor=started\n"
        );
    }

    #[test]
    fn problems_stay_on_their_line() {
        let report = Report {
            launcher: "0.1.0".into(),
            installed: None,
            in_use: "0.1.0".into(),
            check: Check::Failed("Updates can't be checked\n  Why: no answer".into()),
            editor: EditorStart::NotStarted("The editor\ncan't be found".into()),
        };
        let text = report.text();
        assert_eq!(text.lines().count(), 5);
        assert!(text.contains("installed=\n"));
        assert!(text.contains("check=failed: Updates can't be checked Why: no answer\n"));
        assert!(text.contains("editor=not started: The editor can't be found\n"));
        let downloaded = Report {
            check: Check::Downloaded("0.2.0".into()),
            ..report
        };
        assert!(downloaded.text().contains("check=downloaded 0.2.0\n"));
    }

    #[test]
    fn the_report_appears_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let to = tmp.path().join("step.txt");
        let report = Report {
            launcher: "0.1.0".into(),
            installed: None,
            in_use: "0.1.0".into(),
            check: Check::Skipped("0.2.0".into()),
            editor: EditorStart::NoAnswer,
        };
        report.write(&to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), report.text());
        assert!(!to.with_extension("part").exists());
    }
}
