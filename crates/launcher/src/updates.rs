//! The version panel at the bottom left: checking, downloading, skipping and
//! the Automatic updates tick box. The network work runs on worker threads.

use crate::work::{Poll, Task};
use egui::RichText;
use std::path::PathBuf;
use ui_kit::{Theme, status};
use updater::{Channel, CheckResult, ReadyUpdate, Release, UpdatePrefs};

/// Where the update check has got to.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    /// Checking is turned off for this run (test runs, `--no-update-check`).
    NotChecking,
    Checking,
    UpToDate,
    /// A newer version the person chose to skip.
    Skipped(String),
    Available(Release),
    Downloading(String),
    /// Downloaded and verified; it installs once installing arrives (Phase 3).
    Ready(String),
    /// The check or download failed. Never blocks anything.
    Failed(String),
}

enum Message {
    Checked(diag::Result<CheckResult>),
    Fetched(diag::Result<ReadyUpdate>),
}

/// The version panel's state.
pub struct Updates {
    version: &'static str,
    data_dir: PathBuf,
    prefs: UpdatePrefs,
    state: UpdateState,
    task: Option<Task<Message>>,
    problem: Option<diag::AppError>,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Updates {
    /// Starts checking in the background if `check` is set.
    pub fn start(ctx: &egui::Context, version: &'static str, data_dir: PathBuf, check: bool) -> Updates {
        let prefs = UpdatePrefs::load(&data_dir);
        let mut updates = Updates {
            version,
            data_dir,
            prefs,
            state: UpdateState::NotChecking,
            task: None,
            problem: None,
        };
        if check {
            updates.check(ctx);
        }
        updates
    }

    pub fn channel(&self) -> Channel {
        self.prefs.channel
    }

    /// An error to show in the browser's error strip, once.
    pub fn take_problem(&mut self) -> Option<diag::AppError> {
        self.problem.take()
    }

    fn check(&mut self, ctx: &egui::Context) {
        let (version, prefs) = (self.version, self.prefs.clone());
        self.state = UpdateState::Checking;
        self.task = Some(Task::spawn(ctx, move || {
            Message::Checked(updater::release::check(version, &prefs))
        }));
    }

    fn download(&mut self, ctx: &egui::Context, release: Release) {
        let downloads = self.data_dir.join("downloads");
        self.state = UpdateState::Downloading(release.version.to_string());
        self.task = Some(Task::spawn(ctx, move || {
            Message::Fetched(updater::fetch(&release, &downloads))
        }));
    }

    /// Picks up finished work. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.task else {
            return;
        };
        let message = match task.poll() {
            Poll::Running => return,
            Poll::Lost => {
                self.task = None;
                self.state = UpdateState::Failed("The update check stopped unexpectedly.".into());
                return;
            }
            Poll::Done(message) => message,
        };
        self.task = None;
        match message {
            Message::Checked(Ok(CheckResult::UpToDate)) => self.state = UpdateState::UpToDate,
            Message::Checked(Ok(CheckResult::Skipped(v))) => self.state = UpdateState::Skipped(v.to_string()),
            Message::Checked(Ok(CheckResult::Available(release))) => {
                tracing::info!(version = %release.version, "an update is available");
                if self.prefs.automatic {
                    self.download(ctx, release);
                } else {
                    self.state = UpdateState::Available(release);
                }
            }
            Message::Checked(Err(err)) => {
                tracing::info!("{}", err.report());
                self.state = UpdateState::Failed(err.what().to_owned());
            }
            Message::Fetched(Ok(ready)) => {
                tracing::info!(version = %ready.version, package = %ready.package.display(), "update downloaded and verified");
                self.state = UpdateState::Ready(ready.version);
            }
            Message::Fetched(Err(err)) => {
                tracing::warn!("{}", err.report());
                self.state = UpdateState::Failed(err.what().to_owned());
                self.problem = Some(err);
            }
        }
    }

    /// Changes the update channel and checks again.
    pub fn set_channel(&mut self, ctx: &egui::Context, channel: Channel) {
        if self.prefs.channel != channel {
            self.prefs.channel = channel;
            self.save_prefs();
            if self.task.is_none() && self.state != UpdateState::NotChecking {
                self.check(ctx);
            }
        }
    }

    fn save_prefs(&mut self) {
        if let Err(err) = self.prefs.save(&self.data_dir) {
            self.problem = Some(err);
        }
    }

    /// Lets a running download finish (the window has closed). Checks are dropped.
    pub fn finish_download(self) {
        if matches!(self.state, UpdateState::Downloading(_))
            && let Some(task) = self.task
        {
            tracing::info!("finishing the update download before quitting");
            if let Some(Message::Fetched(Err(err))) = task.wait() {
                tracing::warn!("{}", err.report());
            }
        }
    }

    /// Draws the panel.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let ctx = ui.ctx().clone();
        ui.label(
            RichText::new(format!("{} {}", brand::APP_NAME, self.version))
                .family(ui_kit::fonts::semibold())
                .size(13.0),
        );
        let (line, colour) = match &self.state {
            UpdateState::NotChecking => ("Update check off for this run".to_owned(), theme.muted),
            UpdateState::Checking => ("Checking for updates…".to_owned(), theme.muted),
            UpdateState::UpToDate => ("Up to date".to_owned(), theme.muted),
            UpdateState::Skipped(v) => (format!("{v} skipped"), theme.muted),
            UpdateState::Available(r) => (format!("{} available", r.version), status::WARNING),
            UpdateState::Downloading(v) => (format!("Downloading {v}…"), theme.text_secondary),
            UpdateState::Ready(v) => (format!("{v} downloaded and verified"), status::GOOD),
            UpdateState::Failed(what) => (what.clone(), theme.muted),
        };
        let response = ui.label(RichText::new(line).size(11.5).color(colour));
        if matches!(self.state, UpdateState::Ready(_)) {
            response.on_hover_text(
                "Installing downloaded updates from here is still being built. Until then, install new versions from the Releases page.",
            );
        }

        if let UpdateState::Available(release) = &self.state {
            let release = release.clone();
            ui.horizontal(|ui| {
                if ui_kit::accent_button(ui, theme, "Update").clicked() {
                    self.download(&ctx, release.clone());
                }
                if ui.button("Skip this version").clicked() {
                    let version = release.version.to_string();
                    self.prefs.skipped_version = Some(version.clone());
                    self.save_prefs();
                    self.state = UpdateState::Skipped(version);
                }
            });
        }

        let mut automatic = self.prefs.automatic;
        if ui
            .checkbox(&mut automatic, "Automatic updates")
            .on_hover_text("Download new versions as soon as they are out.")
            .changed()
        {
            self.prefs.automatic = automatic;
            self.save_prefs();
            if automatic && let UpdateState::Available(release) = &self.state {
                let release = release.clone();
                self.download(&ctx, release);
            }
        }
    }
}
