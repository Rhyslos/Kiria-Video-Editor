//! What the Edit page's drawer lists: the project's media files. Double-click
//! one (or press its +) to place it at the end of the timeline.

use crate::import;
use project::{MediaId, MediaRef, Project};
use ui_kit::Theme;
use ui_kit::drawer::DrawerTab;

/// Where a picked file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    End,
    /// At the edit point, pushing everything after it later.
    Insert,
    /// At the edit point, over whatever is there.
    Overwrite,
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
    /// Draws the drawer's contents. Returns the file to place and where, if
    /// one was picked.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        missing: &[MediaId],
    ) -> Option<(MediaId, Placement)> {
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
                    if let Some(placement) = row(ui, theme, media, missing.contains(&media.id)) {
                        picked = Some((media.id, placement));
                    }
                }
            });
        picked
    }
}

/// One file: name, details, and a + to place it at the end; right-click to
/// insert or overwrite at the edit point. Returns where it was asked to go.
fn row(ui: &mut egui::Ui, theme: &Theme, media: &MediaRef, missing: bool) -> Option<Placement> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 42.0), egui::Sense::click());
    let response = response.on_hover_text(format!(
        "{}\nDouble-click to place it at the end of the timeline; right-click to insert or overwrite at the edit point.",
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
    let mut placed = (response.double_clicked() || add.clicked()).then_some(Placement::End);
    response.context_menu(|ui| {
        for (placement, text) in [
            (Placement::End, "Place at the end"),
            (Placement::Insert, "Insert at the edit point"),
            (Placement::Overwrite, "Overwrite at the edit point"),
        ] {
            if ui.button(text).clicked() {
                placed = Some(placement);
                ui.close();
            }
        }
    });
    placed
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
