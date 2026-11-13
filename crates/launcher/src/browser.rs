//! The launcher window: folder tree, project grid and version panel.

use crate::cards::{self, CardAction};
use crate::delete::Bin;
use crate::delete_dialog::{self, DeleteDialog};
use crate::editor_start::{show_in_file_manager, start_editor};
use crate::files;
use crate::new_project::{Choice, NewProjectDialog};
use crate::prefs::LauncherPrefs;
use crate::recovery::{self, LastRun};
use crate::run_mode::RunMode;
use crate::updates::Updates;
use crate::work::{Poll, Task};
use diag::AppError;
use egui::{RichText, Sense};
use project_index::{Folder, FolderKind, Index, Issue, ProjectEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use ui_kit::{Action, ErrorCenter, ErrorResponse, ShortcutTable, Theme, TitleBar, layout};

mod settings_ui;

/// Height of the version panel at the bottom of the left panel.
const VERSION_PANEL_HEIGHT: f32 = 128.0;

/// The outcome of an automated run, read by `main` after the window closes.
#[derive(Debug, Default)]
pub struct RunReport {
    pub failed: bool,
    pub message: String,
}

/// What the grid is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    All,
    Folder(Folder),
}

/// A file or folder chosen in a system dialog.
#[derive(Debug)]
enum Picked {
    ProjectFile(Option<PathBuf>),
    Folder(FolderKind, Option<PathBuf>),
    /// Browse… in the New project dialog.
    SaveFolder(Option<PathBuf>),
    /// Change… for the default folder in Settings.
    DefaultFolder(Option<PathBuf>),
}

/// A finished rename or move: the old path and where the project went.
type FileJob = Task<diag::Result<(PathBuf, files::Moved)>>;

/// "Recover the unsaved work?" for one project.
#[derive(Debug, Clone)]
struct RecoveryDialog {
    path: PathBuf,
    name: String,
    unsaved: usize,
    /// Shown at start-up because the editor crashed (rather than on opening).
    after_crash: bool,
}

#[derive(Debug)]
struct RenameDialog {
    path: PathBuf,
    name: String,
    focused: bool,
}

/// Everything the window needs at start.
#[derive(Debug)]
pub struct Start {
    pub data_dir: PathBuf,
    pub theme: ui_kit::ThemeName,
    /// The shortcuts changed in the editor, so Search and Settings are on
    /// the same keys here.
    pub shortcuts: ui_kit::shortcuts::SavedBindings,
    pub check_updates: bool,
    /// Start the editor when a project is opened (off in test runs).
    pub launch_editor: bool,
    pub mode: RunMode,
    pub report: Arc<Mutex<RunReport>>,
    /// Receives the version panel when the window closes, so a running download can finish.
    pub handoff: Arc<Mutex<Option<Updates>>>,
    /// Things that happened before the window opened (an update was installed, or failed).
    pub notes: Vec<crate::Note>,
    /// Set when the person asks to restart into a downloaded update.
    pub restart: Arc<AtomicBool>,
    /// Opens the update window at a sample step (screenshots).
    pub update_preview: Option<crate::run_mode::UpdatePreview>,
    /// Receives the editor process once a project is opened, so `main` can
    /// check that it starts after the window has closed.
    pub editor: Arc<Mutex<Option<std::process::Child>>>,
    /// Where deleted projects go: the recycle bin, or a stand-in folder for sample projects.
    pub bin: Bin,
    /// Opens the Delete project question for a sample project (screenshots).
    pub delete_preview: bool,
    /// Opens Settings at this tab or setting (`--settings updates`, screenshots).
    pub settings_at: Option<String>,
}

pub struct Browser {
    theme: Theme,
    /// The theme's name, as Settings shows it (it follows the editor's).
    theme_name: ui_kit::ThemeName,
    data_dir: PathBuf,
    prefs: LauncherPrefs,
    index_file: PathBuf,
    index: Option<Index>,
    errors: ErrorCenter,
    shortcuts: ShortcutTable,
    show_diagnostics: bool,
    show_settings: bool,
    /// Settings' tab, search and row to jump to.
    settings_view: ui_kit::settings::SettingsView,
    /// What draws the window: the first-frames check, the question after a
    /// failed start with Vulkan, and the Graphics row in Settings.
    graphics: graphics::WindowGraphics,
    /// The user guide in the web browser (F1, Settings › Help).
    user_guide: ui_kit::user_guide::UserGuide,
    folders: Vec<Folder>,
    projects: Vec<ProjectEntry>,
    view: View,
    search: String,
    focus_search: bool,
    selected: Option<PathBuf>,
    status: String,
    refresh: Option<Task<diag::Result<HashMap<PathBuf, cards::Notes>>>>,
    /// Projects with unsaved changes left by a crash, and how many.
    /// Unsaved changes from a crash and missing media, per project.
    notes: HashMap<PathBuf, cards::Notes>,
    thumbnails: crate::thumbnails::Thumbnails,
    recovery: Option<RecoveryDialog>,
    /// Shown once after a version failed to start and the previous one came back.
    rollback: Option<updater::health::RollbackNotice>,
    picker: Option<Task<Picked>>,
    file_job: Option<FileJob>,
    new_project: Option<NewProjectDialog>,
    rename: Option<RenameDialog>,
    delete: DeleteDialog,
    bin: Bin,
    updates: Option<Updates>,
    launch_editor: bool,
    mode: RunMode,
    frames: u64,
    report: Arc<Mutex<RunReport>>,
    handoff: Arc<Mutex<Option<Updates>>>,
    restart: Arc<AtomicBool>,
    editor: Arc<Mutex<Option<std::process::Child>>>,
    screenshot_requested: bool,
    screenshot_saved: Arc<AtomicBool>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("data_dir", &self.data_dir)
            .field("projects", &self.projects.len())
            .finish_non_exhaustive()
    }
}

impl Browser {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start, opened: graphics::Opened) -> Browser {
        let ctx = &cc.egui_ctx;
        ui_kit::setup(ctx, start.theme);
        let index_file = start.data_dir.join(project_index::INDEX_FILE_NAME);
        let mut errors = ErrorCenter::default();
        let index = match Index::open(&index_file) {
            Ok(index) => Some(index),
            Err(err) => {
                errors.push(&err);
                None
            }
        };
        let graphics = graphics::WindowGraphics::new(cc, opened, &mut errors);
        let mut updates = Updates::start(
            ctx,
            crate::version_in_use(),
            start.data_dir.clone(),
            start.check_updates,
        );
        if let Some(step) = start.update_preview {
            updates.preview(step);
        }
        let mut settings_view = ui_kit::settings::SettingsView::default();
        if let Some(id) = &start.settings_at
            && !settings_view.open_at(settings_ui::CATEGORIES, id)
        {
            tracing::warn!("--settings: no tab or setting is called {id}");
        }
        // Installed, the launcher sits above the versions: the guide is in
        // the current version's folder (`ui_kit::user_guide`).
        let layout = updater::install::Layout::at(crate::install_root());
        let user_guide =
            ui_kit::user_guide::UserGuide::also_in(layout.current_version().map(|v| layout.version_dir(&v)));
        let mut browser = Browser {
            theme: start.theme.theme(),
            theme_name: start.theme,
            prefs: LauncherPrefs::load(&start.data_dir),
            data_dir: start.data_dir,
            index_file,
            index,
            errors,
            shortcuts: ShortcutTable::restored(&start.shortcuts),
            show_diagnostics: false,
            show_settings: start.settings_at.is_some(),
            settings_view,
            graphics,
            user_guide,
            folders: Vec::new(),
            projects: Vec::new(),
            view: View::All,
            search: String::new(),
            focus_search: false,
            selected: None,
            status: String::new(),
            refresh: None,
            notes: HashMap::new(),
            thumbnails: Default::default(),
            recovery: None,
            rollback: None,
            picker: None,
            file_job: None,
            new_project: None,
            rename: None,
            delete: DeleteDialog::default(),
            bin: start.bin,
            updates: Some(updates),
            launch_editor: start.launch_editor,
            mode: start.mode,
            frames: 0,
            report: start.report,
            handoff: start.handoff,
            restart: start.restart,
            editor: start.editor,
            screenshot_requested: false,
            screenshot_saved: Arc::new(AtomicBool::new(false)),
        };
        for note in start.notes {
            match note {
                crate::Note::Updated(version) => {
                    browser.status = format!("Updated to {version}.");
                }
                crate::Note::Problem(err) => browser.errors.push(&err),
                crate::Note::RolledBack(notice) => browser.rollback = Some(notice),
            }
        }
        browser.add_default_folder();
        browser.reload();
        browser.check_last_run();
        browser.start_refresh(ctx);
        if start.delete_preview {
            browser.preview_delete(ctx);
        }
        browser
    }

    /// Opens the Delete project question for the sample project with the
    /// most files beside it (screenshots; nothing is deleted without a click).
    fn preview_delete(&mut self, ctx: &egui::Context) {
        let sample = self
            .projects
            .iter()
            .filter(|p| p.issue.is_none())
            .max_by_key(|p| project::side_files::existing(&p.path).len());
        if let Some(sample) = sample {
            let (path, name) = (sample.path.clone(), sample.name.clone());
            self.selected = Some(path.clone());
            self.delete.begin(ctx, &self.data_dir, path, name);
        }
    }

    /// On the first run, makes a project folder in the person's documents.
    fn add_default_folder(&mut self) {
        let Some(index) = &self.index else {
            return;
        };
        let has_project_folder = index
            .folders()
            .is_ok_and(|f| f.iter().any(|f| f.kind == FolderKind::Projects));
        if has_project_folder {
            return;
        }
        let documents = directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| self.data_dir.clone());
        let folder = documents.join(format!("{} projects", brand::APP_NAME));
        let result = std::fs::create_dir_all(&folder)
            .map_err(|err| {
                AppError::new("The projects folder can't be made")
                    .why(format!("{} could not be created.", folder.display()))
                    .fix("Add a folder of your own with + Add folder.")
                    .source(err)
            })
            .and_then(|()| index.add_folder(&folder, FolderKind::Projects));
        match result {
            Ok(()) if self.prefs.default_project_folder.is_none() => self.set_default_folder(Some(folder)),
            Ok(()) => {}
            Err(err) => self.errors.push(&err),
        }
    }

    /// Re-reads the list from the index. One quick query; safe every frame it's needed.
    fn reload(&mut self) {
        let Some(index) = &self.index else {
            return;
        };
        match index.folders().and_then(|f| Ok((f, index.projects()?))) {
            Ok((folders, projects)) => {
                self.folders = folders;
                self.projects = projects;
                if let View::Folder(folder) = &self.view
                    && !self.folders.contains(folder)
                {
                    self.view = View::All;
                }
            }
            Err(err) => self.errors.push(&err),
        }
    }

    /// Looks through the folders on disk on a worker thread.
    fn start_refresh(&mut self, ctx: &egui::Context) {
        if self.refresh.is_some() || self.index.is_none() {
            return;
        }
        let file = self.index_file.clone();
        self.refresh = Some(Task::spawn(ctx, move || {
            let index = Index::open(&file)?;
            index.refresh()?;
            // Only each file's short summary and journal are read, so this stays quick.
            Ok(index
                .projects()?
                .into_iter()
                .filter(|p| p.issue.is_none())
                .filter_map(|p| {
                    let notes = cards::Notes {
                        unsaved: recovery::unsaved_changes(&p.path),
                        missing_media: missing_media(&p.path),
                    };
                    (notes != cards::Notes::default()).then_some((p.path, notes))
                })
                .collect())
        }));
    }

    /// Says what happened if the editor didn't close properly last time.
    fn check_last_run(&mut self) {
        if self.rollback.is_some() {
            // The crash was the failed start that the rollback notice explains.
            if let Err(err) = recovery::forget_crashed_runs(&self.data_dir) {
                tracing::warn!("{}", err.report());
            }
            return;
        }
        let crashed = match recovery::last_run(&self.data_dir) {
            LastRun::Fine => return,
            LastRun::CrashedNothingLost => {
                self.status = format!(
                    "{} closed unexpectedly last time. Nothing was lost.",
                    brand::APP_NAME
                );
                true
            }
            LastRun::CrashedWithWork { project, unsaved } => {
                self.recovery = Some(RecoveryDialog {
                    name: self.project_name(&project),
                    path: project,
                    unsaved,
                    after_crash: true,
                });
                true
            }
        };
        // Dealt with here, so the editor doesn't report it again.
        if crashed && let Err(err) = recovery::forget_crashed_runs(&self.data_dir) {
            tracing::warn!("{}", err.report());
        }
    }

    fn project_name(&self, path: &Path) -> String {
        self.projects
            .iter()
            .find(|p| p.path == path)
            .map(|p| p.name.clone())
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default()
    }

    fn with_index(&mut self, f: impl FnOnce(&Index) -> diag::Result<()>) {
        let result = match &self.index {
            Some(index) => f(index),
            None => Err(AppError::new("The project list is not available")
                .why("It could not be opened when the launcher started.")
                .fix("Close the launcher and open it again.")),
        };
        if let Err(err) = result {
            self.errors.push(&err);
        }
        self.reload();
    }

    /// Picks up finished background work.
    fn poll_work(&mut self, ctx: &egui::Context) {
        // A file chooser that could not open: said in the New project
        // dialog when it asked, else in the error strip.
        if let Some(problem) = ui_kit::file_dialog::take_problem(ctx) {
            match &mut self.new_project {
                Some(dialog) => dialog.set_problem(&problem),
                None => self.errors.push(&problem),
            }
        }
        if let Some(task) = &self.refresh {
            match task.poll() {
                Poll::Running => {}
                Poll::Done(result) => {
                    self.refresh = None;
                    match result {
                        Ok(notes) => self.notes = notes,
                        Err(err) => self.errors.push(&err),
                    }
                    self.reload();
                }
                Poll::Lost => self.refresh = None,
            }
        }
        if let Some(task) = &self.picker {
            match task.poll() {
                Poll::Running => {}
                Poll::Done(picked) => {
                    self.picker = None;
                    self.picked(ctx, picked);
                }
                Poll::Lost => self.picker = None,
            }
        }
        if let Some(task) = &self.file_job {
            match task.poll() {
                Poll::Running => {}
                Poll::Done(result) => {
                    self.file_job = None;
                    self.status.clear();
                    match result {
                        Ok((from, moved)) => {
                            self.with_index(|index| index.moved(&from, &moved.to).map(|_| ()));
                            self.selected = Some(moved.to);
                            if let Some(err) = moved.left_behind {
                                self.errors.push(&err);
                            }
                        }
                        Err(err) => self.errors.push(&err),
                    }
                }
                Poll::Lost => {
                    self.file_job = None;
                    self.status.clear();
                }
            }
        }
        if let Some(updates) = &mut self.updates {
            updates.poll(ctx);
            if let Some(err) = updates.take_problem() {
                self.errors.push(&err);
            }
        }
    }

    fn picked(&mut self, ctx: &egui::Context, picked: Picked) {
        match picked {
            Picked::ProjectFile(Some(path)) => {
                let mut recorded = false;
                self.with_index(|index| index.record(&path).map(|_| recorded = true));
                if recorded {
                    self.open(ctx, &path);
                }
            }
            Picked::Folder(kind, Some(path)) => {
                self.with_index(|index| index.add_folder(&path, kind));
                self.view = View::Folder(Folder { path, kind });
                self.start_refresh(ctx);
            }
            Picked::SaveFolder(Some(path)) => {
                self.with_index(|index| index.add_folder_if_new(&path, FolderKind::Projects));
                if let Some(dialog) = &mut self.new_project {
                    dialog.set_folder(path);
                }
            }
            Picked::DefaultFolder(Some(path)) => {
                self.with_index(|index| index.add_folder_if_new(&path, FolderKind::Projects));
                self.set_default_folder(Some(path));
                self.start_refresh(ctx);
            }
            Picked::ProjectFile(None)
            | Picked::Folder(_, None)
            | Picked::SaveFolder(None)
            | Picked::DefaultFolder(None) => {}
        }
    }

    /// Remembers where new projects go.
    fn set_default_folder(&mut self, folder: Option<PathBuf>) {
        self.prefs.default_project_folder = folder;
        if let Err(err) = self.prefs.save(&self.data_dir) {
            self.errors.push(&err);
        }
    }

    /// Opens the system folder dialog; `make` wraps the answer.
    fn pick_any_folder(&mut self, ctx: &egui::Context, title: &str, make: fn(Option<PathBuf>) -> Picked) {
        if self.picker.is_some() {
            return;
        }
        let (title, start) = (title.to_owned(), self.default_project_folder());
        let asker = ctx.clone();
        self.picker = Some(Task::spawn(ctx, move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(start) = start.filter(|s| s.is_dir()) {
                dialog = dialog.set_directory(start);
            }
            make(ui_kit::file_dialog::run(&asker, || dialog.pick_folder()))
        }));
    }

    fn pick_project_file(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() {
            return;
        }
        let start = self.default_project_folder();
        let asker = ctx.clone();
        self.picker = Some(Task::spawn(ctx, move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title("Open a project")
                .add_filter(format!("{} project", brand::APP_NAME), &[project::FILE_EXTENSION]);
            if let Some(start) = start.filter(|s| s.is_dir()) {
                dialog = dialog.set_directory(start);
            }
            Picked::ProjectFile(ui_kit::file_dialog::run(&asker, || dialog.pick_file()))
        }));
    }

    fn pick_folder(&mut self, ctx: &egui::Context, kind: FolderKind) {
        if self.picker.is_some() {
            return;
        }
        let title = format!("Add a folder to {}", kind.label());
        let asker = ctx.clone();
        self.picker = Some(Task::spawn(ctx, move || {
            Picked::Folder(
                kind,
                ui_kit::file_dialog::run(&asker, || rfd::FileDialog::new().set_title(title).pick_folder()),
            )
        }));
    }

    /// The folder chosen for new projects, or else the first project folder.
    fn default_project_folder(&self) -> Option<PathBuf> {
        self.prefs.default_project_folder.clone().or_else(|| {
            self.folders
                .iter()
                .find(|f| f.kind == FolderKind::Projects)
                .map(|f| f.path.clone())
        })
    }

    /// Where the New project dialog starts: the folder being viewed if it
    /// holds projects, else the chosen folder for new projects.
    fn new_project_folder(&self) -> Option<PathBuf> {
        if let View::Folder(folder) = &self.view
            && folder.kind.holds_projects()
        {
            return Some(folder.path.clone());
        }
        self.default_project_folder()
    }

    /// Opens a project in the editor and closes the launcher.
    fn open(&mut self, ctx: &egui::Context, path: &Path) {
        let issue = self
            .projects
            .iter()
            .find(|p| p.path == path)
            .and_then(|p| p.issue);
        // A project from a newer version opens read-only in the editor.
        if let Some(issue) = issue.filter(|i| *i != Issue::NewerVersion) {
            self.errors.push(&cannot_open(issue, path));
            return;
        }
        // Read fresh: the list may be older than the file.
        let unsaved = recovery::unsaved_changes(path);
        if unsaved > 0 {
            self.recovery = Some(RecoveryDialog {
                path: path.to_path_buf(),
                name: self.project_name(path),
                unsaved,
                after_crash: false,
            });
            return;
        }
        self.launch(ctx, path, false);
    }

    /// Starts the editor with `path` and closes the launcher. With
    /// `open_last_save`, unsaved work from a crash is set aside instead of recovered.
    fn launch(&mut self, ctx: &egui::Context, path: &Path, open_last_save: bool) {
        let mut args = vec![path.as_os_str()];
        if open_last_save {
            args.push(std::ffi::OsStr::new("--open-last-save"));
        }
        if self.launch_editor {
            match start_editor(&self.data_dir, &args) {
                Ok(child) => {
                    let mut started = self.editor.lock().unwrap_or_else(|p| p.into_inner());
                    *started = Some(child);
                }
                Err(err) => {
                    self.errors.push(&err);
                    return;
                }
            }
        }
        if let Some(index) = &self.index
            && let Err(err) = index.mark_opened(path, project_index::now())
        {
            tracing::warn!("{}", err.report());
        }
        if self.launch_editor {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            let how = if open_last_save { " as last saved" } else { "" };
            self.status = format!("Would open {}{how} in the editor.", path.display());
            self.reload();
        }
    }

    fn card_actions(&mut self, ctx: &egui::Context, actions: Vec<CardAction>) {
        for action in actions {
            match action {
                CardAction::Select(path) => self.selected = Some(path),
                CardAction::Open(path) => self.open(ctx, &path),
                CardAction::Rename(path) => {
                    let name = self
                        .projects
                        .iter()
                        .find(|p| p.path == path)
                        .map_or_else(String::new, |p| p.name.clone());
                    self.rename = Some(RenameDialog {
                        path,
                        name,
                        focused: false,
                    });
                }
                CardAction::MoveTo { project, folder } => {
                    let data_dir = self.data_dir.clone();
                    self.run_file_job(ctx, "Moving…", move || {
                        recovery::refuse_if_open_in_editor(&data_dir, &project, "moved")?;
                        files::move_project(&project, &folder).map(|moved| (project, moved))
                    });
                }
                CardAction::ShowInFileManager(path) => {
                    if let Err(err) = show_in_file_manager(&path) {
                        self.errors.push(&err);
                    }
                }
                CardAction::Forget(path) => {
                    self.with_index(|index| index.forget(&path));
                    if self.selected.as_deref() == Some(path.as_path()) {
                        self.selected = None;
                    }
                }
                CardAction::Delete(path) => {
                    let name = self.project_name(&path);
                    if self.file_job.is_some() || !self.delete.begin(ctx, &self.data_dir, path, name) {
                        self.status = "Wait for the last change to finish.".into();
                    }
                }
            }
        }
    }

    fn run_file_job(
        &mut self,
        ctx: &egui::Context,
        status: &str,
        job: impl FnOnce() -> diag::Result<(PathBuf, files::Moved)> + Send + 'static,
    ) {
        if self.file_job.is_some() || self.delete.is_working() {
            self.status = "Wait for the last change to finish.".into();
            return;
        }
        self.status = status.into();
        self.file_job = Some(Task::spawn(ctx, job));
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let split = full.max.y - VERSION_PANEL_HEIGHT;
        let tree_rect = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, split));
        let version_rect = egui::Rect::from_min_max(egui::pos2(full.min.x, split), full.max);

        ui.scope_builder(egui::UiBuilder::new().max_rect(tree_rect.shrink(12.0)), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.folder_tree(ui, &ctx);
                });
        });
        ui.painter()
            .hline(full.x_range(), split, egui::Stroke::new(1.0, self.theme.line));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(version_rect.shrink2(egui::vec2(16.0, 14.0))),
            |ui| {
                if let Some(updates) = &mut self.updates
                    && updates.show(ui, &self.theme)
                {
                    // The new version installs as the launcher starts again.
                    self.restart.store(true, Ordering::Relaxed);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );
    }

    fn folder_tree(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.y = 2.0;
        if tree_row(ui, &self.theme, "All projects", None, self.view == View::All).clicked() {
            self.view = View::All;
        }
        let mut remove = None;
        let mut reveal = None;
        let mut make_default = None;
        let default_folder = self.default_project_folder();
        for kind in FolderKind::ALL {
            ui.add_space(12.0);
            ui.label(
                RichText::new(kind.label().to_uppercase())
                    .size(10.5)
                    .color(self.theme.faint),
            );
            ui.add_space(2.0);
            let folders: Vec<Folder> = self.folders.iter().filter(|f| f.kind == kind).cloned().collect();
            if folders.is_empty() {
                ui.label(RichText::new("None yet").size(11.5).color(self.theme.faint));
            }
            for folder in folders {
                let selected = self.view == View::Folder(folder.clone());
                let is_default = default_folder.as_ref() == Some(&folder.path);
                let badge = is_default.then_some("new projects");
                let response = tree_row(ui, &self.theme, &folder.name(), badge, selected);
                if response.clicked() {
                    self.view = View::Folder(folder.clone());
                }
                response.context_menu(|ui| {
                    if folder.kind.holds_projects()
                        && !is_default
                        && ui
                            .button("Save new projects here")
                            .on_hover_text("New project will suggest this folder.")
                            .clicked()
                    {
                        make_default = Some(folder.path.clone());
                    }
                    if ui.button("Show in file manager").clicked() {
                        reveal = Some(folder.path.clone());
                    }
                    if ui
                        .button("Remove from list")
                        .on_hover_text("The folder and its files stay where they are.")
                        .clicked()
                    {
                        remove = Some(folder.path.clone());
                    }
                });
                response.on_hover_text_at_pointer(folder.path.display().to_string());
            }
        }
        ui.add_space(12.0);
        ui.menu_button(
            RichText::new("+ Add folder").color(self.theme.text_secondary),
            |ui| {
                for kind in FolderKind::ALL {
                    if ui.button(format!("To {}", kind.label())).clicked() {
                        self.pick_folder(ctx, kind);
                        ui.close();
                    }
                }
            },
        );
        if let Some(path) = reveal
            && let Err(err) = show_in_file_manager(&path)
        {
            self.errors.push(&err);
        }
        if let Some(path) = remove {
            self.with_index(|index| index.remove_folder(&path));
            if self.prefs.default_project_folder.as_ref() == Some(&path) {
                self.set_default_folder(None);
            }
        }
        if let Some(path) = make_default {
            self.set_default_folder(Some(path));
        }
    }

    fn main_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(response) = self.errors.show_inline(ui, &self.theme) {
            match response {
                ErrorResponse::OpenDiagnostics => self.show_diagnostics = true,
                ErrorResponse::Fix(action) => self.status = format!("No handler for {} yet.", action.command),
            }
            ui.add_space(8.0);
        }

        let (title, folder) = match &self.view {
            View::All => ("All projects".to_owned(), None),
            View::Folder(f) => (f.name(), Some(f.clone())),
        };
        let folder_path = folder.as_ref().map(|f| f.path.clone());
        let count = cards::filter(&self.projects, folder_path.as_deref(), &self.search).len();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&title).family(ui_kit::fonts::semibold()).size(18.0));
            if folder.as_ref().is_none_or(|f| f.kind.holds_projects()) {
                ui.label(RichText::new(count.to_string()).color(self.theme.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, &self.theme, "New project").clicked() {
                    self.new_project = Some(NewProjectDialog::new(
                        self.new_project_folder(),
                        self.default_project_folder(),
                    ));
                }
                if ui
                    .button("Open…")
                    .on_hover_text("Open a project file from anywhere")
                    .clicked()
                {
                    self.pick_project_file(&ctx);
                }
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(ui_kit::with_keys(
                            "Search projects",
                            &self.shortcuts.keys_for(Action::Search),
                        ))
                        .desired_width(220.0),
                );
                if std::mem::take(&mut self.focus_search) {
                    search.request_focus();
                }
            });
        });
        if let Some(folder) = &folder {
            ui.label(
                RichText::new(folder.path.display().to_string())
                    .size(11.5)
                    .color(self.theme.faint),
            );
        }
        let status = self.delete.status().unwrap_or_else(|| self.status.clone());
        if !status.is_empty() {
            ui.label(RichText::new(status).color(self.theme.text_secondary));
        }
        ui.add_space(12.0);

        if folder.as_ref().is_some_and(|f| f.kind == FolderKind::Media) {
            empty_state(
                ui,
                &self.theme,
                "A media folder",
                "Media folders are remembered so importing can start there, once the Library page arrives. Projects are not listed here.",
            );
            return;
        }
        let shown = cards::filter(&self.projects, folder_path.as_deref(), &self.search);
        if shown.is_empty() {
            let (head, body) = if !self.search.trim().is_empty() {
                (
                    "No projects match",
                    "Try another word, or clear the search box.".to_owned(),
                )
            } else if self.refresh.is_some() {
                ("Looking for projects…", String::new())
            } else {
                (
                    "No projects yet",
                    "Make one with New project, or open a project file with the Open… button.".to_owned(),
                )
            };
            empty_state(ui, &self.theme, head, &body);
            return;
        }
        let actions = cards::grid(
            ui,
            &self.theme,
            &shown,
            self.selected.as_deref(),
            &self.folders,
            &self.notes,
            &mut self.thumbnails,
            project_index::now(),
        );
        self.card_actions(&ctx, actions);
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = &mut self.new_project {
            let folders: Vec<PathBuf> = self
                .folders
                .iter()
                .filter(|f| f.kind.holds_projects())
                .map(|f| f.path.clone())
                .collect();
            match dialog.show(ctx, &self.theme, &folders) {
                None => {}
                Some(Choice::Cancel) => self.new_project = None,
                Some(Choice::Browse) => {
                    self.pick_any_folder(ctx, "Save the new project in", Picked::SaveFolder);
                }
                Some(Choice::Create {
                    name,
                    settings,
                    folder,
                    remember,
                }) => {
                    self.new_project = None;
                    if remember {
                        self.set_default_folder(Some(folder.clone()));
                    }
                    match files::create_project(&folder, &name, settings) {
                        Ok(path) => {
                            self.with_index(|index| index.record(&path).map(|_| ()));
                            self.selected = Some(path.clone());
                            self.open(ctx, &path);
                        }
                        Err(err) => self.errors.push(&err),
                    }
                }
            }
        }

        if let Some(notice) = self.rollback.clone() {
            let mut close = false;
            let modal = ui_kit::dialog(ctx, &self.theme, "rolled-back", |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(format!("{} went back to {}", brand::APP_NAME, notice.restored))
                        .family(ui_kit::fonts::semibold())
                        .size(16.0),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Version {} couldn't start, twice in a row, so {} switched back to {}, which worked before. {} won't be offered again; the next version will be. Your projects are not affected.",
                        notice.failed,
                        brand::APP_NAME,
                        notice.restored,
                        notice.failed
                    ))
                    .color(self.theme.text_secondary),
                );
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui_kit::accent_button(ui, &self.theme, "OK").clicked() {
                        close = true;
                    }
                    if ui
                        .button("Report the problem")
                        .on_hover_text("Opens a new issue on the project's GitHub page. Attach the crash report if there is one.")
                        .clicked()
                    {
                        let url = format!(
                            "https://github.com/{}/{}/issues/new",
                            brand::RELEASE_REPO_OWNER,
                            brand::RELEASE_REPO_NAME
                        );
                        if let Err(err) = crate::editor_start::open_in_browser(&url) {
                            self.errors.push(&err);
                        }
                    }
                    if ui.button("Show crash reports").clicked() {
                        let folder = diag::AppDirs::locate().map(|d| d.crashes);
                        if let Err(err) = folder.and_then(|f| show_in_file_manager(&f)) {
                            self.errors.push(&err);
                        }
                    }
                });
            });
            if close || modal.should_close() {
                self.rollback = None;
            }
        }

        if let Some(dialog) = self.recovery.clone() {
            let mut choice = None;
            let modal = ui_kit::dialog(ctx, &self.theme, "recover-project", |ui| {
                ui.set_width(400.0);
                let title = if dialog.after_crash {
                    format!("{} closed unexpectedly", brand::APP_NAME)
                } else {
                    "Recover unsaved work?".to_owned()
                };
                ui.label(RichText::new(title).family(ui_kit::fonts::semibold()).size(16.0));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(unsaved_says(&dialog.name, dialog.unsaved))
                        .color(self.theme.text_secondary),
                );
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui_kit::accent_button(ui, &self.theme, "Recover and open").clicked() {
                        choice = Some(Some(false));
                    }
                    if ui
                        .button("Open last save")
                        .on_hover_text("Opens the project as it was last saved. The unsaved changes are kept in a file beside it, never deleted.")
                        .clicked()
                    {
                        choice = Some(Some(true));
                    }
                    if ui.button("Not now").clicked() {
                        choice = Some(None);
                    }
                });
            });
            if modal.should_close() && choice.is_none() {
                choice = Some(None);
            }
            if let Some(choice) = choice {
                self.recovery = None;
                if let Some(open_last_save) = choice {
                    self.launch(ctx, &dialog.path, open_last_save);
                }
            }
        }

        // One question at a time: this one waits for the two above.
        if self.rollback.is_none()
            && self.recovery.is_none()
            && let Some(err) = self.graphics.show_warning(ctx, &self.theme)
        {
            self.errors.push(&err);
        }

        if let Some(dialog) = &mut self.rename {
            let mut done = false;
            let mut confirmed = false;
            let modal = ui_kit::dialog(ctx, &self.theme, "rename-project", |ui| {
                ui.set_width(360.0);
                ui.label(
                    RichText::new("Rename project")
                        .family(ui_kit::fonts::semibold())
                        .size(16.0),
                );
                ui.add_space(8.0);
                let field = ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(f32::INFINITY));
                if !dialog.focused {
                    field.request_focus();
                    dialog.focused = true;
                }
                ui.label(
                    RichText::new("The file is renamed to match.")
                        .size(11.5)
                        .color(self.theme.muted),
                );
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui_kit::accent_button(ui, &self.theme, "Rename").clicked() || enter {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            });
            if modal.should_close() {
                done = true;
            }
            if confirmed {
                let (path, name) = (dialog.path.clone(), dialog.name.clone());
                let data_dir = self.data_dir.clone();
                self.rename = None;
                self.run_file_job(ctx, "Renaming…", move || {
                    recovery::refuse_if_open_in_editor(&data_dir, &path, "renamed")?;
                    files::rename_project(&path, &name).map(|moved| (path, moved))
                });
            } else if done {
                self.rename = None;
            }
        }

        match self.delete.show(ctx, &self.theme, &self.bin) {
            None => {}
            Some(delete_dialog::Event::Failed(err)) => self.errors.push(&err),
            Some(delete_dialog::Event::Deleted { project, name, moved }) => {
                self.with_index(|index| index.deleted(&project));
                self.notes.remove(&project);
                if self.selected.as_deref() == Some(project.as_path()) {
                    self.selected = None;
                }
                self.status = if moved == 0 {
                    format!("{name} was taken off the list.")
                } else {
                    format!("{name} is in the {}.", Bin::NAME)
                };
            }
        }

        self.settings_window(ctx);
        if let Some(err) = self.user_guide.poll() {
            self.errors.push(&err);
        }

        let mut diagnostics = self.show_diagnostics;
        self.errors.diagnostics_window(ctx, &mut diagnostics, &self.theme);
        self.show_diagnostics = diagnostics;
    }

    /// Opens the user guide in the web browser (F1).
    fn open_user_guide(&mut self, ctx: &egui::Context) {
        match self.user_guide.open(ctx) {
            Ok(_) => self.status = "Opening the user guide in your web browser.".into(),
            Err(err) => self.errors.push(&err),
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let dialog_open = self.new_project.is_some()
            || self.rename.is_some()
            || self.delete.is_asking()
            || self.recovery.is_some()
            || self.rollback.is_some()
            || self.graphics.is_asking();
        for action in self.shortcuts.pressed(ctx) {
            match action {
                Action::OpenSettings => self.show_settings = true,
                Action::Search if !dialog_open => self.focus_search = true,
                Action::UserGuide => self.open_user_guide(ctx),
                _ => {}
            }
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        if !dialog_open
            && !typing
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(path) = self.selected.clone()
        {
            self.open(ctx, &path);
        }
    }

    /// Automated runs: smoke test and screenshot.
    fn automation(&mut self, ctx: &egui::Context) {
        match &self.mode {
            RunMode::Normal => {}
            RunMode::SmokeTest => {
                ctx.request_repaint();
                if self.frames >= 30 && self.refresh.is_none() && !self.delete.is_working() {
                    let message = format!(
                        "Launcher smoke test passed: {} frames drawn, {} projects listed.",
                        self.frames,
                        self.projects.len()
                    );
                    self.finish(ctx, false, message);
                }
            }
            RunMode::Screenshot(path) => {
                ctx.request_repaint();
                if self.frames >= 10
                    && self.refresh.is_none()
                    && !self.delete.is_working()
                    && !self.screenshot_requested
                {
                    self.screenshot_requested = true;
                    let (path, saved, report) = (
                        path.clone(),
                        Arc::clone(&self.screenshot_saved),
                        Arc::clone(&self.report),
                    );
                    ctx.request_screenshot(move |image| {
                        if let Err(err) = save_png(&image, &path) {
                            let mut report = report.lock().unwrap_or_else(|p| p.into_inner());
                            report.failed = true;
                            report.message = err;
                        }
                        saved.store(true, Ordering::Relaxed);
                    });
                }
                if self.screenshot_saved.load(Ordering::Relaxed) {
                    let failed = self.report.lock().map(|r| r.failed).unwrap_or(true);
                    let message = format!("Screenshot saved to {}", path.display());
                    self.finish(ctx, failed, message);
                }
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context, failed: bool, message: String) {
        let mut report = self.report.lock().unwrap_or_else(|p| p.into_inner());
        report.failed |= failed;
        if report.message.is_empty() {
            report.message = message;
        }
        drop(report);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Browser {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.graphics.frame(&ctx, self.frames);
        self.poll_work(&ctx);
        self.keyboard(&ctx);

        let mut settings = false;
        egui::Panel::top("title-bar")
            .exact_size(layout::TOP_BAR_HEIGHT)
            .frame(egui::Frame::new().fill(self.theme.panel))
            .show(ui, |ui| {
                settings = TitleBar {
                    title: brand::APP_NAME,
                    settings_keys: &self.shortcuts.keys_for(Action::OpenSettings),
                }
                .show(ui, &self.theme);
            });
        self.show_settings |= settings;
        egui::Panel::left("folders")
            .exact_size(layout::LEFT_PANEL_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(self.theme.panel))
            .show(ui, |ui| self.left_panel(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| self.main_area(ui));
        self.dialogs(&ctx);
        if let Some(updates) = &mut self.updates
            && updates.show_window(&ctx, &self.theme)
        {
            self.restart.store(true, Ordering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        self.frames += 1;
        self.automation(&ctx);
        // Keep "last used" times and the update line current without input.
        ctx.request_repaint_after(std::time::Duration::from_secs(30));
    }

    fn on_exit(&mut self) {
        if let Some(updates) = self.updates.take() {
            let mut handoff = self.handoff.lock().unwrap_or_else(|p| p.into_inner());
            *handoff = Some(updates);
        }
    }
}

/// One clickable row of the folder tree, with an optional quiet badge on the right.
fn tree_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    text: &str,
    badge: Option<&str>,
    selected: bool,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), Sense::click());
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, if selected { theme.active } else { theme.raised });
    }
    let colour = if selected {
        theme.text
    } else {
        theme.text_secondary
    };
    let family = if selected {
        ui_kit::fonts::semibold()
    } else {
        egui::FontFamily::Proportional
    };
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::new(12.5, family),
        colour,
    );
    if let Some(badge) = badge {
        ui.painter().text(
            rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            badge,
            egui::FontId::proportional(10.5),
            theme.faint,
        );
    }
    response
}

fn empty_state(ui: &mut egui::Ui, theme: &Theme, head: &str, body: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(head).family(ui_kit::fonts::semibold()).size(15.0));
        ui.add_space(4.0);
        ui.label(RichText::new(body).color(theme.muted));
    });
}

/// Why a project with an issue can't be opened, and what to do.
fn cannot_open(issue: Issue, path: &Path) -> AppError {
    match issue {
        Issue::Missing => AppError::new("This project can't be found")
            .why(format!("{} was moved, renamed or deleted.", path.display()))
            .fix(
                "Find the file and open it with Open…, or right-click the card and choose Remove from list.",
            ),
        Issue::NewerVersion => AppError::new("This project was saved by a newer version")
            .why("It opens read-only: this version can show it but not change it.")
            .fix("Update the app with the Update button at the bottom left to edit it."),
        Issue::Damaged => AppError::new("This project file is damaged")
            .why(format!("{} could not be read as a project.", path.display()))
            .fix("Open a backup copy if you have one."),
    }
}

fn save_png(image: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    image::save_buffer(path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("The screenshot could not be saved: {e}"))
}

/// How many of the project's media files are not where it says.
fn missing_media(project: &Path) -> usize {
    match project::file::read_summary(project) {
        Ok(Some(summary)) => summary.media.iter().filter(|m| !m.exists()).count(),
        _ => 0,
    }
}

/// What the recovery question says about `unsaved` changes in the project `name`.
fn unsaved_says(name: &str, unsaved: usize) -> String {
    let changes = project::document::changes_text(unsaved);
    if unsaved == 1 {
        format!(
            "{name} has {changes} that wasn't saved before the app closed. It was kept and can be put back."
        )
    } else {
        format!(
            "{name} has {changes} that weren't saved before the app closed. They were kept and can be put back."
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_recovery_question_counts_in_words() {
        assert_eq!(
            super::unsaved_says("big", 1),
            "big has 1 change that wasn't saved before the app closed. It was kept and can be put back."
        );
        assert_eq!(
            super::unsaved_says("big", 4),
            "big has 4 changes that weren't saved before the app closed. They were kept and can be put back."
        );
    }
}
