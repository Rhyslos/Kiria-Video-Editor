//! What the Edit page's drawer lists: the project's media files. Double-click
//! one (or press its +) to place it at the end of the timeline. Each video
//! file says what its proxy is up to ("Needs proxy", "Proxy 43 %",
//! "Proxy"); right-click to build one now or delete it.

use crate::import;
use crate::proxies::{self, Status};
use project::{MediaId, MediaRef, Project};
use ui_kit::Theme;
use ui_kit::drawer::DrawerTab;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picked {
    /// Place the file on the timeline.
    Place(MediaId, Placement),
    /// Build the file's proxy next.
    BuildProxy(MediaId),
    /// Delete the file's proxy.
    DeleteProxy(MediaId),
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
}

/// The media in name order, keeping only names containing `filter`
/// (any case).
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
    /// Draws the drawer's contents. Returns what was asked for, if anything.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        missing: &[MediaId],
        proxy: &ProxyInfo<'_>,
    ) -> Option<Picked> {
        ui_kit::drawer::tab_switch(ui, &mut self.tab);
        ui.add_space(6.0);
        if self.tab == DrawerTab::Effects {
            ui.label(egui::RichText::new("Effects arrive with the inspector.").color(theme.muted));
            return None;
        }
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Find a file"));
        ui.add_space(6.0);
        let list = listed(project, &self.filter);
        if list.is_empty() {
            let text = if project.media.is_empty() {
                "Imported files show here."
            } else {
                "No file has that in its name."
            };
            ui.label(egui::RichText::new(text).color(theme.muted));
            return None;
        }
        let mut picked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for media in list {
                    let status = (proxy.status)(media.id);
                    if let Some(asked) =
                        row(ui, theme, media, missing.contains(&media.id), &status, proxy.mode)
                    {
                        picked = Some(asked);
                    }
                }
            });
        picked
    }
}

/// One file: name, details, what its proxy is up to, and a + to place it
/// at the end; right-click to insert or overwrite at the edit point, or to
/// build or delete its proxy. Returns what was asked for.
fn row(
    ui: &mut egui::Ui,
    theme: &Theme,
    media: &MediaRef,
    missing: bool,
    proxy: &Status,
    mode: cache::proxy::ProxyMode,
) -> Option<Picked> {
    let width = ui.available_width();
    // What its proxy is up to, on a line of its own under the details.
    let proxy_line = proxies::label(proxy).filter(|_| !missing);
    let height = if proxy_line.is_some() { 56.0 } else { 42.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    if response.drag_started() {
        response.dnd_set_drag_payload(ui_kit::DraggedMedia(media.id.0));
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let about_proxy =
        proxies::explanation(proxy, mode).map_or_else(String::new, |text| format!("\n\n{text}"));
    let response = response.on_hover_text(format!(
        "{}\nDouble-click to place it at the end of the timeline; right-click to insert or overwrite at the edit point; or drag it onto the timeline.{about_proxy}",
        media.path.display()
    ));
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 6.0, theme.line);
    }
    let details = import::summary(media);
    let details = details.split_once(" · ").map_or("", |(_, rest)| rest);
    let mut x = rect.left() + 8.0;
    if missing {
        painter.circle_filled(egui::pos2(x + 3.0, rect.top() + 13.0), 3.5, ui_kit::status::ERROR);
        x += 12.0;
    }
    if let Some(text) = proxy_line {
        let colour = match proxy {
            Status::Failed(_) => ui_kit::status::ERROR,
            Status::Ready => theme.accent,
            _ => theme.muted,
        };
        let mut line =
            egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(11.0), colour);
        line.wrap = egui::text::TextWrapping::truncate_at_width(width - 44.0);
        let galley = painter.layout_job(line);
        painter.galley(egui::pos2(rect.left() + 8.0, rect.top() + 39.0), galley, colour);
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
    });
    picked
}

/// The proxy's lines in a video file's right-click menu.
fn proxy_menu(ui: &mut egui::Ui, media: MediaId, proxy: &Status, missing: bool) -> Option<Picked> {
    match proxy {
        Status::Nothing if !missing => None,
        Status::Ready => {
            ui.separator();
            ui.button("Delete proxy")
                .on_hover_text("It is not built again by itself this session; build it again from here.")
                .clicked()
                .then_some(Picked::DeleteProxy(media))
        }
        Status::NotNeeded | Status::Needed | Status::Failed(_) if !missing => {
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
