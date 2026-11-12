//! The two rows over the Library's files. The tab bar: Video, Audio,
//! Images and Effects, then the project's custom lists and "+ New list"
//! (it scrolls and fades out when there are more than fit; right-click a
//! list to rename or remove it). The filter row: "Find" words, the filters
//! that are on as chips to take off, then group by, order, thumbnail size
//! and the grid / rows switch.

use super::arrange::Facts;
use super::cards::{LARGEST, SMALLEST};
use super::{About, GroupBy, LibraryPage, Picked, Tab, View};
use crate::organise::name_field;
use library::{Sort, Type};
use project::{Command, MediaListId};
use ui_kit::Theme;
use ui_kit::tab_strip::{StripTab, chip, tab_strip};

/// What is being typed in the rows: a new list's name (and the list to
/// open once it exists), and the new name of the list being renamed.
#[derive(Debug, Default)]
pub struct Drafts {
    new_list: String,
    open_when_made: Option<String>,
    renaming: Option<(MediaListId, String)>,
}

pub fn ui(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
) -> Vec<Picked> {
    let mut picked = tabs(page, ui, theme, about, facts);
    ui.add_space(6.0);
    picked.extend(filters(page, ui, theme, about));
    picked
}

fn tabs(
    page: &mut LibraryPage,
    ui: &mut egui::Ui,
    theme: &Theme,
    about: &About<'_>,
    facts: &Facts,
) -> Vec<Picked> {
    let library = &about.project.library;
    // A list just made opens.
    if let Some(name) = &page.top.open_when_made
        && let Some(list) = library.lists.iter().find(|l| l.name == *name)
    {
        page.tab = Tab::List(list.id);
        page.top.open_when_made = None;
    }
    let mut shown: Vec<(Tab, String, Option<usize>)> = Type::ALL
        .iter()
        .map(|t| {
            (
                Tab::Type(*t),
                t.name().to_owned(),
                Some(facts.types.get(t).copied().unwrap_or(0)),
            )
        })
        .collect();
    shown.extend(
        library
            .lists
            .iter()
            .map(|l| (Tab::List(l.id), l.name.clone(), Some(l.media.len()))),
    );
    let strip: Vec<StripTab<'_>> = shown
        .iter()
        .map(|(tab, text, count)| StripTab {
            text,
            count: *count,
            selected: page.tab == *tab,
        })
        .collect();
    let mut picked = Vec::new();
    let drafts = &mut page.top;
    let response = tab_strip(ui, theme, "library-tabs", &strip, |ui| {
        let mut made = None;
        ui.menu_button("+ New list", |ui| {
            ui.label(
                egui::RichText::new("A list of files picked by hand, in any order")
                    .size(12.0)
                    .color(theme.muted),
            );
            if let Some(name) = name_field(ui, &mut drafts.new_list, "List name") {
                made = Some(name);
                drafts.new_list.clear();
                ui.close();
            }
        });
        made
    });
    if let Some(name) = response.trailing {
        page.top.open_when_made = Some(name.clone());
        picked.push(Picked::Edit(Box::new(Command::AddMediaList {
            name,
            media: Vec::new(),
        })));
    }
    for ((tab, _, _), tab_response) in shown.iter().zip(&response.tabs) {
        if tab_response.clicked() {
            page.tab = *tab;
        }
        let Tab::List(list) = *tab else { continue };
        let Some(list) = library.list(list) else { continue };
        let tab_response = tab_response.clone().on_hover_text(format!(
            "Your list \"{}\": files of any type, in the order they were put in. Right-click to rename or remove it.",
            list.name
        ));
        tab_response.context_menu(|ui| {
            if page.top.renaming.as_ref().is_none_or(|(id, _)| *id != list.id) {
                page.top.renaming = Some((list.id, list.name.clone()));
            }
            if let Some((_, name)) = &mut page.top.renaming {
                ui.horizontal(|ui| {
                    let field = ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let changed = !name.trim().is_empty() && name.trim() != list.name;
                    if (ui.add_enabled(changed, egui::Button::new("Rename")).clicked() || entered) && changed
                    {
                        picked.push(Picked::Edit(Box::new(Command::RenameMediaList {
                            list: list.id,
                            name: name.trim().to_owned(),
                        })));
                        ui.close();
                    }
                });
            }
            if ui
                .button("Remove list")
                .on_hover_text("Its files stay in the project. Undo brings the list back.")
                .clicked()
            {
                picked.push(Picked::Edit(Box::new(Command::RemoveMediaList { list: list.id })));
                ui.close();
            }
        });
    }
    picked
}

fn filters(page: &mut LibraryPage, ui: &mut egui::Ui, theme: &Theme, about: &About<'_>) -> Vec<Picked> {
    let library = &about.project.library;
    let in_list = matches!(page.tab, Tab::List(_));
    let sound = page.tab == Tab::Type(Type::Audio);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut page.text)
                .hint_text("Find: name, label or note")
                .desired_width(190.0),
        );
        let mut off = None;
        for label in page.labels.iter().filter_map(|l| library.label(*l)) {
            if chip(
                ui,
                theme,
                &label.name,
                Some(crate::timeline_panel::label_colour(label.color)),
            ) {
                off = Some(label.id);
            }
        }
        if let Some(gone) = off {
            page.labels.retain(|l| *l != gone);
        }
        if let Some(smart) = page.smart
            && chip(ui, theme, smart.name(), None)
        {
            page.smart = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !sound {
                ui.selectable_value(&mut page.view, View::Rows, "Rows")
                    .on_hover_text("Files as rows, with a small picture");
                ui.selectable_value(&mut page.view, View::Grid, "Grid")
                    .on_hover_text("Files as a grid of thumbnails");
            }
            let size_tip = if sound {
                "How tall the rows are"
            } else {
                "Thumbnail size"
            };
            ui.add_sized(
                egui::vec2(90.0, 18.0),
                egui::Slider::new(&mut page.size, SMALLEST..=LARGEST).show_value(false),
            )
            .on_hover_text(size_tip);
            egui::ComboBox::from_id_salt("library-sort")
                .selected_text(format!("Sort: {}", page.sort.name()))
                .show_ui(ui, |ui| {
                    for sort in Sort::ALL {
                        if sort == Sort::ListOrder && !in_list {
                            continue;
                        }
                        ui.selectable_value(&mut page.sort, sort, sort.name());
                    }
                });
            egui::ComboBox::from_id_salt("library-group")
                .selected_text(format!("Group: {}", page.group.name()))
                .show_ui(ui, |ui| {
                    for group in GroupBy::ALL {
                        ui.selectable_value(&mut page.group, group, group.name());
                    }
                });
        });
    });
    if !in_list && page.sort == Sort::ListOrder {
        page.sort = Sort::Name;
    }
    Vec::new()
}
