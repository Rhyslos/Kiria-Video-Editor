//! The Saved versions window: save the project as it is under a name, and
//! go back to any version saved before (an undoable change).

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::Project;
use project::versions::{self, Version};
use std::path::Path;
use ui_kit::Theme;

#[derive(Default)]
pub struct VersionsWindow {
    pub open: bool,
    name: String,
    /// The list, read when the window opens and after each save.
    listed: Option<Vec<Version>>,
    /// A version being read to restore it.
    reading: Option<(String, JobHandle<Project>)>,
    saving: Option<JobHandle<Version>>,
}

/// What the window asked for this frame.
pub enum Asked {
    Nothing,
    Saved(String),
    /// Restore this project (read from the version named).
    Restore(String, Box<Project>),
    Failed(diag::AppError),
}

impl VersionsWindow {
    pub fn show_again(&mut self) {
        self.listed = None;
    }

    /// The window. `project` is the project as it is now (saved as a
    /// version on a worker when asked); `undo` is Undo's shortcut, for a hint.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        jobs: &JobSystem,
        project: &Project,
        project_file: &Path,
        undo: &str,
    ) -> Asked {
        let mut asked = Asked::Nothing;
        if let Some(handle) = &self.saving
            && let Some(result) = handle.try_result()
        {
            asked = match result {
                Ok(v) => {
                    self.listed = None;
                    Asked::Saved(v.name)
                }
                Err(err) => Asked::Failed(err),
            };
            self.saving = None;
        }
        if let Some((name, handle)) = &self.reading
            && let Some(result) = handle.try_result()
        {
            asked = match result {
                Ok(p) => Asked::Restore(name.clone(), Box::new(p)),
                Err(err) => Asked::Failed(err),
            };
            self.reading = None;
        }
        let listed = self.listed.get_or_insert_with(|| versions::list(project_file));
        let mut open = self.open;
        let mut restore = None;
        egui::Window::new("Saved versions")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text("Name, like \"Before the colour pass\""),
                    );
                    if self.saving.is_none()
                        && ui_kit::accent_button(ui, theme, "Save this version").clicked()
                    {
                        let (p, file, name) =
                            (project.clone(), project_file.to_path_buf(), self.name.clone());
                        self.saving = Some(jobs.submit_write(
                            PoolKind::Export,
                            Priority::Interactive,
                            "save version",
                            move |_| versions::save(&p, &file, &name),
                        ));
                    }
                });
                ui.add_space(8.0);
                if listed.is_empty() {
                    ui.label(egui::RichText::new("No versions yet.").color(theme.muted));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for v in listed.iter() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&v.name).family(ui_kit::fonts::semibold()));
                            ui.label(
                                egui::RichText::new(ui_kit::time_text::ago(
                                    now.saturating_sub(v.saved).max(0) as u64,
                                ))
                                .color(theme.muted),
                            );
                            if self.reading.is_none()
                                && ui
                                    .button("Restore")
                                    .on_hover_text(format!("Go back to this version. {undo} returns to now."))
                                    .clicked()
                            {
                                restore = Some(v.clone());
                            }
                        });
                    }
                });
                if self.reading.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the version…");
                    });
                }
            });
        if let Some(v) = restore {
            let name = v.name.clone();
            let handle = jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "read version",
                move |_| versions::load(&v),
            );
            self.reading = Some((name, handle));
        }
        if matches!(asked, Asked::Saved(_)) {
            self.name.clear();
        }
        self.open = open;
        asked
    }
}
