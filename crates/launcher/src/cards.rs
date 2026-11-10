//! The project grid: three cards per row, 16:9 thumbnails, scrolling down.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use project_index::{Folder, FolderKind, Issue, ProjectEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ui_kit::{Theme, status, time_text};

/// Cards per row.
pub const COLUMNS: usize = 3;
const GAP: f32 = 16.0;
/// Height of the text under a thumbnail: name, details and the issue line.
const TEXT_HEIGHT: f32 = 62.0;

/// Something done to a card this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum CardAction {
    Select(PathBuf),
    Open(PathBuf),
    Rename(PathBuf),
    MoveTo { project: PathBuf, folder: PathBuf },
    ShowInFileManager(PathBuf),
    Forget(PathBuf),
}

/// Draws the cards. `now` is the current time in seconds since 1970.
pub fn grid(
    ui: &mut Ui,
    theme: &Theme,
    projects: &[&ProjectEntry],
    selected: Option<&Path>,
    folders: &[Folder],
    recoverable: &HashMap<PathBuf, usize>,
    now: i64,
) -> Vec<CardAction> {
    let mut actions = Vec::new();
    let width = ui.available_width();
    let card_width = ((width - GAP * (COLUMNS as f32 - 1.0)) / COLUMNS as f32)
        .floor()
        .max(120.0);
    let thumb_height = (card_width * 9.0 / 16.0).round();
    let row_height = thumb_height + TEXT_HEIGHT;
    let rows = projects.len().div_ceil(COLUMNS);
    ui.spacing_mut().item_spacing.y = GAP;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for entry in projects.iter().skip(row * COLUMNS).take(COLUMNS) {
                        let is_selected = selected == Some(entry.path.as_path());
                        let unsaved = recoverable.get(&entry.path).copied().unwrap_or(0);
                        card(
                            ui,
                            theme,
                            entry,
                            is_selected,
                            unsaved,
                            folders,
                            now,
                            card_width,
                            thumb_height,
                            &mut actions,
                        );
                    }
                });
            }
        });
    actions
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut Ui,
    theme: &Theme,
    entry: &ProjectEntry,
    selected: bool,
    unsaved: usize,
    folders: &[Folder],
    now: i64,
    width: f32,
    thumb_height: f32,
    actions: &mut Vec<CardAction>,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, thumb_height + TEXT_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    let thumb = Rect::from_min_size(rect.min, vec2(width, thumb_height));

    // Thumbnail. Real pictures arrive once the editor writes them on save.
    painter.rect_filled(thumb, 8.0, theme.viewport);
    let settings = entry.settings.map(|s| s.short_label());
    painter.text(
        thumb.center(),
        Align2::CENTER_CENTER,
        settings.as_deref().unwrap_or("—"),
        FontId::proportional(13.0),
        theme.faint,
    );
    if let Some(length) = entry.length {
        let text = time_text::duration(length.as_secs_f64().max(0.0) as u64);
        let galley = painter.layout_no_wrap(text, ui_kit::fonts::mono(11.0), theme.text);
        let badge = Rect::from_min_size(
            thumb.right_bottom() - galley.size() - vec2(16.0, 12.0),
            galley.size() + vec2(10.0, 4.0),
        );
        painter.rect_filled(badge, 4.0, theme.bg.gamma_multiply(0.85));
        painter.galley(badge.min + vec2(5.0, 2.0), galley, theme.text);
    }
    let stroke = if selected {
        Stroke::new(2.0, theme.accent)
    } else if response.hovered() {
        Stroke::new(1.0, theme.line_strong)
    } else {
        Stroke::new(1.0, theme.line)
    };
    painter.rect_stroke(thumb, 8.0, stroke, StrokeKind::Inside);

    // Text under the thumbnail.
    let mut y = thumb.bottom() + 8.0;
    one_line(
        &painter,
        pos2(rect.left(), y),
        width,
        &entry.name,
        FontId::new(13.0, ui_kit::fonts::semibold()),
        theme.text,
    );
    y += 19.0;
    let mut details = Vec::new();
    if let Some(used) = entry.last_used() {
        details.push(format!(
            "Last used {}",
            time_text::ago(now.saturating_sub(used).max(0) as u64)
        ));
    }
    if let Some(settings) = settings {
        details.push(settings);
    }
    one_line(
        &painter,
        pos2(rect.left(), y),
        width,
        &details.join(" · "),
        FontId::proportional(11.5),
        theme.muted,
    );
    y += 17.0;
    if let Some(issue) = entry.issue {
        let colour = match issue {
            Issue::Missing | Issue::Damaged => status::ERROR,
            Issue::NewerVersion => status::WARNING,
        };
        painter.circle_filled(pos2(rect.left() + 4.0, y + 7.0), 3.5, colour);
        one_line(
            &painter,
            pos2(rect.left() + 14.0, y),
            width - 14.0,
            issue.message(),
            FontId::proportional(11.5),
            colour,
        );
    } else if unsaved > 0 {
        painter.circle_filled(pos2(rect.left() + 4.0, y + 7.0), 3.5, status::WARNING);
        let text = if unsaved == 1 {
            "1 unsaved change from a crash".to_owned()
        } else {
            format!("{unsaved} unsaved changes from a crash")
        };
        one_line(
            &painter,
            pos2(rect.left() + 14.0, y),
            width - 14.0,
            &text,
            FontId::proportional(11.5),
            status::WARNING,
        );
    }

    if response.clicked() {
        actions.push(CardAction::Select(entry.path.clone()));
    }
    if response.double_clicked() {
        actions.push(CardAction::Open(entry.path.clone()));
    }
    response.context_menu(|ui| {
        actions.push(CardAction::Select(entry.path.clone()));
        if ui.button("Open").clicked() {
            actions.push(CardAction::Open(entry.path.clone()));
        }
        let usable = entry.issue.is_none();
        if ui.add_enabled(usable, egui::Button::new("Rename…")).clicked() {
            actions.push(CardAction::Rename(entry.path.clone()));
        }
        ui.add_enabled_ui(usable, |ui| {
            ui.menu_button("Move to", |ui| {
                let here = entry.folder();
                let targets: Vec<&Folder> = folders
                    .iter()
                    .filter(|f| f.kind.holds_projects() && Some(f.path.as_path()) != here)
                    .collect();
                if targets.is_empty() {
                    ui.label(RichText::new("Add another project folder first.").color(theme.muted));
                }
                for folder in targets {
                    let label = match folder.kind {
                        FolderKind::Other => format!("{} (other)", folder.name()),
                        _ => folder.name(),
                    };
                    if ui
                        .button(label)
                        .on_hover_text(folder.path.display().to_string())
                        .clicked()
                    {
                        actions.push(CardAction::MoveTo {
                            project: entry.path.clone(),
                            folder: folder.path.clone(),
                        });
                    }
                }
            });
        });
        if ui
            .add_enabled(
                entry.issue != Some(Issue::Missing),
                egui::Button::new("Show in file manager"),
            )
            .clicked()
        {
            actions.push(CardAction::ShowInFileManager(entry.path.clone()));
        }
        ui.separator();
        if ui
            .button("Remove from list")
            .on_hover_text("The file stays where it is. Opening it again lists it again.")
            .clicked()
        {
            actions.push(CardAction::Forget(entry.path.clone()));
        }
    });
    response.on_hover_text_at_pointer(entry.path.display().to_string());
}

/// Text cut to one line with an ellipsis.
fn one_line(painter: &egui::Painter, at: egui::Pos2, width: f32, text: &str, font: FontId, colour: Color32) {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, colour);
    job.wrap = TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    painter.galley(at, galley, colour);
}

/// The projects to show for a view and search text, in list order.
pub fn filter<'a>(
    projects: &'a [ProjectEntry],
    folder: Option<&Path>,
    search: &str,
) -> Vec<&'a ProjectEntry> {
    let needle = search.trim().to_lowercase();
    projects
        .iter()
        .filter(|p| folder.is_none_or(|f| p.path.starts_with(f)))
        .filter(|p| {
            needle.is_empty()
                || p.name.to_lowercase().contains(&needle)
                || p.path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, name: &str) -> ProjectEntry {
        ProjectEntry {
            path: path.into(),
            name: name.into(),
            settings: None,
            length: None,
            last_opened: None,
            modified: None,
            issue: None,
        }
    }

    #[test]
    fn filtering_by_folder_and_search() {
        let all = [
            entry("/p/trip.x", "Lofoten trip"),
            entry("/p/sub/ranked.x", "Ranked night"),
            entry("/other/trip2.x", "Second trip"),
        ];
        assert_eq!(filter(&all, None, "").len(), 3);
        assert_eq!(
            filter(&all, Some(Path::new("/p")), "").len(),
            2,
            "subfolders count"
        );
        let names: Vec<&str> = filter(&all, None, "TRIP")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["Lofoten trip", "Second trip"]);
        assert_eq!(filter(&all, None, "ranked.x").len(), 1, "file names match too");
        assert!(filter(&all, Some(Path::new("/p")), "second").is_empty());
    }
}
