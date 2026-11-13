//! The Library's sidebar: Import (into the project's media only), "New in
//! your folders" (`crate::new_files`, as in the media drawer), the labels
//! (with how many files of the
//! open tab have each; a click narrows the files to those with it, as a
//! chip in the filter row; right-click to rename, recolour, offer in every
//! project or remove), labels offered in every project that this one hasn't
//! got yet, the smart filters, and Prune.

use super::arrange::Facts;
use super::{About, LibraryPage, Picked, Smart};
use crate::organise::Everywhere;
use project::{Command, LabelColor, MediaLabel, MediaLabelId};
use ui_kit::{Action, Theme};

/// What is being typed in the sidebar: the new name of the label being renamed.
#[derive(Debug, Default)]
pub struct Drafts {
    renaming: Option<(MediaLabelId, String)>,
}

/// What Prune does, for its tooltip (the media drawer's ⋯ menu says the same).
pub const PRUNE_TIP: &str =
    "Lists the files no timeline uses, to take out of the project. Nothing is deleted from disk.";

pub fn ui(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let width = ui.available_width();
    let import = ui
        .add_sized(
            egui::vec2(width, 30.0),
            egui::Button::new(egui::RichText::new("Import media…").color(theme.accent_ink))
                .fill(theme.accent),
        )
        .on_hover_text(format!(
            "{}: the files go into the project's media, ready to add to a timeline from here",
            about.shortcuts.tip(Action::Import)
        ));
    if import.clicked() {
        picked.push(Picked::Import);
    }
    if about.importing > 0 {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new(format!(
                    "Checking {} file{}…",
                    about.importing,
                    if about.importing == 1 { "" } else { "s" }
                ))
                .size(12.0)
                .color(theme.text_secondary),
            );
        });
    }
    if !about.new_files.files.is_empty() {
        ui.add_space(8.0);
        if let Some(asked) = crate::new_files::ui(ui, theme, &about.new_files) {
            picked.push(match asked {
                crate::new_files::Asked::Add(paths) => Picked::AddFound(paths),
                crate::new_files::Asked::Dismiss(path) => Picked::Dismiss(path),
            });
        }
    }
    ui.add_space(10.0);
    let prune_room = 40.0;
    egui::ScrollArea::vertical()
        .id_salt("library-sidebar")
        .auto_shrink([false, true])
        .max_height((ui.available_height() - prune_room).max(60.0))
        .show(ui, |ui| {
            picked.extend(labels(page, ui, theme, about, facts));
            ui.add_space(14.0);
            heading(ui, theme, "Smart filters");
            for smart in Smart::ALL {
                let on = page.smart == Some(smart);
                let count = facts.smart.get(&smart).copied().unwrap_or(0);
                let row = filter_row(ui, theme, on, None, smart.name(), None, Some(count), false)
                    .on_hover_text(format!("{}. Click again to show every file.", smart.tip()));
                if row.clicked() {
                    page.smart = if on { None } else { Some(smart) };
                }
            }
        });
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        let prune = ui
            .add(egui::Button::new("Prune unused media…").min_size(egui::vec2(width, 28.0)))
            .on_hover_text(PRUNE_TIP);
        if prune.clicked() {
            picked.push(Picked::Prune);
        }
    });
    picked
}

/// The labels: the project's, then those offered everywhere it hasn't got.
fn labels(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let library = &about.project.library;
    ui.horizontal(|ui| {
        heading(ui, theme, "Labels");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("+")
                .on_hover_text("Make a label, on no file yet")
                .clicked()
            {
                page.organiser.ask_label(Vec::new());
            }
        });
    });
    let others: Vec<&project::GlobalLabel> = about
        .global
        .labels
        .iter()
        .filter(|g| {
            !library
                .labels
                .iter()
                .any(|l| l.name.to_lowercase() == g.name.to_lowercase())
        })
        .collect();
    if library.labels.is_empty() && others.is_empty() {
        ui.label(
            egui::RichText::new("Labels you make, or give files from their right-click menu, show here.")
                .size(12.0)
                .color(theme.muted),
        );
    }
    for label in &library.labels {
        let on = page.labels.contains(&label.id);
        let everywhere = about.global.find(&label.name).is_some();
        let count = facts.labels.get(&label.id).copied().unwrap_or(0);
        let colour = crate::timeline_panel::label_colour(label.color);
        let tag = everywhere.then_some("everywhere");
        let row = filter_row(ui, theme, on, Some(colour), &label.name, tag, Some(count), false).on_hover_text(
            "Click to show only files with this label (with others picked: files with all of them). Right-click to rename, recolour or remove it.",
        );
        if row.clicked() {
            if on {
                page.labels.retain(|l| *l != label.id);
            } else {
                page.labels.push(label.id);
            }
        }
        row.context_menu(|ui| {
            if let Some(asked) = label_menu(&mut page.sidebar, ui, theme, label, everywhere) {
                picked.push(asked);
                ui.close();
            }
        });
    }
    for global in others {
        let colour = crate::timeline_panel::label_colour(global.color);
        let row = filter_row(ui, theme, false, Some(colour), &global.name, Some("everywhere"), None, true)
            .on_hover_text("Offered in every project. Put it on a file (its right-click menu) or right-click here to add it to this project.");
        row.context_menu(|ui| {
            if ui.button("Add to this project").clicked() {
                let label = MediaLabel {
                    id: MediaLabelId(0),
                    name: global.name.clone(),
                    color: global.color,
                };
                picked.push(Picked::Edit(Box::new(Command::AddMediaLabel {
                    label,
                    media: Vec::new(),
                })));
                ui.close();
            }
            if ui
                .button("Stop offering it everywhere")
                .on_hover_text("Projects that use it keep it as their own label")
                .clicked()
            {
                picked.push(Picked::Everywhere(Everywhere::StopOffering(global.name.clone())));
                ui.close();
            }
        });
    }
    picked
}

/// A label's right-click menu: rename, colour, every project, remove.
fn label_menu(
    drafts: &mut Drafts,
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &MediaLabel,
    everywhere: bool,
) -> Option<Picked> {
    let mut asked = None;
    if drafts.renaming.as_ref().is_none_or(|(id, _)| *id != label.id) {
        drafts.renaming = Some((label.id, label.name.clone()));
    }
    if let Some((_, name)) = &mut drafts.renaming {
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(name).desired_width(130.0));
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let changed = !name.trim().is_empty() && name.trim() != label.name;
            if (ui.add_enabled(changed, egui::Button::new("Rename")).clicked() || entered) && changed {
                asked = Some(edit(label, name.trim().to_owned(), label.color));
            }
        });
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for color in LabelColor::ALL {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            ui.painter()
                .circle_filled(rect.center(), 7.0, crate::timeline_panel::label_colour(color));
            if color == label.color {
                ui.painter()
                    .circle_stroke(rect.center(), 8.5, egui::Stroke::new(1.5, theme.text));
            }
            if response.clicked() && color != label.color {
                asked = Some(edit(label, label.name.clone(), color));
            }
        }
    });
    ui.separator();
    let offer = if everywhere {
        ui.button("Stop offering it everywhere")
            .on_hover_text("Projects that use it keep it as their own label")
            .clicked()
            .then(|| Everywhere::StopOffering(label.name.clone()))
    } else {
        ui.button("Use it in every project")
            .on_hover_text("Offered in every project's Labels menu, with this colour")
            .clicked()
            .then(|| Everywhere::Offer(label.name.clone(), label.color))
    };
    if let Some(change) = offer {
        asked = Some(Picked::Everywhere(change));
    }
    if ui
        .button("Remove label")
        .on_hover_text("Takes it off every file and out of the project. Undo brings it back.")
        .clicked()
    {
        asked = Some(Picked::Edit(Box::new(Command::RemoveMediaLabel {
            label: label.id,
        })));
    }
    if asked.is_some() {
        drafts.renaming = None;
    }
    asked
}

fn edit(label: &MediaLabel, name: String, color: LabelColor) -> Picked {
    Picked::Edit(Box::new(Command::EditMediaLabel {
        label: label.id,
        name,
        color,
    }))
}

fn heading(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .family(ui_kit::fonts::semibold())
            .size(12.0)
            .color(theme.text_secondary),
    );
}

/// One line of the sidebar: a dot in the label's colour, the name, a quiet
/// tag ("everywhere"), the count on the right; lit while it is on.
#[allow(clippy::too_many_arguments)]
fn filter_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    on: bool,
    dot: Option<egui::Color32>,
    name: &str,
    tag: Option<&str>,
    count: Option<usize>,
    dim: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if on {
        painter.rect_filled(rect, 5.0, theme.active);
    } else if response.hovered() {
        painter.rect_filled(rect, 5.0, theme.line);
    }
    let mut x = rect.left() + 8.0;
    let y = rect.center().y;
    if let Some(colour) = dot {
        let colour = if dim { colour.gamma_multiply(0.5) } else { colour };
        painter.circle_filled(egui::pos2(x + 4.0, y), 4.5, colour);
        x += 16.0;
    }
    let ink = if dim {
        theme.muted
    } else if on {
        theme.text
    } else {
        theme.text_secondary
    };
    let count_room = 34.0;
    let mut job = egui::text::LayoutJob::default();
    job.append(
        name,
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(13.0), ink),
    );
    if let Some(tag) = tag {
        job.append(
            tag,
            6.0,
            egui::TextFormat::simple(egui::FontId::proportional(10.5), theme.muted),
        );
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - count_room - x).max(10.0));
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(x, y - galley.size().y / 2.0), galley, ink);
    if let Some(count) = count {
        painter.text(
            egui::pos2(rect.right() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(12.0),
            theme.muted,
        );
    }
    response
}
