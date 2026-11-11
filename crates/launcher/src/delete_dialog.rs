//! "Delete project…": finds the project's files on a worker, asks with the
//! list, then moves them to the recycle bin on a worker. The file work
//! itself is in `delete.rs`.

use crate::delete::{self, Bin, Plan};
use crate::work::{Poll, Task};
use diag::AppError;
use egui::RichText;
use std::path::{Path, PathBuf};
use ui_kit::Theme;

/// Rows of the file list that show without scrolling.
const ROWS_SHOWN: usize = 9;

/// How a delete ended.
#[derive(Debug)]
pub enum Event {
    /// The project's files are in the bin: `moved` files and folders (none
    /// when nothing of it was left on disk).
    Deleted {
        project: PathBuf,
        name: String,
        moved: usize,
    },
    /// Nothing, or not everything, was deleted.
    Failed(AppError),
}

#[derive(Debug)]
enum Step {
    /// Finding the files that would go.
    Looking(Task<diag::Result<Plan>>),
    /// Asking, with the list.
    Asking(Plan),
    /// Moving them to the bin.
    Moving(Task<(PathBuf, diag::Result<usize>)>),
}

/// The delete of one project, from the menu entry to the bin.
#[derive(Debug, Default)]
pub struct DeleteDialog {
    /// The project's name, for the question and the status line.
    name: String,
    step: Option<Step>,
}

impl DeleteDialog {
    /// Starts looking for the files of `project`. Nothing is deleted before
    /// the person has seen the list and agreed. Returns `false` while an
    /// earlier delete is still under way.
    pub fn begin(&mut self, ctx: &egui::Context, data_dir: &Path, project: PathBuf, name: String) -> bool {
        if self.step.is_some() {
            return false;
        }
        self.name = name;
        let data_dir = data_dir.to_path_buf();
        self.step = Some(Step::Looking(Task::spawn(ctx, move || {
            crate::recovery::refuse_if_open_in_editor(&data_dir, &project, "deleted")?;
            delete::plan(&project)
        })));
        true
    }

    /// The question is on screen.
    pub fn is_asking(&self) -> bool {
        matches!(self.step, Some(Step::Asking(_)))
    }

    /// Files are being looked for or moved.
    pub fn is_working(&self) -> bool {
        matches!(self.step, Some(Step::Looking(_) | Step::Moving(_)))
    }

    /// What to say in the status line while files are being moved.
    pub fn status(&self) -> Option<String> {
        matches!(self.step, Some(Step::Moving(_))).then(|| format!("Moving to the {}…", Bin::NAME))
    }

    /// Call every frame: picks up finished work and shows the question.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, bin: &Bin) -> Option<Event> {
        match self.step.take()? {
            Step::Looking(task) => match task.poll() {
                Poll::Running => self.step = Some(Step::Looking(task)),
                Poll::Done(Ok(plan)) => self.step = Some(Step::Asking(plan)),
                Poll::Done(Err(err)) => return Some(Event::Failed(err)),
                Poll::Lost => {}
            },
            Step::Asking(plan) => match question(ctx, theme, &self.name, &plan) {
                None => self.step = Some(Step::Asking(plan)),
                Some(false) => {}
                Some(true) => {
                    let bin = bin.clone();
                    self.step = Some(Step::Moving(Task::spawn(ctx, move || {
                        let result = delete::run(&plan, &bin);
                        (plan.project().to_path_buf(), result)
                    })));
                }
            },
            Step::Moving(task) => match task.poll() {
                Poll::Running => self.step = Some(Step::Moving(task)),
                Poll::Done((project, Ok(moved))) => {
                    return Some(Event::Deleted {
                        project,
                        name: std::mem::take(&mut self.name),
                        moved,
                    });
                }
                Poll::Done((_, Err(err))) => return Some(Event::Failed(err)),
                Poll::Lost => return Some(Event::Failed(stopped_part_way())),
            },
        }
        None
    }
}

/// The worker moving the files stopped without an answer.
fn stopped_part_way() -> AppError {
    AppError::new("Deleting the project stopped part-way")
        .why(format!(
            "The work ended unexpectedly, so some of its files may be in the {} and others not.",
            Bin::NAME
        ))
        .fix("Delete the project again to move the rest; a file that is already gone is fine. The log has the details.")
}

/// Shows the confirmation. `Some(true)` to delete, `Some(false)` to leave
/// everything alone, `None` while it is open.
fn question(ctx: &egui::Context, theme: &Theme, name: &str, plan: &Plan) -> Option<bool> {
    let mut answer = None;
    let nothing_left = plan.items().is_empty();
    let modal = ui_kit::dialog(ctx, theme, "delete-project", |ui| {
        ui.set_width(460.0);
        ui.label(
            RichText::new(format!("Delete {name}?"))
                .family(ui_kit::fonts::semibold())
                .size(16.0),
        );
        ui.add_space(6.0);
        let explanation = if nothing_left {
            "None of this project's files are on disk any more. It will be taken off the list.".to_owned()
        } else {
            format!(
                "These files go to the {}, so you can get them back from there. Recordings used in the project stay where they are.",
                Bin::NAME
            )
        };
        ui.label(RichText::new(explanation).color(theme.text_secondary));
        if !nothing_left {
            ui.add_space(10.0);
            file_list(ui, theme, plan);
        }
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let confirm = if nothing_left {
                "Remove from list".to_owned()
            } else {
                format!("Move to {}", Bin::NAME)
            };
            if ui_kit::accent_button(ui, theme, &confirm).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }
    answer
}

/// The folder once, then one row per file: what it is and its name.
fn file_list(ui: &mut egui::Ui, theme: &Theme, plan: &Plan) {
    if let Some(folder) = plan.project().parent() {
        ui.label(
            RichText::new(folder.display().to_string())
                .font(ui_kit::fonts::mono(11.0))
                .color(theme.faint),
        );
        ui.add_space(4.0);
    }
    let row_height = 20.0;
    egui::Frame::new()
        .fill(theme.panel)
        .stroke(egui::Stroke::new(1.0, theme.line))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(row_height * ROWS_SHOWN as f32)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for item in plan.items() {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), row_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let what = match &item.detail {
                                    Some(detail) => format!("{} ({detail})", item.what),
                                    None => item.what.to_owned(),
                                };
                                ui.label(RichText::new(what).size(12.5));
                                let name = item
                                    .path
                                    .file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(name)
                                                .font(ui_kit::fonts::mono(11.0))
                                                .color(theme.muted),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(item.path.display().to_string());
                                });
                            },
                        );
                    }
                });
        });
}
