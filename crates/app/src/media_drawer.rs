//! What the Edit page's drawer lists: the project's media files, found and
//! ordered by the library index (`library::Live`) from the "Find a file"
//! words (matched in names, labels and notes) and the type, label, list and
//! order picked under it. Double-click one (or press its +) to place it at
//! the end of the timeline. Each video file says what its proxy is up to
//! ("Needs proxy", "Proxy 43 %", "Proxy"); right-click to build one now or
//! delete it, and to give a file labels, put it in lists or write its note.

use crate::import;
use crate::organise::{Everywhere, Organised, Organiser};
use crate::proxies::{self, Status};
use library::{Query, Sort, Type};
use project::{Command, GlobalLabels, MediaId, MediaLabelId, MediaListId, MediaRef, Project};
use ui_kit::Theme;
use ui_kit::drawer::DrawerTab;

/// The name the drawer asks the library index under.
pub const ASKER: library::Asker = "media drawer";

/// Where a picked file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    End,
    /// At the edit point, pushing everything after it later.
    Insert,
    /// At the edit point, over what is on the first video track there: as
    /// a file dropped on that track, its sound finds free audio tracks.
    Overwrite,
}

/// What was asked for in the drawer.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    /// A change to the project's labels, lists or notes: one undo step.
    Edit(Box<Command>),
    /// A change to the labels offered in every project.
    Everywhere(Everywhere),
    /// Place the file on the timeline.
    Place(MediaId, Placement),
    /// Build the file's proxy next.
    BuildProxy(MediaId),
    /// Delete the file's proxy.
    DeleteProxy(MediaId),
    /// Add these new files from the media folders to the project's media.
    AddFound(Vec<std::path::PathBuf>),
    /// Hide a new file from the media folders for this session.
    Dismiss(std::path::PathBuf),
    /// A command of the ⋯ menu (Prune, Package project).
    Run(ui_kit::Action),
}

/// What the drawer is told besides the project.
pub struct About<'a> {
    /// The library index's newest answer (`None` until it has one).
    pub found: Option<&'a [MediaId]>,
    /// Files the project has that aren't on disk.
    pub missing: &'a [MediaId],
    pub proxy: ProxyInfo<'a>,
    /// The labels offered in every project.
    pub global: &'a GlobalLabels,
    pub new_files: crate::new_files::NewFiles<'a>,
}

/// What the drawer is told about proxies: each file's, and the mode.
pub struct ProxyInfo<'a> {
    pub status: &'a dyn Fn(MediaId) -> Status,
    pub mode: cache::proxy::ProxyMode,
}

/// The drawer's state, kept by the page.
#[derive(Default)]
pub struct MediaDrawer {
    pub open: bool,
    pub tab: DrawerTab,
    pub filter: String,
    /// Only files of this type.
    pub kind: Option<Type>,
    /// Only files with this label.
    pub label: Option<MediaLabelId>,
    /// Only files in this list.
    pub list: Option<MediaListId>,
    pub sort: Sort,
    /// What is being typed in a file's Labels, Lists and Note menus.
    organiser: Organiser,
}

/// The media in name order, keeping only names containing `filter`
/// (any case): what the drawer shows until the index has answered.
pub fn listed<'a>(project: &'a Project, filter: &str) -> Vec<&'a MediaRef> {
    let filter = filter.to_lowercase();
    let mut list: Vec<&MediaRef> = project
        .media
        .values()
        .filter(|m| name(m).to_lowercase().contains(&filter))
        .collect();
    list.sort_by_key(|m| name(m).to_lowercase());
    list
}

fn name(m: &MediaRef) -> String {
    m.path.file_name().map_or_else(
        || m.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl MediaDrawer {
    /// A drawer, open or not, with these words in "Find a file".
    pub fn new(open: bool, filter: String) -> MediaDrawer {
        MediaDrawer {
            open,
            filter,
            ..MediaDrawer::default()
        }
    }

    /// What the drawer asks the library index for now. A label or list
    /// that no longer exists (undone, removed) asks for nothing of it.
    pub fn query(&self, project: &Project) -> Query {
        Query {
            text: self.filter.clone(),
            types: self.kind.into_iter().collect(),
            labels: self
                .label
                .filter(|l| project.library.label(*l).is_some())
                .into_iter()
                .collect(),
            list: self.list.filter(|l| project.library.list(*l).is_some()),
            usage: library::Usage::Any,
            sort: self.sort,
        }
    }

    /// The name dialog of a new label or list asked for in a file's menu:
    /// call every frame, whether the drawer is shown or not (it may have
    /// closed as the menu did).
    pub fn dialog(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        project: &Project,
        global: &GlobalLabels,
    ) -> Option<Picked> {
        self.organiser
            .dialog(ctx, theme, project, global)
            .map(|asked| match asked {
                Organised::Edit(command) => Picked::Edit(command),
                Organised::Everywhere(change) => Picked::Everywhere(change),
            })
    }

    /// Whether anything besides the words narrows the list.
    fn narrowed(&self) -> bool {
        self.kind.is_some() || self.label.is_some() || self.list.is_some()
    }

    /// Draws the drawer's contents. Returns what was asked for, if anything.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        about: &About<'_>,
    ) -> Option<Picked> {
        let About {
            found,
            missing,
            proxy,
            global,
            new_files,
        } = about;
        ui_kit::drawer::tab_switch(ui, &mut self.tab);
        ui.add_space(6.0);
        if self.tab == DrawerTab::Effects {
            ui.label(egui::RichText::new("Effects arrive with the inspector.").color(theme.muted));
            return None;
        }
        let mut ran = None;
        ui.horizontal(|ui| {
            ran = more_menu(ui);
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Find a file: name, label or note")
                    .desired_width(f32::INFINITY),
            );
        });
        if ran.is_some() {
            return ran;
        }
        ui.add_space(4.0);
        self.filters(ui, project);
        ui.add_space(6.0);
        let mut new_picked = None;
        if !new_files.files.is_empty() {
            new_picked = crate::new_files::ui(ui, theme, new_files).map(|asked| match asked {
                crate::new_files::Asked::Add(paths) => Picked::AddFound(paths),
                crate::new_files::Asked::Dismiss(path) => Picked::Dismiss(path),
            });
            ui.add_space(4.0);
            ui.separator();
        }
        let list: Vec<&MediaRef> = match *found {
            Some(ids) => ids.iter().filter_map(|id| project.media.get(id)).collect(),
            None => listed(project, &self.filter),
        };
        if list.is_empty() {
            let text = if project.media.is_empty() {
                "Imported files show here."
            } else if self.narrowed() {
                "No file matches these words and choices."
            } else {
                "No file has that in its name, labels or note."
            };
            ui.label(egui::RichText::new(text).color(theme.muted));
            return new_picked;
        }
        let mut picked = new_picked;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, list.len(), |ui, range| {
                for media in &list[range] {
                    let status = (proxy.status)(media.id);
                    let about = Row {
                        media,
                        missing: missing.contains(&media.id),
                        proxy: &status,
                        mode: proxy.mode,
                        global,
                    };
                    if let Some(asked) = self.row(ui, theme, project, &about) {
                        picked = Some(asked);
                    }
                }
            });
        picked
    }

    /// The type and order, and the label and list when the project has any.
    fn filters(&mut self, ui: &mut egui::Ui, project: &Project) {
        let library = &project.library;
        let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("drawer-type")
                .width(width - 8.0)
                .selected_text(self.kind.map_or("All types", Type::name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind, None, "All types");
                    for t in [Type::Video, Type::Audio, Type::Image] {
                        ui.selectable_value(&mut self.kind, Some(t), t.name());
                    }
                });
            egui::ComboBox::from_id_salt("drawer-sort")
                .width(width - 8.0)
                .selected_text(format!("Sort: {}", self.sort.name()))
                .show_ui(ui, |ui| {
                    for sort in Sort::ALL {
                        if sort == Sort::ListOrder && self.list.is_none() {
                            continue;
                        }
                        ui.selectable_value(&mut self.sort, sort, sort.name());
                    }
                });
        });
        if library.labels.is_empty() && library.lists.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            let label_name = self.label.and_then(|l| library.label(l)).map(|l| l.name.as_str());
            egui::ComboBox::from_id_salt("drawer-label")
                .width(width - 8.0)
                .selected_text(label_name.unwrap_or("Any label"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.label, None, "Any label");
                    for l in &library.labels {
                        ui.selectable_value(&mut self.label, Some(l.id), &l.name);
                    }
                });
            let list_name = self.list.and_then(|l| library.list(l)).map(|l| l.name.as_str());
            egui::ComboBox::from_id_salt("drawer-list")
                .width(width - 8.0)
                .selected_text(list_name.unwrap_or("Any list"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.list, None, "Any list");
                    for l in &library.lists {
                        ui.selectable_value(&mut self.list, Some(l.id), &l.name);
                    }
                });
        });
        if self.list.is_none() && self.sort == Sort::ListOrder {
            self.sort = Sort::Name;
        }
    }
}

/// The ⋯ menu beside "Find a file": what works on the project's media as
/// a whole.
fn more_menu(ui: &mut egui::Ui) -> Option<Picked> {
    let mut ran = None;
    ui.menu_button("⋯", |ui| {
        if ui
            .button("Prune unused media…")
            .on_hover_text(
                "Lists the files no timeline uses, to take out of the project. Nothing is deleted from disk.",
            )
            .clicked()
        {
            ran = Some(Picked::Run(ui_kit::Action::Prune));
            ui.close();
        }
        if ui
            .button("Package project…")
            .on_hover_text("Copies the project and only the media its timelines use into one folder, to open on another machine.")
            .clicked()
        {
            ran = Some(Picked::Run(ui_kit::Action::PackageProject));
            ui.close();
        }
    })
    .response
    .on_hover_text("More for the project's media");
    ran
}

/// Every row is this tall, so only the rows in view are drawn (10 000
/// files scroll as smoothly as ten).
const ROW_HEIGHT: f32 = 58.0;

/// What one row shows.
struct Row<'a> {
    media: &'a MediaRef,
    missing: bool,
    proxy: &'a Status,
    mode: cache::proxy::ProxyMode,
    global: &'a GlobalLabels,
}

impl MediaDrawer {
    /// One file: name, details, its labels, what its proxy is up to, and a +
    /// to place it at the end; right-click to insert or overwrite at the edit
    /// point, to build or delete its proxy, or for its labels, lists and note.
    /// Returns what was asked for.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        about: &Row<'_>,
    ) -> Option<Picked> {
        let Row {
            media,
            missing,
            proxy,
            mode,
            global,
        } = *about;
        let width = ui.available_width();
        // What its proxy is up to and its labels, on a line under the details.
        let proxy_line = proxies::label(proxy).filter(|_| !missing);
        let labels = project.library.labels_of(media.id);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT - 2.0), egui::Sense::click_and_drag());
        if response.drag_started() {
            response.dnd_set_drag_payload(ui_kit::DraggedMedia(media.id.0));
        }
        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let about_proxy =
            proxies::explanation(proxy, mode).map_or_else(String::new, |text| format!("\n\n{text}"));
        let note = project.library.note(media.id);
        let about_note = if note.is_empty() {
            String::new()
        } else {
            format!("\n\nNote: {note}")
        };
        let about_end = media::end_missing_words(media).map_or_else(String::new, |words| {
            format!("\n\n{words}: it was imported as far as it goes.")
        });
        let response = response.on_hover_text(format!(
        "{}\nDouble-click to place it at the end of the timeline; right-click to insert or overwrite at the edit point, or for labels, lists and a note; or drag it onto the timeline.{about_end}{about_proxy}{about_note}",
        media.path.display()
    ));
        let painter = ui.painter_at(rect);
        if response.hovered() {
            painter.rect_filled(rect, 6.0, theme.line);
        }
        let details = import::summary(media);
        let details = details.split_once(" · ").map_or("", |(_, rest)| rest);
        // A coloured edge, shared between its labels, as the library shows them.
        if !labels.is_empty() {
            let part = (rect.height() - 8.0) / labels.len() as f32;
            for (i, label) in labels.iter().enumerate() {
                let top = rect.top() + 4.0 + i as f32 * part;
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(3.0, part)),
                    1.0,
                    crate::timeline_panel::label_colour(label.color),
                );
            }
        }
        let mut x = rect.left() + 8.0;
        if missing {
            painter.circle_filled(egui::pos2(x + 3.0, rect.top() + 13.0), 3.5, ui_kit::status::ERROR);
            x += 12.0;
        }
        let mut line = egui::text::LayoutJob::default();
        let small = egui::FontId::proportional(11.0);
        if media.info.stated_length.is_some() && !missing {
            line.append(
                "End missing",
                0.0,
                egui::TextFormat::simple(small.clone(), ui_kit::status::WARNING),
            );
        }
        if let Some(text) = proxy_line {
            let colour = proxies::colour(proxy, theme);
            let gap = if line.text.is_empty() { 0.0 } else { 8.0 };
            line.append(&text, gap, egui::TextFormat::simple(small.clone(), colour));
        }
        for (i, label) in labels.iter().enumerate() {
            let gap = if i == 0 && line.text.is_empty() { 0.0 } else { 8.0 };
            line.append(
                &format!("● {}", label.name),
                gap,
                egui::TextFormat::simple(small.clone(), crate::timeline_panel::label_colour(label.color)),
            );
        }
        if !note.is_empty() {
            let gap = if line.text.is_empty() { 0.0 } else { 8.0 };
            line.append("Note", gap, egui::TextFormat::simple(small, theme.muted));
        }
        if !line.text.is_empty() {
            line.wrap = egui::text::TextWrapping::truncate_at_width(width - 44.0);
            let galley = painter.layout_job(line);
            painter.galley(
                egui::pos2(rect.left() + 8.0, rect.top() + 39.0),
                galley,
                theme.muted,
            );
        }
        painter.text(
            egui::pos2(x, rect.top() + 6.0),
            egui::Align2::LEFT_TOP,
            name(media),
            egui::FontId::new(12.5, ui_kit::fonts::semibold()),
            if missing { theme.muted } else { theme.text },
        );
        let mut job = egui::text::LayoutJob::simple_singleline(
            (if missing { "can't be found" } else { details }).to_owned(),
            egui::FontId::proportional(11.0),
            theme.muted,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 44.0);
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(rect.left() + 8.0, rect.top() + 24.0),
            galley,
            theme.muted,
        );
        let plus = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            egui::vec2(22.0, 22.0),
        );
        let add = ui
            .interact(plus, response.id.with("place"), egui::Sense::click())
            .on_hover_text("Place at the end of the timeline");
        if response.hovered() || add.hovered() {
            painter.rect_filled(plus, 5.0, if add.hovered() { theme.accent } else { theme.panel });
            painter.text(
                plus.center(),
                egui::Align2::CENTER_CENTER,
                "+",
                egui::FontId::proportional(15.0),
                if add.hovered() {
                    theme.accent_ink
                } else {
                    theme.text
                },
            );
        }
        let mut picked =
            (response.double_clicked() || add.clicked()).then_some(Picked::Place(media.id, Placement::End));
        response.context_menu(|ui| {
            for (placement, text) in [
                (Placement::End, "Place at the end"),
                (Placement::Insert, "Insert at the edit point"),
                (Placement::Overwrite, "Overwrite at the edit point"),
            ] {
                if ui.button(text).clicked() {
                    picked = Some(Picked::Place(media.id, placement));
                    ui.close();
                }
            }
            if let Some(asked) = proxy_menu(ui, media.id, proxy, missing) {
                picked = Some(asked);
                ui.close();
            }
            ui.separator();
            if let Some(asked) = self.organiser.menus(ui, project, global, &[media.id]) {
                picked = Some(match asked {
                    Organised::Edit(command) => Picked::Edit(command),
                    Organised::Everywhere(change) => Picked::Everywhere(change),
                });
                ui.close();
            }
        });
        picked
    }
}

/// The proxy's lines in a video file's right-click menu.
pub(crate) fn proxy_menu(ui: &mut egui::Ui, media: MediaId, proxy: &Status, missing: bool) -> Option<Picked> {
    match proxy {
        Status::Nothing if !missing => None,
        Status::Ready => {
            ui.separator();
            ui.button("Delete proxy")
                .on_hover_text("It is not built again by itself; build it again from here.")
                .clicked()
                .then_some(Picked::DeleteProxy(media))
        }
        Status::NotNeeded | Status::Needed | Status::Cleared | Status::Failed(_) if !missing => {
            ui.separator();
            ui.button("Build proxy now")
                .on_hover_text(
                    "A small copy that is quick to jump around in, built in the background before any other.",
                )
                .clicked()
                .then_some(Picked::BuildProxy(media))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaInfo, MediaKind, Time};

    fn media(id: u64, path: &str) -> MediaRef {
        MediaRef {
            id: MediaId(id),
            path: path.into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: Time::seconds(1),
            info: MediaInfo::default(),
        }
    }

    #[test]
    fn lists_by_name_and_filters() {
        let mut p = Project::new_automatic("T");
        for (id, path) in [(1, "/r/Zebra.mkv"), (2, "/r/apple.wav"), (3, "/r/Mid.mp4")] {
            p.media.insert(MediaId(id), media(id, path));
        }
        let names: Vec<u64> = listed(&p, "").iter().map(|m| m.id.0).collect();
        assert_eq!(names, [2, 3, 1]);
        let found: Vec<u64> = listed(&p, "MKV").iter().map(|m| m.id.0).collect();
        assert_eq!(found, [1]);
    }
}
