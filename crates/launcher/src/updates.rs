//! Updates in the launcher: the version panel at the bottom left and the
//! update window that opens when a new version is found.
//!
//! The window walks through three steps: "Update available" (Download or
//! Skip this version), a download bar, then "Update ready" (Restart now or
//! Later). With automatic updates on it opens straight at the download bar.
//! Closing it never cancels anything; the version panel keeps the same
//! choices. The network work runs on worker threads, and the choices are
//! written to `updates.ron` on a thread of their own (a `project::disk::Keeper`).
//!
//! Another launcher (another project open at the same time) may write
//! `updates.ron` too, above all to mark a version that failed to start as
//! bad. So the panel never writes back the copy it read at start: each
//! write reads the file afresh and lays only the choices made in this
//! window over it ([`Changes`]), holding the lock between launchers from
//! the read to the write (`updater::UpdatePrefs::change`, on the keeper's
//! thread), and each check reads it afresh too, so a version marked bad
//! elsewhere is never offered or written away.

use crate::work::{Poll, Task};
use egui::RichText;
use project::disk::Keeper;
use std::path::PathBuf;
use ui_kit::{Theme, status};
use updater::{Channel, CheckResult, Progress, ReadyUpdate, Release, UpdateLock, UpdatePrefs};

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
    /// Downloaded and verified; it installs on the next start.
    Ready(String),
    /// The check or download failed. Never blocks anything.
    Failed(String),
    /// Another launcher (another project opened at the same time) is
    /// updating, so this one leaves it alone this time.
    Elsewhere,
}

enum Message {
    Checked(diag::Result<CheckResult>),
    Fetched(diag::Result<ReadyUpdate>),
    /// Not downloaded: another launcher holds the update lock.
    Elsewhere,
}

/// The version panel's state.
pub struct Updates {
    /// The version in use (see `updater::apply::InstallKind::version_in_use`).
    version: String,
    data_dir: PathBuf,
    /// The choices as this window shows them: the file as read at start,
    /// with this window's own changes.
    prefs: UpdatePrefs,
    /// The choices made in this window, laid over the file's newest ones.
    changes: Changes,
    /// Writes `updates.ron` off the interface thread, newest choices last.
    prefs_keeper: Keeper,
    state: UpdateState,
    task: Option<Task<Message>>,
    problem: Option<diag::AppError>,
    /// The running (or last) download's progress.
    progress: Progress,
    /// Whether the update window is open.
    window_open: bool,
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
    pub fn start(ctx: &egui::Context, version: String, data_dir: PathBuf, check: bool) -> Updates {
        let prefs = UpdatePrefs::load(&data_dir);
        let prefs_keeper = Keeper::new(
            data_dir.join(updater::prefs::PREFS_FILE_NAME),
            "Your update choices",
        );
        let mut updates = Updates {
            version,
            data_dir,
            prefs,
            changes: Changes::default(),
            prefs_keeper,
            state: UpdateState::NotChecking,
            task: None,
            problem: None,
            progress: Progress::new(),
            window_open: false,
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
        let (version, data_dir, changes) =
            (self.version.clone(), self.data_dir.clone(), self.changes.clone());
        self.state = UpdateState::Checking;
        self.task = Some(Task::spawn(ctx, move || {
            // Read afresh: another launcher may have marked a version bad.
            let prefs = changes.over(UpdatePrefs::load(&data_dir));
            Message::Checked(updater::release::check(&version, &prefs))
        }));
    }

    fn download(&mut self, ctx: &egui::Context, release: Release) {
        let downloads = self.data_dir.join("downloads");
        self.state = UpdateState::Downloading(release.version.to_string());
        self.progress = Progress::new();
        let progress = self.progress.clone();
        let data_dir = self.data_dir.clone();
        self.task = Some(Task::spawn(ctx, move || {
            let Some(_lock) = UpdateLock::for_step(&data_dir, "downloading an update") else {
                return Message::Elsewhere;
            };
            Message::Fetched(updater::fetch_with_progress(&release, &downloads, &progress))
        }));
    }

    fn skip(&mut self, release: &Release) {
        let version = release.version.to_string();
        self.prefs.skipped_version = Some(version.clone());
        self.changes.skipped_version = Some(Some(version.clone()));
        self.save_prefs();
        self.state = UpdateState::Skipped(version);
    }

    /// Picks up finished work. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(err) = self.prefs_keeper.take_failure() {
            self.problem = Some(err);
        }
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
                self.window_open = true;
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
                // The restart prompt, even if the download window was hidden.
                self.window_open = true;
            }
            Message::Elsewhere => {
                // Nothing to show: the other launcher's download is the same update.
                self.state = UpdateState::Elsewhere;
                self.window_open = false;
            }
            Message::Fetched(Err(err)) => {
                tracing::warn!("{}", err.report());
                self.state = UpdateState::Failed(err.what().to_owned());
                // Shown in the update window if it is open, else in the error strip.
                if !self.window_open {
                    self.problem = Some(err);
                }
            }
        }
    }

    /// Changes the update channel and checks again.
    pub fn set_channel(&mut self, ctx: &egui::Context, channel: Channel) {
        if self.prefs.channel != channel {
            self.prefs.channel = channel;
            self.changes.channel = Some(channel);
            self.save_prefs();
            if self.task.is_none() && self.state != UpdateState::NotChecking {
                self.check(ctx);
            }
        }
    }

    /// Hands the choices to the keeper, which writes them whole on its own
    /// thread; a failure shows on a later frame ([`Updates::poll`]). The
    /// keeper reads the file afresh and changes only what this window
    /// changed, under the lock between launchers, so what another launcher
    /// wrote meanwhile (a version marked bad) is kept. Every change made
    /// here goes each time, since the keeper drops a change still waiting
    /// for a newer one.
    fn save_prefs(&mut self) {
        let (data_dir, changes) = (self.data_dir.clone(), self.changes.clone());
        self.prefs_keeper.keep_written(move || {
            UpdatePrefs::change(&data_dir, |prefs| *prefs = changes.over(std::mem::take(prefs))).map(drop)
        });
    }

    /// Lets a running download finish (the window has closed), and the
    /// choices reach the disk. Checks are dropped.
    pub fn finish_download(self) {
        if let Err(err) = self.prefs_keeper.flush() {
            tracing::warn!("{}", err.report());
        }
        if matches!(self.state, UpdateState::Downloading(_))
            && let Some(task) = self.task
        {
            tracing::info!("finishing the update download before quitting");
            if let Some(Message::Fetched(Err(err))) = task.wait() {
                tracing::warn!("{}", err.report());
            }
        }
    }

    /// Draws the panel. Returns true when "Restart to update" was clicked.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) -> bool {
        let mut restart = false;
        let ctx = ui.ctx().clone();
        ui.label(
            RichText::new(format!("{} {}", brand::APP_NAME, self.version))
                .family(ui_kit::fonts::semibold())
                .size(13.0),
        );
        let (line, colour) = self.state_line(theme);
        ui.label(RichText::new(line).size(11.5).color(colour));
        if matches!(self.state, UpdateState::Ready(_))
            && ui_kit::accent_button(ui, theme, "Restart to update")
                .on_hover_text("Installs it now. Otherwise it installs the next time you open the app.")
                .clicked()
        {
            restart = true;
        }

        if let UpdateState::Available(release) = &self.state {
            let release = release.clone();
            ui.horizontal(|ui| {
                if ui_kit::accent_button(ui, theme, "Update").clicked() {
                    self.download(&ctx, release.clone());
                }
                if ui.button("Skip this version").clicked() {
                    self.skip(&release);
                }
            });
        }

        let mut automatic = self.prefs.automatic;
        if ui
            .checkbox(&mut automatic, "Automatic updates")
            .on_hover_text("Download new versions as soon as they are out.")
            .changed()
        {
            self.set_automatic(&ctx, automatic);
        }
        restart
    }

    /// Whether new versions download as soon as they are out.
    pub fn automatic(&self) -> bool {
        self.prefs.automatic
    }

    /// Turns automatic updates on or off (the panel's tick and Settings'
    /// switch); on, a version waiting to be downloaded starts downloading.
    pub fn set_automatic(&mut self, ctx: &egui::Context, automatic: bool) {
        if self.prefs.automatic == automatic {
            return;
        }
        self.prefs.automatic = automatic;
        self.changes.automatic = Some(automatic);
        self.save_prefs();
        if automatic && let UpdateState::Available(release) = &self.state {
            let release = release.clone();
            self.download(ctx, release);
        }
    }

    /// The version in use, as the panel names it.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Where updating stands, in a line, and its colour.
    pub fn state_line(&self, theme: &Theme) -> (String, egui::Color32) {
        match &self.state {
            UpdateState::NotChecking => ("Update check off for this run".to_owned(), theme.muted),
            UpdateState::Checking => ("Checking for updates…".to_owned(), theme.muted),
            UpdateState::UpToDate => ("Up to date".to_owned(), theme.muted),
            UpdateState::Skipped(v) => (format!("{v} skipped"), theme.muted),
            UpdateState::Available(r) => (format!("{} available", r.version), status::WARNING),
            UpdateState::Downloading(v) => (
                match self.progress.fraction() {
                    Some(f) => format!("Downloading {v}… {:.0} %", f * 100.0),
                    None => format!("Downloading {v}…"),
                },
                theme.text_secondary,
            ),
            UpdateState::Ready(v) => (format!("{v} downloaded and verified"), status::GOOD),
            UpdateState::Failed(what) => (what.clone(), theme.muted),
            UpdateState::Elsewhere => ("Updating in another window".to_owned(), theme.muted),
        }
    }

    /// Opens the window at `step` with a made-up next version, for screenshots.
    pub fn preview(&mut self, step: crate::run_mode::UpdatePreview) {
        use crate::run_mode::UpdatePreview;
        let next = "0.2.0".to_owned();
        self.window_open = true;
        self.state = match step {
            UpdatePreview::Available => UpdateState::Available(Release {
                version: semver::Version::new(0, 2, 0),
                prerelease: false,
                notes: String::new(),
                assets: Vec::new(),
            }),
            UpdatePreview::Downloading => {
                self.progress.set(41_300_000, 63_500_000);
                UpdateState::Downloading(next)
            }
            UpdatePreview::Ready => UpdateState::Ready(next),
        };
    }

    /// Draws the update window when it is open. Returns true when "Restart now" was clicked.
    pub fn show_window(&mut self, ctx: &egui::Context, theme: &Theme) -> bool {
        if !self.window_open {
            return false;
        }
        if matches!(self.state, UpdateState::Downloading(_)) {
            // Keep the bar moving.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut restart = false;
        let mut close = false;
        let modal = ui_kit::dialog(ctx, theme, "update-window", |ui| {
            ui.set_width(380.0);
            match self.state.clone() {
                UpdateState::Available(release) => {
                    heading(ui, "Update available");
                    body(
                        ui,
                        theme,
                        &format!(
                            "{} {} is out. You have {}.",
                            brand::APP_NAME,
                            release.version,
                            self.version
                        ),
                    );
                    ui.add_space(14.0);
                    buttons(ui, |ui| {
                        if ui_kit::accent_button(ui, theme, "Download").clicked() {
                            self.download(ctx, release.clone());
                        }
                        if ui.button("Skip this version").clicked() {
                            self.skip(&release);
                            close = true;
                        }
                    });
                }
                UpdateState::Downloading(version) => {
                    heading(ui, &format!("Downloading {version}"));
                    let fraction = self.progress.fraction();
                    let (done, total) = self.progress.bytes();
                    let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
                        .fill(theme.accent)
                        .desired_height(8.0)
                        .corner_radius(4.0);
                    ui.add_space(4.0);
                    ui.add(if fraction.is_none() {
                        bar.animate(true)
                    } else {
                        bar
                    });
                    let detail = if total > 0 {
                        format!("{} of {} MB", mb(done), mb(total))
                    } else {
                        "Starting…".to_owned()
                    };
                    body(ui, theme, &detail);
                    ui.add_space(10.0);
                    buttons(ui, |ui| {
                        if ui
                            .button("Hide")
                            .on_hover_text(
                                "The download carries on; the panel at the bottom left shows how far it is.",
                            )
                            .clicked()
                        {
                            close = true;
                        }
                    });
                }
                UpdateState::Ready(version) => {
                    heading(ui, "Update ready");
                    body(
                        ui,
                        theme,
                        &format!("{version} is downloaded and checked. Restart to start using it."),
                    );
                    ui.add_space(14.0);
                    buttons(ui, |ui| {
                        if ui_kit::accent_button(ui, theme, "Restart now").clicked() {
                            restart = true;
                        }
                        if ui
                            .button("Later")
                            .on_hover_text("It installs the next time you open the app.")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                }
                UpdateState::Failed(what) => {
                    heading(ui, "The update didn't finish");
                    body(
                        ui,
                        theme,
                        &format!("{what} Nothing changed; the app tries again next time."),
                    );
                    ui.add_space(14.0);
                    buttons(ui, |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                }
                UpdateState::NotChecking
                | UpdateState::Checking
                | UpdateState::UpToDate
                | UpdateState::Skipped(_)
                | UpdateState::Elsewhere => close = true,
            }
        });
        if close || modal.should_close() {
            self.window_open = false;
        }
        restart
    }
}

/// The update choices made in one window, each `None` until it is changed
/// there. Versions marked bad are never among them: only the start check
/// (`updater::health::after_start`) adds those, and they are always kept.
#[derive(Debug, Clone, Default, PartialEq)]
struct Changes {
    automatic: Option<bool>,
    channel: Option<Channel>,
    skipped_version: Option<Option<String>>,
}

impl Changes {
    /// `newest` (the file as it is now) with these changes laid over it.
    fn over(&self, mut newest: UpdatePrefs) -> UpdatePrefs {
        if let Some(automatic) = self.automatic {
            newest.automatic = automatic;
        }
        if let Some(channel) = self.channel {
            newest.channel = channel;
        }
        if let Some(skipped) = &self.skipped_version {
            newest.skipped_version.clone_from(skipped);
        }
        newest
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).family(ui_kit::fonts::semibold()).size(16.0));
    ui.add_space(6.0);
}

fn body(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label(RichText::new(text).color(theme.text_secondary));
}

/// Buttons on the right, main action first (rightmost).
fn buttons(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
}

/// Bytes as megabytes with one decimal.
fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megabytes_read_naturally() {
        assert_eq!(mb(0), "0.0");
        assert_eq!(mb(63_500_000), "63.5");
    }

    #[test]
    fn choices_are_written_off_the_interface_thread_and_kept_on_closing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut updates = Updates::start(&ctx, "0.1.0".into(), tmp.path().to_path_buf(), false);
        updates.set_automatic(&ctx, false);
        updates.set_channel(&ctx, Channel::Beta);
        updates.finish_download();
        let saved = UpdatePrefs::load(tmp.path());
        assert!(!saved.automatic);
        assert_eq!(saved.channel, Channel::Beta);
    }

    /// Another launcher's write while this one is open: this window's
    /// choices go over it, and nothing else of it is lost.
    fn marked_bad_elsewhere(data_dir: &std::path::Path) {
        let mut other = UpdatePrefs::load(data_dir);
        other.bad_versions.push("0.2.0".into());
        other.save(data_dir).unwrap();
    }

    #[test]
    fn a_version_marked_bad_by_another_launcher_is_never_written_away() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut updates = Updates::start(&ctx, "0.1.0".into(), tmp.path().to_path_buf(), false);
        // Another launcher rolls 0.2.0 back after this window read the file.
        marked_bad_elsewhere(tmp.path());
        updates.set_automatic(&ctx, false);
        updates.prefs_keeper.flush().unwrap();
        let saved = UpdatePrefs::load(tmp.path());
        assert_eq!(saved.bad_versions, ["0.2.0"]);
        assert!(!saved.automatic);
        // Every later choice keeps it too.
        updates.set_channel(&ctx, Channel::Beta);
        updates.skip(&Release {
            version: semver::Version::new(0, 3, 0),
            prerelease: false,
            notes: String::new(),
            assets: Vec::new(),
        });
        updates.finish_download();
        let saved = UpdatePrefs::load(tmp.path());
        assert_eq!(saved.bad_versions, ["0.2.0"]);
        assert!(!saved.automatic);
        assert_eq!(saved.channel, Channel::Beta);
        assert_eq!(saved.skipped_version.as_deref(), Some("0.3.0"));
    }

    #[test]
    fn a_check_reads_the_versions_marked_bad_afresh() {
        let tmp = tempfile::tempdir().unwrap();
        let changes = Changes {
            channel: Some(Channel::Beta),
            ..Changes::default()
        };
        marked_bad_elsewhere(tmp.path());
        let prefs = changes.over(UpdatePrefs::load(tmp.path()));
        assert_eq!(prefs.bad_versions, ["0.2.0"]);
        assert_eq!(prefs.channel, Channel::Beta);
        // Choices not made in this window are the file's.
        assert!(prefs.automatic);
        assert_eq!(prefs.skipped_version, None);
    }

    #[test]
    fn choices_made_in_another_window_are_kept_unless_changed_here() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut updates = Updates::start(&ctx, "0.1.0".into(), tmp.path().to_path_buf(), false);
        let mut other = UpdatePrefs::load(tmp.path());
        other.channel = Channel::Beta;
        other.save(tmp.path()).unwrap();
        updates.set_automatic(&ctx, false);
        updates.finish_download();
        let saved = UpdatePrefs::load(tmp.path());
        assert_eq!(saved.channel, Channel::Beta);
        assert!(!saved.automatic);
    }
}
