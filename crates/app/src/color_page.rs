//! The Color page: the picture large, one tool at a time under it (Basic or
//! Wheels for now), a layer dropdown with a Simple / Advanced switch, and
//! the bar of what is being graded with Apply to, Copy grade, Paste and
//! Reset. A clip's grade is `project::Grade`; every change is one undo
//! step (`Command::SetGrades`), a value being dragged is shown at once
//! (`ColorPage::shown`) and becomes one when let go.

mod basic;
mod wheels;

use project::{ClipId, Command, Grade, Primary, Project, SequenceId, Time};
use ui_kit::Theme;

/// The tools, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Basic,
    Wheels,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Basic, Tool::Wheels];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Basic => "Basic",
            Tool::Wheels => "Wheels",
        }
    }
}

/// Which clips "Apply" puts the grade on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ApplyTo {
    /// Every picture of a file on the timeline.
    #[default]
    EveryClip,
    /// Every picture of the same file.
    SameFile,
    /// Every picture whose file has this label.
    Label(project::MediaLabelId),
}

/// The page's own state.
#[derive(Debug, Default)]
pub struct ColorPage {
    pub tool: Tool,
    /// The layer the tools change.
    pub layer: usize,
    /// Advanced wheels: larger, with typed numbers.
    pub advanced: bool,
    pub apply_to: ApplyTo,
    /// A grade copied with Copy grade, for Paste.
    pub copied: Option<Grade>,
    /// A grade being dragged, not yet an undo step.
    changing: Option<(ClipId, Grade)>,
    /// The scope shown.
    pub scope: engine::scope::ScopeKind,
    /// The scope's picture on the card, and what it was made from.
    scope_picture: Option<ScopePicture>,
}

/// A scope's picture on the card: what it was made from, and the texture.
struct ScopePicture(usize, engine::scope::ScopeKind, egui::TextureHandle);

impl std::fmt::Debug for ScopePicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScopePicture({:?})", self.1)
    }
}

/// What the page's controls asked for, once drawing is done.
#[derive(Debug, Default)]
pub struct Asked {
    pub commands: Vec<Command>,
    pub status: Option<String>,
}

/// The picture clip a grade goes on: the first selected picture of a file,
/// else the top one under the playhead.
pub fn graded_clip(seq: &project::Sequence, selected: &[ClipId], playhead: Time) -> Option<ClipId> {
    let pictures = || {
        seq.video_tracks
            .iter()
            .filter(|t| !t.hidden)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_file() || c.is_title())
    };
    selected
        .iter()
        .find_map(|id| pictures().find(|c| c.id == *id))
        .or_else(|| {
            seq.video_tracks.iter().rev().filter(|t| !t.hidden).find_map(|t| {
                t.clips
                    .iter()
                    .find(|c| (c.is_file() || c.is_title()) && c.start <= playhead && playhead < c.end())
            })
        })
        .map(|c| c.id)
}

/// The clips "Apply" reaches from `clip`.
pub fn reached(project: &Project, seq: &project::Sequence, clip: ClipId, to: &ApplyTo) -> Vec<ClipId> {
    let Some(from) = seq
        .video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
    else {
        return Vec::new();
    };
    seq.video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_file())
        .filter(|c| match to {
            ApplyTo::EveryClip => true,
            ApplyTo::SameFile => c.media == from.media,
            ApplyTo::Label(label) => project
                .library
                .tagged
                .get(&c.media)
                .is_some_and(|labels| labels.contains(label)),
        })
        .map(|c| c.id)
        .collect()
}

impl ColorPage {
    /// The project with the grade being dragged, for the viewer.
    pub fn shown(&self, project: &Project) -> Option<Project> {
        let (clip, grade) = self.changing.as_ref()?;
        Command::SetGrades {
            sequence: project.active_sequence,
            grades: vec![(*clip, grade.clone())],
        }
        .apply(project)
        .ok()
    }

    /// The tools for `clip`'s grade: the switcher, the layer dropdown and
    /// the Simple / Advanced switch, then the tool shown.
    pub fn tools_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        sequence: SequenceId,
        clip: &project::Clip,
    ) -> Asked {
        let stored = &clip.grade;
        let mut grade = match &self.changing {
            Some((id, grade)) if *id == clip.id => grade.clone(),
            _ => stored.clone(),
        };
        let layers = grade.0.len().max(1);
        self.layer = self.layer.min(layers);
        ui.horizontal(|ui| {
            for tool in Tool::ALL {
                if ui.selectable_label(self.tool == tool, tool.name()).clicked() {
                    self.tool = tool;
                }
            }
            ui.separator();
            let name = |i: usize| match grade.0.get(i) {
                Some(layer) => format!("Layer {} · {}", i + 1, layer.name),
                None if i == 0 => format!("Layer 1 · {}", Grade::FIRST_LAYER),
                None => format!("Layer {}", i + 1),
            };
            egui::ComboBox::from_id_salt("grade layer")
                .selected_text(name(self.layer))
                .show_ui(ui, |ui| {
                    for i in 0..layers {
                        ui.selectable_value(&mut self.layer, i, name(i));
                    }
                    if ui.selectable_label(false, "+ Add a layer").clicked() {
                        self.layer = layers;
                    }
                });
            if let Some(layer) = grade.0.get_mut(self.layer) {
                let mut on = !layer.off;
                if ui
                    .checkbox(&mut on, "On")
                    .on_hover_text("Switch this layer off to compare")
                    .changed()
                {
                    layer.off = !on;
                }
            }
            if self.tool == Tool::Wheels {
                ui.separator();
                for (advanced, label) in [(false, "Simple"), (true, "Advanced")] {
                    if ui.selectable_label(self.advanced == advanced, label).clicked() {
                        self.advanced = advanced;
                    }
                }
            }
        });
        ui.add_space(8.0);
        let before = grade.0.get(self.layer).map_or(Primary::PLAIN, |l| l.primary);
        let mut primary = before;
        let busy = match self.tool {
            Tool::Basic => basic::ui(ui, theme, &mut primary),
            Tool::Wheels => wheels::ui(ui, theme, &mut primary, self.advanced),
        };
        if primary != before {
            grade.layer_mut(self.layer).primary = primary;
        }
        let mut asked = Asked::default();
        if grade == *stored {
            self.changing = None;
        } else if busy {
            self.changing = Some((clip.id, grade));
        } else {
            self.changing = None;
            asked.commands.push(Command::SetGrades {
                sequence,
                grades: vec![(clip.id, grade)],
            });
        }
        asked
    }

    /// The scope chosen, of `sampled` (the newest small copy of the
    /// picture), with its line in plain words.
    pub fn scope_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        sampled: Option<&std::sync::Arc<engine::scope::Sampled>>,
    ) {
        use engine::scope::ScopeKind;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("scope")
                .selected_text(self.scope.name())
                .show_ui(ui, |ui| {
                    for kind in ScopeKind::ALL {
                        ui.selectable_value(&mut self.scope, kind, kind.name());
                    }
                });
        });
        let Some(sampled) = sampled else {
            ui.label(egui::RichText::new("The scope shows once the picture is ready.").color(theme.muted));
            return;
        };
        let key = std::sync::Arc::as_ptr(sampled) as usize;
        let fresh = self
            .scope_picture
            .as_ref()
            .is_none_or(|ScopePicture(k, kind, _)| *k != key || *kind != self.scope);
        if fresh {
            let image = sampled.image(self.scope);
            let colour = egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba);
            let texture = ui
                .ctx()
                .load_texture("scope", colour, egui::TextureOptions::LINEAR);
            self.scope_picture = Some(ScopePicture(key, self.scope, texture));
        }
        if let Some(ScopePicture(_, _, texture)) = &self.scope_picture {
            let width = ui.available_width();
            let height = match self.scope {
                ScopeKind::Vectorscope => width.min(ui.available_height() - 24.0).max(40.0),
                _ => (width * 0.55).min(ui.available_height() - 24.0).max(40.0),
            };
            let size = match self.scope {
                ScopeKind::Vectorscope => egui::vec2(height, height),
                _ => egui::vec2(width, height),
            };
            ui.add(egui::Image::new(texture).fit_to_exact_size(size).corner_radius(4));
        }
        ui.label(
            egui::RichText::new(sampled.reading())
                .color(theme.muted)
                .size(12.0),
        );
    }

    /// The bar over the tools: what is being graded, and what can be done
    /// with its grade.
    pub fn bar_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        clip: &project::Clip,
        selected: &[ClipId],
    ) -> Asked {
        let mut asked = Asked::default();
        let Some(seq) = project.active() else { return asked };
        let sequence = seq.id;
        ui.label(egui::RichText::new(project.clip_name(clip)).strong());
        ui.label(
            egui::RichText::new(match clip.grade.is_graded() {
                true => "Graded",
                false => "Not graded yet",
            })
            .color(theme.muted)
            .size(12.0),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let labels: Vec<(project::MediaLabelId, String)> = project
                .library
                .labels
                .iter()
                .map(|l| (l.id, l.name.clone()))
                .collect();
            let shown = match &self.apply_to {
                ApplyTo::EveryClip => "Every clip".to_owned(),
                ApplyTo::SameFile => "Clips of this file".to_owned(),
                ApplyTo::Label(id) => labels.iter().find(|(l, _)| l == id).map_or_else(
                    || "A label".to_owned(),
                    |(_, name)| format!("Clips labelled {name}"),
                ),
            };
            egui::ComboBox::from_id_salt("apply grade to")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.apply_to, ApplyTo::EveryClip, "Every clip");
                    ui.selectable_value(&mut self.apply_to, ApplyTo::SameFile, "Clips of this file");
                    for (id, name) in &labels {
                        ui.selectable_value(
                            &mut self.apply_to,
                            ApplyTo::Label(*id),
                            format!("Clips labelled {name}"),
                        );
                    }
                });
            if ui
                .button("Apply")
                .on_hover_text("Puts this clip's grade on those clips too: one undo step")
                .clicked()
            {
                let clips = reached(project, seq, clip.id, &self.apply_to);
                asked.status = Some(format!("Grade applied to {} clips", clips.len()));
                asked.commands.push(Command::SetGrades {
                    sequence,
                    grades: clips.into_iter().map(|id| (id, clip.grade.clone())).collect(),
                });
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Copy grade").clicked() {
                self.copied = Some(clip.grade.clone());
                asked.status = Some("Grade copied: select clips and Paste".into());
            }
            let paste = ui.add_enabled(self.copied.is_some(), egui::Button::new("Paste"));
            if paste
                .on_hover_text("Puts the copied grade on the selected pictures, or this one")
                .clicked()
                && let Some(copied) = &self.copied
            {
                let mut onto: Vec<ClipId> = selected
                    .iter()
                    .copied()
                    .filter(|id| {
                        seq.video_tracks
                            .iter()
                            .flat_map(|t| t.clips.iter())
                            .any(|c| c.id == *id)
                    })
                    .collect();
                if onto.is_empty() {
                    onto.push(clip.id);
                }
                asked.commands.push(Command::SetGrades {
                    sequence,
                    grades: onto.into_iter().map(|id| (id, copied.clone())).collect(),
                });
            }
            let reset = ui.add_enabled(clip.grade != Grade::default(), egui::Button::new("Reset"));
            if reset.on_hover_text("Takes this clip's grade away").clicked() {
                asked.commands.push(Command::SetGrades {
                    sequence,
                    grades: vec![(clip.id, Grade::default())],
                });
            }
        });
        asked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaInfo, MediaKind, MediaRef, SequenceSettings, VideoStream};

    fn project_with_two_files() -> Project {
        let mut p = Project::new("T", SequenceSettings::GAME_CLIP);
        for name in ["a.mkv", "b.mkv"] {
            let media = MediaRef {
                id: project::MediaId(0),
                path: name.into(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: name.into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(4),
                info: MediaInfo {
                    video: Some(VideoStream {
                        index: 0,
                        codec: "h264".into(),
                        width: 1920,
                        height: 1080,
                        frame_rate: project::Rate::FPS_60,
                        rotation: 0,
                        variable_frame_rate: false,
                    }),
                    ..MediaInfo::default()
                },
            };
            p = Command::Import {
                sequence: p.active_sequence,
                media,
            }
            .apply(&p)
            .expect("imports");
        }
        p
    }

    #[test]
    fn the_graded_clip_is_the_selected_picture_or_the_one_at_the_playhead() {
        let p = project_with_two_files();
        let seq = p.active().expect("a timeline");
        let clips: Vec<ClipId> = seq.video_tracks[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(graded_clip(seq, &[], Time::seconds(5)), Some(clips[1]));
        assert_eq!(graded_clip(seq, &[clips[0]], Time::seconds(5)), Some(clips[0]));
        assert_eq!(graded_clip(seq, &[], Time::seconds(30)), None, "past the end");
    }

    #[test]
    fn apply_reaches_every_clip_or_those_of_the_same_file() {
        let p = project_with_two_files();
        let seq = p.active().expect("a timeline");
        let first = seq.video_tracks[0].clips[0].id;
        assert_eq!(reached(&p, seq, first, &ApplyTo::EveryClip).len(), 2);
        assert_eq!(reached(&p, seq, first, &ApplyTo::SameFile), vec![first]);
    }
}
