//! "Remove from project" on the Library page: files no timeline uses go at
//! once (`Command::RemoveMedia`); when some are used, a warning says where
//! and offers "Remove anyway", which takes their clips off every timeline
//! too (`Command::RemoveMediaAndClips`). Either is one undo step, and
//! nothing is ever deleted from disk.

use super::*;
use project::{MediaId, Project};

/// Files asked to be removed while a timeline uses some of them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RemovalAsk {
    /// Every file asked for.
    pub media: Vec<MediaId>,
    /// Those of them no timeline uses.
    pub unused: Vec<MediaId>,
    /// What the warning says.
    pub says: String,
}

/// What was chosen in the warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Anyway,
    OnlyUnused,
    Cancel,
}

/// The warning for removing `media` from `project`, or `None` when no
/// timeline uses any of it.
pub(crate) fn ask(project: &Project, media: Vec<MediaId>) -> Option<RemovalAsk> {
    let used = project::prune::used_media(project);
    let (in_use, unused): (Vec<_>, Vec<_>) = media.iter().partition(|m| used.contains(m));
    if in_use.is_empty() {
        return None;
    }
    let (clips, timelines) = project::prune::uses(project, &in_use);
    let files = match (in_use.len(), media.len()) {
        (1, 1) => file_name(project, in_use[0]),
        (1, _) => format!("1 of the {} files", media.len()),
        (n, all) if n == all => format!("These {n} files are"),
        (n, all) => format!("{n} of the {all} files are"),
    };
    let files = if in_use.len() == 1 {
        format!("{files} is")
    } else {
        files
    };
    let clips = match clips {
        1 => "1 clip".to_owned(),
        n => format!("{n} clips"),
    };
    let on = match timelines.as_slice() {
        [one] => format!("the timeline \"{one}\""),
        many => format!("{} timelines ({})", many.len(), many.join(", ")),
    };
    let says = format!(
        "{files} used in {clips} on {on}. Remove anyway takes those clips off the timelines too, leaving gaps. Nothing is deleted from disk, and undo brings everything back."
    );
    Some(RemovalAsk { media, unused, says })
}

fn file_name(project: &Project, media: MediaId) -> String {
    project.media.get(&media).map_or_else(
        || "This file".to_owned(),
        |m| {
            m.path.file_name().map_or_else(
                || m.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        },
    )
}

impl Editor {
    /// Takes the chosen files out of the project as one step, or asks first
    /// when a timeline uses any of them.
    pub(super) fn remove_from_project(&mut self, media: Vec<MediaId>) {
        let Some(document) = &self.document else { return };
        match ask(document.project(), media.clone()) {
            Some(asked) => self.removal = Some(asked),
            None => self.remove_media(Command::RemoveMedia { media: media.clone() }, &media),
        }
    }

    /// The warning, while it is asked.
    pub(super) fn removal_window(&mut self, ctx: &egui::Context) {
        let Some(asked) = &self.removal else { return };
        let mut answer = None;
        let theme = &self.theme;
        let modal = ui_kit::dialog::dialog(ctx, theme, "remove-used-media", |ui| {
            ui.set_width(420.0);
            ui.label(
                egui::RichText::new("Used on a timeline")
                    .family(ui_kit::fonts::semibold())
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label(&asked.says);
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, theme, "Remove anyway").clicked() {
                    answer = Some(Answer::Anyway);
                }
                if !asked.unused.is_empty()
                    && ui
                        .button(format!("Remove only the {} unused", asked.unused.len()))
                        .on_hover_text("The files a timeline uses stay, with their clips")
                        .clicked()
                {
                    answer = Some(Answer::OnlyUnused);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(Answer::Cancel);
                }
            });
        });
        if answer.is_none() && modal.should_close() {
            answer = Some(Answer::Cancel);
        }
        let Some(answer) = answer else { return };
        let Some(asked) = self.removal.take() else { return };
        match answer {
            Answer::Anyway => {
                let media = asked.media;
                self.remove_media(Command::RemoveMediaAndClips { media: media.clone() }, &media);
            }
            Answer::OnlyUnused => {
                let media = asked.unused;
                self.remove_media(Command::RemoveMedia { media: media.clone() }, &media);
            }
            Answer::Cancel => {}
        }
    }

    /// Applies `command`, which takes `media` out of the project, and says so.
    fn remove_media(&mut self, command: Command, media: &[MediaId]) {
        match self.with_document(|d| d.apply(command)) {
            Ok(()) => {
                for m in media {
                    self.waveforms.forget(*m);
                    self.pictures.forget(*m);
                }
                self.selected.retain(|clip| {
                    self.document
                        .as_ref()
                        .and_then(|d| d.project().active())
                        .is_some_and(|seq| seq.tracks().any(|t| t.clips.iter().any(|c| c.id == *clip)))
                });
                let undo = self.shortcuts.keys_or_name(Action::Undo);
                let files = match media.len() {
                    1 => "1 file".to_owned(),
                    n => format!("{n} files"),
                };
                self.status = format!(
                    "Took {files} out of the project. Nothing was deleted from disk ({undo} brings them back)."
                );
            }
            Err(err) => self.errors.push(&err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Clip, Fingerprint, MediaRef, SequenceSettings};

    /// The test recording (a picture and three sound streams) as `name`.
    fn recording(name: &str) -> MediaRef {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures");
        let probed = media::probe(&fixtures.join("game-3-audio.mkv")).unwrap();
        MediaRef {
            id: MediaId(0),
            path: fixtures.join(name),
            fingerprint: Fingerprint {
                size: 5,
                sample_hash: name.into(),
            },
            kind: probed.kind,
            duration: probed.duration,
            info: probed.info,
        }
    }

    fn with_files(used: usize, unused: usize) -> (Project, Vec<MediaId>) {
        let mut p = Project::new("T", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        for i in 0..used + unused {
            let media = recording(&format!("clip{i}.mkv"));
            p = match i < used {
                true => Command::Import { sequence, media },
                false => Command::AddMedia { media },
            }
            .apply(&p)
            .unwrap();
        }
        let ids = p.media.keys().copied().collect();
        (p, ids)
    }

    #[test]
    fn files_no_timeline_uses_go_without_asking() {
        let (p, ids) = with_files(0, 2);
        assert_eq!(ask(&p, ids), None);
    }

    #[test]
    fn the_warning_says_where_the_files_are_used() {
        let (p, ids) = with_files(1, 0);
        let asked = ask(&p, ids.clone()).unwrap();
        assert!(asked.unused.is_empty());
        assert!(
            asked
                .says
                .starts_with("clip0.mkv is used in 4 clips on the timeline"),
            "{}",
            asked.says
        );
        let (p, ids) = with_files(2, 1);
        let asked = ask(&p, ids.clone()).unwrap();
        assert_eq!(asked.unused, [ids[2]]);
        assert!(
            asked.says.starts_with("2 of the 3 files are used in 8 clips"),
            "{}",
            asked.says
        );
        // Remove anyway leaves nothing of them behind.
        let after = Command::RemoveMediaAndClips { media: asked.media }
            .apply(&p)
            .unwrap();
        assert!(after.media.is_empty());
        let clips: Vec<&Clip> = after
            .active()
            .unwrap()
            .tracks()
            .flat_map(|t| t.clips.iter())
            .collect();
        assert!(clips.is_empty());
    }
}
