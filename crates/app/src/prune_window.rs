//! Prune: the dialog that lists the project's files no timeline uses
//! (`project::prune`), every one ticked, with its size, and takes the
//! ticked ones out of the project as one undoable step. Files on disk are
//! never touched; only the project forgets them.
//!
//! Opened by the Prune command (`Action::Prune`: the search palette, a
//! shortcut if one is set, the media drawer's ⋯ menu, the Library page's
//! Prune button). The list is worked out from the project in memory (no
//! disk), and again only when the project's media or timelines change.

use crate::cache_cleanup::size_text;
use project::{MediaId, MediaRef, Project};
use std::collections::BTreeSet;
use ui_kit::Theme;

#[derive(Default)]
pub struct PruneWindow {
    open: bool,
    /// The project the list was worked out for (compared by sharing, so
    /// an unchanged project is never looked through again).
    listed_for: Option<Project>,
    unused: Vec<MediaId>,
    /// Files unticked by hand: kept in the project.
    kept: BTreeSet<MediaId>,
}

/// What the dialog asked for this frame.
#[derive(Debug, PartialEq)]
pub enum Asked {
    Nothing,
    /// Take these files out of the project (`Command::RemoveMedia`);
    /// `says` is for the status line once it is done.
    Prune {
        media: Vec<MediaId>,
        says: String,
    },
}

impl PruneWindow {
    /// Opens the dialog with every unused file ticked.
    pub fn open(&mut self) {
        self.open = true;
        self.listed_for = None;
        self.kept.clear();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The files no timeline uses, worked out again only when the project's
    /// media or timelines changed (undo while the dialog is open).
    fn unused(&mut self, project: &Project) -> &[MediaId] {
        let same = self
            .listed_for
            .as_ref()
            .is_some_and(|p| p.media.ptr_eq(&project.media) && p.sequences.ptr_eq(&project.sequences));
        if !same {
            self.unused = project::prune::unused_media(project);
            self.kept.retain(|m| project.media.contains_key(m));
            self.listed_for = Some(project.clone());
        }
        &self.unused
    }

    /// The dialog, while open. `missing`: files that aren't on disk;
    /// `undo`: Undo's keys, for the words.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        project: &Project,
        missing: &[MediaId],
        undo: &str,
    ) -> Asked {
        if !self.open {
            return Asked::Nothing;
        }
        let unused: Vec<&MediaRef> = self
            .unused(project)
            .to_vec()
            .iter()
            .filter_map(|id| project.media.get(id))
            .collect();
        let chosen: Vec<&MediaRef> = unused
            .iter()
            .copied()
            .filter(|m| !self.kept.contains(&m.id))
            .collect();
        let chosen_bytes: u64 = chosen.iter().map(|m| m.fingerprint.size).sum();
        let (mut prune, mut close) = (false, false);
        let modal = ui_kit::dialog(ctx, theme, "prune", |ui| {
            ui.set_width(520.0);
            ui.label(
                egui::RichText::new("Prune unused media")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(6.0);
            if unused.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Every file in the project is used on a timeline: there is nothing to prune.",
                    )
                    .color(theme.text_secondary),
                );
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
                return;
            }
            ui.label(
                egui::RichText::new(format!(
                    "No timeline uses these files. The ticked ones are taken out of the project; the files stay on disk, and {undo} brings them back."
                ))
                .color(theme.text_secondary),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.small_button("Tick all").clicked() {
                    self.kept.clear();
                }
                if ui.small_button("Tick none").clicked() {
                    self.kept.extend(unused.iter().map(|m| m.id));
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for media in &unused {
                        let mut ticked = !self.kept.contains(&media.id);
                        let about = Row::of(project, media, missing.contains(&media.id));
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut ticked, "").changed() {
                                if ticked {
                                    self.kept.remove(&media.id);
                                } else {
                                    self.kept.insert(media.id);
                                }
                            }
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&about.name).color(theme.text));
                                ui.label(egui::RichText::new(&about.details).size(11.5).color(theme.muted))
                                    .on_hover_text(media.path.display().to_string());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(size_text(media.fingerprint.size))
                                        .color(theme.text_secondary),
                                );
                            });
                        });
                    }
                });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let words = remove_words(chosen.len(), chosen_bytes);
                let button =
                    ui.add_enabled_ui(!chosen.is_empty(), |ui| ui_kit::accent_button(ui, theme, &words));
                if button.inner.clicked() {
                    prune = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.open = false;
            return Asked::Nothing;
        }
        if !prune || chosen.is_empty() {
            return Asked::Nothing;
        }
        self.open = false;
        Asked::Prune {
            says: pruned_says(chosen.len(), chosen_bytes, undo),
            media: chosen.iter().map(|m| m.id).collect(),
        }
    }
}

/// One line of the list: the file's name, and where it is with what else
/// the project knows about it.
struct Row {
    name: String,
    details: String,
}

impl Row {
    fn of(project: &Project, media: &MediaRef, missing: bool) -> Row {
        let name = media.path.file_name().map_or_else(
            || media.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut parts = Vec::new();
        if missing {
            parts.push("can't be found".to_owned());
        }
        if let Some(folder) = media.path.parent().and_then(|f| f.file_name()) {
            parts.push(format!("in {}", folder.to_string_lossy()));
        }
        let library = &project.library;
        let labels: Vec<&str> = library
            .labels_of(media.id)
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        if !labels.is_empty() {
            parts.push(labels.join(", "));
        }
        match library.lists_of(media.id).len() {
            0 => {}
            1 => parts.push("in a list".to_owned()),
            n => parts.push(format!("in {n} lists")),
        }
        if !library.note(media.id).is_empty() {
            parts.push("has a note".to_owned());
        }
        Row {
            name,
            details: parts.join(" · "),
        }
    }
}

/// The button: "Remove 3 files (2.1 GB) from the project".
pub fn remove_words(files: usize, bytes: u64) -> String {
    match files {
        0 => "Remove from the project".to_owned(),
        1 => format!("Remove 1 file ({}) from the project", size_text(bytes)),
        n => format!("Remove {n} files ({}) from the project", size_text(bytes)),
    }
}

/// The status line after pruning.
pub fn pruned_says(files: usize, bytes: u64, undo: &str) -> String {
    let what = match files {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    format!(
        "Took {what} ({}) out of the project. Nothing was deleted from disk; {undo} brings them back.",
        size_text(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_say_how_much_and_that_nothing_is_deleted() {
        assert_eq!(remove_words(0, 0), "Remove from the project");
        assert_eq!(
            remove_words(1, 2_000_000),
            "Remove 1 file (2 MB) from the project"
        );
        assert_eq!(
            remove_words(3, 2_100_000_000),
            "Remove 3 files (2.1 GB) from the project"
        );
        let says = pruned_says(2, 5_000_000, "Ctrl+Z");
        assert!(says.contains("2 files (5 MB)"));
        assert!(says.contains("Nothing was deleted from disk"));
        assert!(says.contains("Ctrl+Z brings them back"));
    }

    #[test]
    fn the_list_follows_the_project_and_keeps_what_was_unticked() {
        use project::{Command, Fingerprint, MediaKind, SequenceSettings, Time};
        let mut p = Project::new("P", SequenceSettings::GAME_CLIP);
        for name in ["a.mkv", "b.mkv"] {
            p = Command::AddMedia {
                media: MediaRef {
                    id: MediaId(0),
                    path: name.into(),
                    fingerprint: Fingerprint {
                        size: 1,
                        sample_hash: name.into(),
                    },
                    kind: MediaKind::Video,
                    duration: Time::seconds(1),
                    info: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        let mut window = PruneWindow::default();
        window.open();
        assert_eq!(window.unused(&p).len(), 2);
        let first = window.unused[0];
        window.kept.insert(first);
        // Unchanged project: same list, the untick stays.
        assert_eq!(window.unused(&p).len(), 2);
        assert!(window.kept.contains(&first));
        // The file went (another change): it is dropped from the list.
        let q = Command::RemoveMedia { media: vec![first] }.apply(&p).unwrap();
        assert_eq!(window.unused(&q).len(), 1);
        assert!(window.kept.is_empty());
    }
}
