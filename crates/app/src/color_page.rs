//! The Color page: the picture large, one tool at a time under it (Basic,
//! Wheels, Curves, Range or Mask), a layer dropdown with a Simple / Advanced switch, and
//! the bar of what is being graded with Apply to, Copy grade, Paste and
//! Reset. A clip's grade is `project::Grade`; every change is one undo
//! step (`Command::SetGrades`), a value being dragged is shown at once
//! (`ColorPage::shown`) and becomes one when let go.

mod basic;
mod curves;
mod mask;
mod range;
mod wheels;

use crate::pending::Pending;
pub use mask::{dragged as mask_dragged, outline as mask_outline};
use project::{ClipId, Command, Grade, Primary, Project, SequenceId, Time};
use ui_kit::Theme;

/// The tools, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Basic,
    Wheels,
    Curves,
    /// The colours a layer reaches.
    Range,
    /// The shape a layer reaches.
    Mask,
}

impl Tool {
    pub const ALL: [Tool; 5] = [Tool::Basic, Tool::Wheels, Tool::Curves, Tool::Range, Tool::Mask];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Basic => "Basic",
            Tool::Wheels => "Wheels",
            Tool::Curves => "Curves",
            Tool::Range => "Range",
            Tool::Mask => "Mask",
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
#[derive(Debug)]
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
    changing: Pending<ClipId, Grade>,
    /// The scope shown.
    pub scope: engine::scope::ScopeKind,
    /// How much of Auto white balance's correction is used, 0 to 1.
    pub auto_strength: f64,
    /// The curve the Curves tool shows.
    curve_channel: curves::Channel,
    /// "Pick white" is on: the next click on the picture picks.
    pub picking_white: bool,
    /// "Pick from the picture" (Range) is on: the next click picks.
    pub picking_range: bool,
    /// The mask is being dragged on the picture.
    mask_held: bool,
    /// The picture is shown as it was before its grade.
    pub before: bool,
    /// Auto white balance looking at a clip: which, and the work.
    balancing: Option<(ClipId, engine::white_balance::Balancing)>,
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

impl Default for ColorPage {
    fn default() -> ColorPage {
        ColorPage {
            tool: Tool::default(),
            layer: 0,
            advanced: false,
            apply_to: ApplyTo::default(),
            copied: None,
            changing: Pending::default(),
            scope: engine::scope::ScopeKind::default(),
            scope_picture: None,
            auto_strength: 1.0,
            picking_white: false,
            picking_range: false,
            mask_held: false,
            curve_channel: curves::Channel::default(),
            before: false,
            balancing: None,
        }
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
    /// The project as the viewer shows it: with the grade being dragged,
    /// or with `graded`'s grade taken away while Before is on.
    pub fn shown(&self, project: &Project, graded: Option<ClipId>) -> Option<Project> {
        let grades = match (self.before, graded, self.changing.get()) {
            (true, Some(clip), _) => vec![(clip, Grade::default())],
            (_, _, Some((clip, grade))) => vec![(*clip, grade.clone())],
            _ => return None,
        };
        Command::SetGrades {
            sequence: project.active_sequence,
            grades,
        }
        .apply(project)
        .ok()
    }

    /// Finishes a grade left being dragged on a clip that is no longer the
    /// one graded (`graded`): with nothing selected the graded clip follows
    /// the playhead, which can cross into the next clip mid-drag. The grade
    /// becomes its undo step on the clip it was dragged on (when it changes
    /// anything), so it is neither lost nor carried to the next clip. Run
    /// once a frame, before the tools.
    pub fn finish_left_behind(&mut self, seq: &project::Sequence, graded: Option<ClipId>) -> Asked {
        let mut asked = Asked::default();
        let stored = |id: &ClipId| {
            seq.video_tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .find(|c| c.id == *id)
                .map(|c| c.grade.clone())
        };
        let left = self.changing.get().is_some_and(|(id, _)| Some(*id) != graded);
        if left {
            self.mask_held = false;
        }
        if let Some((clip, grade)) = self.changing.finish_on_change(|id| Some(*id) == graded, stored) {
            asked.commands.push(Command::SetGrades {
                sequence: seq.id,
                grades: vec![(clip, grade)],
            });
        }
        asked
    }

    /// "Pick white": the picture at `at` (0 to 1 across and down) should
    /// be neutral, so the shown layer's Temperature and Tint change by what
    /// makes it so, read from `sampled` (the picture as it is now).
    pub fn pick_white(
        &mut self,
        sequence: SequenceId,
        clip: &project::Clip,
        sampled: &engine::scope::Sampled,
        at: (f32, f32),
    ) -> Asked {
        self.picking_white = false;
        let mut asked = Asked::default();
        let Some(light) = sampled.light_around(at, 2) else {
            return asked;
        };
        let (temperature, tint) = engine::white_balance::sliders(light);
        let mut grade = clip.grade.clone();
        let primary = &mut grade.layer_mut(self.layer).primary;
        let moved = |value: i32, by: f32| {
            value
                .saturating_add((by * 1_000_000.0).round() as i32)
                .clamp(-1_000_000, 1_000_000)
        };
        primary.temperature = moved(primary.temperature, temperature);
        primary.tint = moved(primary.tint, tint);
        asked.status =
            Some("White balance set from what you picked: fine-tune it with Temperature and Tint".into());
        asked.commands.push(Command::SetGrades {
            sequence,
            grades: vec![(clip.id, grade)],
        });
        asked
    }

    /// "Pick from the picture": the shown layer's colour range goes round
    /// the colour at `at` (0 to 1 across and down) of `sampled`.
    pub fn pick_range(
        &mut self,
        sequence: SequenceId,
        clip: &project::Clip,
        sampled: &engine::scope::Sampled,
        at: (f32, f32),
    ) -> Asked {
        self.picking_range = false;
        let mut asked = Asked::default();
        let Some(light) = sampled.light_around(at, 1) else {
            return asked;
        };
        let mut grade = clip.grade.clone();
        let layer = grade.layer_mut(self.layer);
        let softness = layer.range.map(|r| r.softness);
        let mut picked = range::around(light);
        if let Some(softness) = softness {
            picked.softness = softness;
        }
        layer.range = Some(picked);
        asked.status = Some(
            "The layer now reaches colours like the one you picked: widen or narrow it with the sliders"
                .into(),
        );
        asked.commands.push(Command::SetGrades {
            sequence,
            grades: vec![(clip.id, grade)],
        });
        asked
    }

    /// The mask of the layer shown on `clip`, as it is now (being
    /// dragged or not), while the Mask tool is open.
    pub fn mask_shown(&self, clip: &project::Clip) -> Option<project::Mask> {
        if self.tool != Tool::Mask {
            return None;
        }
        let grade = self.changing.value_for(&clip.id).unwrap_or(&clip.grade);
        grade.0.get(self.layer)?.mask
    }

    /// The mask dragged on the picture to `mask`: shown at once, one undo
    /// step when `done`.
    pub fn drag_mask(
        &mut self,
        sequence: SequenceId,
        clip: &project::Clip,
        mask: project::Mask,
        done: bool,
    ) -> Asked {
        let mut grade = self.changing.value_for(&clip.id).unwrap_or(&clip.grade).clone();
        grade.layer_mut(self.layer).mask = Some(mask);
        self.mask_held = !done;
        let mut asked = Asked::default();
        if !done {
            self.changing.set(clip.id, grade);
        } else {
            self.changing.clear();
            if grade != clip.grade {
                asked.commands.push(Command::SetGrades {
                    sequence,
                    grades: vec![(clip.id, grade)],
                });
            }
        }
        asked
    }

    /// The tools for `clip`'s grade: the switcher, the layer dropdown and
    /// the Simple / Advanced switch, then the tool shown.
    pub fn tools_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        sequence: SequenceId,
        clip: &project::Clip,
        media: Option<&project::MediaRef>,
    ) -> Asked {
        let stored = &clip.grade;
        let mut grade = self.changing.value_for(&clip.id).unwrap_or(stored).clone();
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
        let working = self.balancing.as_ref().is_some_and(|(id, _)| *id == clip.id);
        let mut asked = Asked::default();
        let busy = match self.tool {
            Tool::Basic => {
                let basic = basic::ui(
                    ui,
                    theme,
                    &mut primary,
                    &mut self.auto_strength,
                    working,
                    &mut self.picking_white,
                );
                if basic.auto {
                    match media.and_then(|m| Some((m, m.info.video.as_ref()?))) {
                        Some((media, video)) if clip.is_file() => {
                            let look = engine::white_balance::Look {
                                path: media.path.clone(),
                                stream: clip.stream,
                                rotation: video.rotation,
                                from: clip.source_in,
                                to: clip.source_at(clip.end()),
                            };
                            match engine::white_balance::Balancing::start(look) {
                                Ok(work) => self.balancing = Some((clip.id, work)),
                                Err(err) => asked.status = Some(err.to_string()),
                            }
                        }
                        _ => {
                            asked.status =
                                Some("Auto white balance looks at a recording; this clip has none".into())
                        }
                    }
                }
                basic.busy
            }
            Tool::Wheels => wheels::ui(ui, theme, &mut primary, self.advanced),
            Tool::Curves => {
                let before = grade
                    .0
                    .get(self.layer)
                    .map(|l| l.curves.clone())
                    .unwrap_or_default();
                let mut bent = before.clone();
                let busy = curves::ui(ui, theme, &mut bent, &mut self.curve_channel);
                if bent != before {
                    grade.layer_mut(self.layer).curves = bent;
                }
                busy
            }
            Tool::Range => {
                let before = grade.0.get(self.layer).and_then(|l| l.range);
                let mut range = before;
                let busy = range::ui(ui, theme, &mut range, &mut self.picking_range);
                if range != before {
                    grade.layer_mut(self.layer).range = range;
                }
                busy
            }
            Tool::Mask => {
                let before = grade.0.get(self.layer).and_then(|l| l.mask);
                let mut mask = before;
                let busy = mask::ui(ui, theme, &mut mask);
                if mask != before {
                    grade.layer_mut(self.layer).mask = mask;
                }
                busy
            }
        };
        // A pick waits only while its tool is open.
        self.picking_white &= self.tool == Tool::Basic;
        self.picking_range &= self.tool == Tool::Range;
        let busy = busy || self.mask_held;
        // Auto's answer goes into the sliders, as a change of their own.
        if let Some((id, work)) = &self.balancing
            && let Some(answer) = work.answer()
        {
            let for_this = *id == clip.id;
            self.balancing = None;
            match answer {
                Ok(Some((temperature, tint))) if for_this => {
                    let strength = self.auto_strength.clamp(0.0, 1.0);
                    primary.temperature = (f64::from(temperature) * strength * 1_000_000.0).round() as i32;
                    primary.tint = (f64::from(tint) * strength * 1_000_000.0).round() as i32;
                    asked.status = Some(
                        "White balance set from the clip: fine-tune it with Temperature and Tint".into(),
                    );
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    asked.status = Some(
                        "Too little in this clip is neutral to tell the light's colour: it is left as it is"
                            .into(),
                    )
                }
                Err(err) => asked.status = Some(err.to_string()),
            }
            ui.ctx().request_repaint();
        } else if self.balancing.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if primary != before {
            grade.layer_mut(self.layer).primary = primary;
        }
        if grade == *stored {
            self.changing.clear();
        } else if busy {
            self.changing.set(clip.id, grade);
        } else {
            self.changing.clear();
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
        ui.add_space(4.0);
        ui.toggle_value(&mut self.before, "Before")
            .on_hover_text("Shows the picture without its grade, to compare; click again for after");
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
    fn before_shows_the_picture_without_its_grade() {
        let p = project_with_two_files();
        let first = p.active().expect("a timeline").video_tracks[0].clips[0].id;
        let mut warm = Grade::default();
        warm.layer_mut(0).primary.temperature = 400_000;
        let p = Command::SetGrades {
            sequence: p.active_sequence,
            grades: vec![(first, warm)],
        }
        .apply(&p)
        .expect("graded");
        let mut page = ColorPage::default();
        assert!(page.shown(&p, Some(first)).is_none(), "as it is");
        page.before = true;
        let before = page.shown(&p, Some(first)).expect("shown before");
        assert!(
            !before.active().expect("a timeline").video_tracks[0].clips[0]
                .grade
                .is_graded()
        );
    }

    #[test]
    fn picking_a_warm_grey_cools_the_clip_until_it_is_neutral() {
        let p = project_with_two_files();
        let clip = p.active().expect("a timeline").video_tracks[0].clips[0].clone();
        // A grey lit warm: more red than blue.
        let warm = [0.6f32, 0.5, 0.4].map(|light| light.powf(1.0 / 2.4));
        let sampled = engine::scope::Sampled {
            width: 4,
            height: 4,
            rgb: vec![warm; 16],
        };
        let mut page = ColorPage {
            picking_white: true,
            ..ColorPage::default()
        };
        let asked = page.pick_white(p.active_sequence, &clip, &sampled, (0.5, 0.5));
        assert!(!page.picking_white, "one pick");
        let Some(Command::SetGrades { grades, .. }) = asked.commands.first() else {
            panic!("a grade set");
        };
        let primary = grades[0].1.0[0].primary;
        let right = (0.4f32 / 0.6).log2();
        assert!(
            (primary.temperature as f32 / 1_000_000.0 - right).abs() < 0.02,
            "{}",
            primary.temperature
        );
    }

    #[test]
    fn picking_white_on_values_at_the_ends_of_the_numbers_neither_wraps_nor_panics() {
        let p = project_with_two_files();
        let mut clip = p.active().expect("a timeline").video_tracks[0].clips[0].clone();
        // Values no slider makes, as a file or a paste could hold them.
        let primary = &mut clip.grade.layer_mut(0).primary;
        primary.temperature = i32::MIN;
        primary.tint = i32::MAX;
        let warm = [0.6f32, 0.5, 0.4].map(|light| light.powf(1.0 / 2.4));
        let sampled = engine::scope::Sampled {
            width: 4,
            height: 4,
            rgb: vec![warm; 16],
        };
        let mut page = ColorPage::default();
        let asked = page.pick_white(p.active_sequence, &clip, &sampled, (0.5, 0.5));
        let Some(Command::SetGrades { grades, .. }) = asked.commands.first() else {
            panic!("a grade set");
        };
        let primary = grades[0].1.0[0].primary;
        assert_eq!(primary.temperature, -1_000_000, "cooled further: held at the end");
        assert_eq!(primary.tint, 1_000_000, "held at the other end");
    }

    #[test]
    fn picking_a_colour_puts_the_layers_range_round_it() {
        let p = project_with_two_files();
        let clip = p.active().expect("a timeline").video_tracks[0].clips[0].clone();
        let skin = [0.6f32, 0.35, 0.25];
        let sampled = engine::scope::Sampled {
            width: 4,
            height: 4,
            rgb: vec![skin.map(|light| light.powf(1.0 / 2.4)); 16],
        };
        let mut page = ColorPage {
            tool: Tool::Range,
            picking_range: true,
            ..ColorPage::default()
        };
        let asked = page.pick_range(p.active_sequence, &clip, &sampled, (0.5, 0.5));
        assert!(!page.picking_range, "one pick");
        let Some(Command::SetGrades { grades, .. }) = asked.commands.first() else {
            panic!("a grade set");
        };
        let range = grades[0].1.0[0].range.expect("a range");
        let (hue, _, _) = range::hsl(skin);
        assert!((f64::from(range.hue) / 1_000_000.0 - hue).abs() < 0.001);
    }

    #[test]
    fn dragging_the_mask_is_shown_at_once_and_one_undo_step_when_let_go() {
        let p = project_with_two_files();
        let clip = p.active().expect("a timeline").video_tracks[0].clips[0].clone();
        let mut page = ColorPage {
            tool: Tool::Mask,
            ..ColorPage::default()
        };
        assert_eq!(page.mask_shown(&clip), None, "no mask yet");
        let moved = project::Mask {
            x: 300_000,
            ..project::Mask::MIDDLE
        };
        let held = page.drag_mask(p.active_sequence, &clip, moved, false);
        assert!(held.commands.is_empty(), "nothing kept while held");
        assert_eq!(page.mask_shown(&clip), Some(moved), "shown at once");
        let shown = page.shown(&p, Some(clip.id)).expect("shown");
        let seq = shown.active().expect("a timeline");
        assert_eq!(seq.video_tracks[0].clips[0].grade.0[0].mask, Some(moved));
        let done = page.drag_mask(p.active_sequence, &clip, moved, true);
        assert_eq!(done.commands.len(), 1, "one undo step");
        assert!(page.shown(&p, Some(clip.id)).is_none());
    }

    #[test]
    fn a_drag_left_when_the_playhead_moves_to_the_next_clip_is_kept_on_its_own_clip() {
        let p = project_with_two_files();
        let seq = p.active().expect("a timeline");
        let (a, b) = (
            seq.video_tracks[0].clips[0].clone(),
            seq.video_tracks[0].clips[1].clone(),
        );
        let mut page = ColorPage {
            tool: Tool::Mask,
            ..ColorPage::default()
        };
        let moved = project::Mask {
            x: 300_000,
            ..project::Mask::MIDDLE
        };
        // Dragging on A, the clip at the playhead.
        let held = page.drag_mask(p.active_sequence, &a, moved, false);
        assert!(held.commands.is_empty(), "nothing kept while held");
        assert!(
            page.finish_left_behind(seq, Some(a.id)).commands.is_empty(),
            "still A"
        );
        assert_eq!(page.mask_shown(&a), Some(moved));
        // The playhead crosses into B mid-drag: A's grade is one undo step.
        let left = page.finish_left_behind(seq, Some(b.id));
        let [Command::SetGrades { grades, .. }] = left.commands.as_slice() else {
            panic!("one grade set, got {:?}", left.commands);
        };
        assert_eq!(grades.len(), 1);
        assert_eq!(grades[0].0, a.id, "on the clip it was dragged on");
        assert_eq!(grades[0].1.0[0].mask, Some(moved));
        assert_eq!(page.mask_shown(&b), None, "not carried to B");
        assert!(page.shown(&p, Some(b.id)).is_none(), "nothing left waiting");
        assert!(
            page.finish_left_behind(seq, Some(b.id)).commands.is_empty(),
            "only once"
        );
        // Past the last clip nothing is graded: a drag left there is kept too.
        page.drag_mask(p.active_sequence, &b, moved, false);
        let left = page.finish_left_behind(seq, None);
        assert_eq!(left.commands.len(), 1, "kept on B");
        assert!(page.shown(&p, None).is_none());
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
