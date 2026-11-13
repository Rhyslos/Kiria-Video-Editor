//! "New in your folders": new recordings in the media folders
//! (`media_folders.rs`), each with Add (to the project's media, after its
//! import checks, never on a timeline) and × (hidden for the session), and
//! "Add all". Drawn the same on the Library page (its sidebar) and in the
//! Edit page's media drawer.

use ui_kit::Theme;

/// New files in the media folders, for "New in your folders".
pub struct NewFiles<'a> {
    pub files: &'a [library::Found],
    /// Whether a file is being checked for adding now.
    pub checking: &'a dyn Fn(&std::path::Path) -> bool,
}

/// What was asked for there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// Add these files to the project's media.
    Add(Vec<std::path::PathBuf>),
    /// Hide this one for the session.
    Dismiss(std::path::PathBuf),
}

/// At most this many new files are listed; "Add all" takes every one.
const NEW_SHOWN: usize = 4;

/// "New in your folders": the newest few new files, each with Add and ×,
/// and "Add all" when there are several.
pub fn ui(ui: &mut egui::Ui, theme: &Theme, new: &NewFiles<'_>) -> Option<Asked> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("New in your folders ({})", new.files.len()))
                .font(egui::FontId::new(12.5, ui_kit::fonts::semibold()))
                .color(theme.text),
        )
        .on_hover_text(
            "Recordings in your media folders (the launcher's Media folders) changed since this project was last saved and finished being written. Nothing is added by itself.",
        );
        if new.files.len() > 1
            && ui
                .small_button("Add all")
                .on_hover_text("Add every one to the project's media, after checking it")
                .clicked()
        {
            picked = Some(Asked::Add(new.files.iter().map(|f| f.path.clone()).collect()));
        }
    });
    for file in new.files.iter().take(NEW_SHOWN) {
        let checking = (new.checking)(&file.path);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 20.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if checking {
                    ui.spinner();
                } else {
                    if ui.small_button("×").on_hover_text("Hide it for now").clicked() {
                        picked = Some(Asked::Dismiss(file.path.clone()));
                    }
                    if ui
                        .small_button("Add")
                        .on_hover_text("Add it to the project's media, after checking it")
                        .clicked()
                    {
                        picked = Some(Asked::Add(vec![file.path.clone()]));
                    }
                }
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &library::file_name(&file.path),
                    0.0,
                    egui::TextFormat::simple(egui::FontId::proportional(12.0), theme.text),
                );
                job.append(
                    &format!(
                        "  {} · {}",
                        size_text(file.size),
                        library::file_name(&file.folder)
                    ),
                    0.0,
                    egui::TextFormat::simple(egui::FontId::proportional(11.0), theme.muted),
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width());
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(job).truncate())
                        .on_hover_text(file.path.display().to_string());
                });
            },
        );
    }
    if new.files.len() > NEW_SHOWN {
        ui.label(
            egui::RichText::new(format!("and {} more", new.files.len() - NEW_SHOWN))
                .size(11.0)
                .color(theme.muted),
        );
    }
    picked
}

/// A file size in a few characters: "840 KB", "12.3 MB", "4.1 GB".
fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_in_a_few_characters() {
        assert_eq!(size_text(200), "1 KB");
        assert_eq!(size_text(840_000), "840 KB");
        assert_eq!(size_text(12_300_000), "12.3 MB");
        assert_eq!(size_text(4_100_000_000), "4.1 GB");
    }
}
