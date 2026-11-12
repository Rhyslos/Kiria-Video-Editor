//! The Templates window: save the region (or the whole timeline) as a
//! template, drop a saved one into the timeline at its start, the edit
//! point or its end, and rename or delete one. Templates are small files in
//! the app's data folder, shared by every project; everything that touches
//! them runs on a worker.

use egui::RichText;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::templates::{self, Listed};
use project::{Project, Region, Template, Time};
use std::path::Path;
use ui_kit::Theme;

/// Where on the timeline a template goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// Before everything: an intro.
    Start,
    /// At the edit needle (or the playhead, when Settings merges them).
    EditPoint,
    /// After everything: an outro.
    End,
}

impl Spot {
    const ALL: [Spot; 3] = [Spot::Start, Spot::EditPoint, Spot::End];

    fn button(self) -> &'static str {
        match self {
            Spot::Start => "At the start",
            Spot::EditPoint => "At the edit point",
            Spot::End => "At the end",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Spot::Start => "As an intro: before everything, which moves later to make room.",
            Spot::EditPoint => {
                "At the edit needle (the playhead, when Settings merges them): everything after it moves later to make room."
            }
            Spot::End => "As an outro: after the last clip.",
        }
    }

    /// The time on a timeline this spot is, given its edit point and length.
    pub fn time(self, edit_point: Time, length: Time) -> Time {
        match self {
            Spot::Start => Time::ZERO,
            Spot::EditPoint => edit_point,
            Spot::End => length,
        }
    }

    /// How the status line says where it went.
    pub fn said(self) -> &'static str {
        match self {
            Spot::Start => "at the start",
            Spot::EditPoint => "at the edit point",
            Spot::End => "at the end",
        }
    }
}

/// What the window asked for this frame.
pub enum Asked {
    Nothing,
    /// Something was done; say so in the status line.
    Said(String),
    /// Put this template into the timeline.
    Insert {
        name: String,
        template: Box<Template>,
        spot: Spot,
    },
    Failed(diag::AppError),
}

/// What a worker finished.
enum Done {
    Listed(Vec<Listed>),
    Saved(String),
    Renamed(String, String),
    Deleted(String),
}

/// A question the window is waiting on.
enum Question {
    Rename {
        template: String,
        typed: String,
        focused: bool,
    },
    Delete(String),
}

#[derive(Default)]
pub struct TemplatesWindow {
    pub open: bool,
    /// The name typed for a new template.
    name: String,
    /// Put the cursor in the name field (the window was opened to save one).
    focus_name: bool,
    /// The templates, read when the window opens and after each change.
    listed: Option<Vec<Listed>>,
    /// The list no longer matches the folder: read it again.
    stale: bool,
    /// The one piece of disk work under way.
    working: Option<JobHandle<Done>>,
    question: Option<Question>,
    /// What went wrong with the last thing asked here (what, and how to
    /// fix it), shown in the window until something else is asked: the
    /// page's own error strip may be under the window.
    problem: Option<(String, String)>,
}

impl TemplatesWindow {
    /// Opens the window with a fresh list; `to_save` puts the cursor in the name field.
    pub fn show(&mut self, to_save: bool) {
        self.open = true;
        self.stale = true;
        self.focus_name = to_save;
        self.problem = None;
    }

    /// Says inside the window that something it asked for failed (the
    /// page's error strip, where the error also goes, may be under it).
    pub fn failed(&mut self, err: &diag::AppError) {
        self.problem = Some((err.what().to_owned(), about(err)));
    }

    fn run(
        &mut self,
        jobs: &JobSystem,
        ctx: &egui::Context,
        name: &'static str,
        work: impl FnOnce() -> diag::Result<Done> + Send + 'static,
    ) {
        let ctx = ctx.clone();
        self.working = Some(
            jobs.submit(PoolKind::Analysis, Priority::Interactive, name, move |_| {
                let done = work();
                ctx.request_repaint();
                done
            }),
        );
    }

    /// The window. `project` is the project as it is now; `dir` is where
    /// templates are kept.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        theme: &Theme,
        jobs: &JobSystem,
        project: &Project,
        dir: &Path,
    ) -> Asked {
        let mut asked = Asked::Nothing;
        if let Some(result) = self.working.as_ref().and_then(JobHandle::try_result) {
            self.working = None;
            // After anything but a reading of the list, the folder may have changed.
            self.stale |= !matches!(result, Ok(Done::Listed(_)));
            match result {
                Ok(Done::Listed(list)) => self.listed = Some(list),
                Ok(Done::Saved(name)) => {
                    self.name.clear();
                    asked = Asked::Said(format!("Saved the template \"{name}\"."));
                }
                Ok(Done::Renamed(from, to)) => {
                    asked = Asked::Said(format!("Renamed the template \"{from}\" to \"{to}\"."));
                }
                Ok(Done::Deleted(name)) => asked = Asked::Said(format!("Deleted the template \"{name}\".")),
                Err(err) => {
                    self.failed(&err);
                    asked = Asked::Failed(err);
                }
            }
        }
        if self.working.is_none() && (self.stale || self.listed.is_none()) {
            self.stale = false;
            let dir = dir.to_path_buf();
            self.run(jobs, ctx, "list templates", move || {
                Ok(Done::Listed(templates::list(&dir)))
            });
        }

        let seq = project.active();
        let region = seq.and_then(|s| s.region);
        let busy = self.working.is_some();
        let mut open = self.open;
        let mut save = false;
        let mut insert = None;
        let listed = self.listed.take();
        egui::Window::new("Templates")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(WIDTH)
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text("Name, like \"Channel intro\"")
                            .desired_width(250.0),
                    );
                    if std::mem::take(&mut self.focus_name) {
                        field.request_focus();
                    }
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let button = ui
                        .add_enabled_ui(!busy, |ui| ui_kit::accent_button(ui, theme, "Save as a template"))
                        .inner;
                    save = !busy && (button.clicked() || enter);
                });
                ui.label(RichText::new(what_is_saved(seq.map(|s| s.length()), region)).color(theme.muted));
                if let Some((what, fix)) = &self.problem {
                    ui.horizontal_wrapped(|ui| {
                        ui_kit::status_dot(ui, ui_kit::status::ERROR);
                        ui.label(RichText::new(what).family(ui_kit::fonts::semibold()));
                        ui.label(RichText::new(fix).color(theme.text_secondary));
                    });
                }
                ui.add_space(8.0);
                ui.separator();
                match &listed {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Reading the templates…").color(theme.muted));
                        });
                    }
                    Some(list) if list.is_empty() => {
                        ui.label(
                            RichText::new(
                                "No templates yet. Build an intro or outro on the timeline, then save it here: it can be dropped into any project.",
                            )
                            .color(theme.muted),
                        );
                    }
                    Some(list) => {
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for item in list {
                                match row(ui, theme, item, busy) {
                                    Some(RowAsk::Insert(spot)) => insert = Some((item, spot)),
                                    Some(RowAsk::Rename) => {
                                        self.question = Some(Question::Rename {
                                            template: item.name.clone(),
                                            typed: item.name.clone(),
                                            focused: false,
                                        });
                                    }
                                    Some(RowAsk::Delete) => {
                                        self.question = Some(Question::Delete(item.name.clone()));
                                    }
                                    None => {}
                                }
                                ui.separator();
                            }
                        });
                    }
                }
            });
        if let Some((item, spot)) = insert
            && let Ok(contents) = &item.contents
        {
            self.problem = None;
            asked = Asked::Insert {
                name: item.name.clone(),
                template: Box::new(contents.template.clone()),
                spot,
            };
        }
        self.listed = listed;
        if save && let Some(seq) = seq {
            self.problem = None;
            // The stretch is cut out of this snapshot and written on a worker.
            let (p, sequence, dir, name) = (project.clone(), seq.id, dir.to_path_buf(), self.name.clone());
            self.run(jobs, ctx, "save template", move || {
                let template = Template::capture(&p, sequence, region)?;
                templates::save(&template, &dir, &name).map(Done::Saved)
            });
        }
        self.answer_question(ctx, theme, jobs, dir);
        if !open {
            self.question = None;
        }
        self.open = open;
        asked
    }

    /// The rename and delete questions, and the work they start.
    fn answer_question(&mut self, ctx: &egui::Context, theme: &Theme, jobs: &JobSystem, dir: &Path) {
        let Some(question) = &mut self.question else {
            return;
        };
        let (mut confirmed, mut closed) = (false, false);
        let modal = ui_kit::dialog(ctx, theme, "template-question", |ui| {
            ui.set_width(360.0);
            let title = match question {
                Question::Rename { .. } => "Rename template",
                Question::Delete(_) => "Delete this template?",
            };
            ui.label(RichText::new(title).family(ui_kit::fonts::semibold()).size(16.0));
            ui.add_space(8.0);
            let mut enter = false;
            let yes = match question {
                Question::Rename { typed, focused, .. } => {
                    let field = ui.add(egui::TextEdit::singleline(typed).desired_width(f32::INFINITY));
                    if !std::mem::replace(focused, true) {
                        field.request_focus();
                    }
                    enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.label(
                        RichText::new("Projects it was dropped into are not changed.")
                            .size(11.5)
                            .color(theme.muted),
                    );
                    "Rename"
                }
                Question::Delete(name) => {
                    ui.label(
                        RichText::new(format!(
                            "\"{name}\" is removed for good. Projects it was dropped into keep their clips."
                        ))
                        .color(theme.text_secondary),
                    );
                    "Delete"
                }
            };
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui_kit::accent_button(ui, theme, yes).clicked() || enter {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    closed = true;
                }
            });
        });
        if confirmed && self.working.is_none() {
            self.problem = None;
            let dir = dir.to_path_buf();
            match self.question.take() {
                Some(Question::Rename { template, typed, .. }) => {
                    self.run(jobs, ctx, "rename template", move || {
                        templates::rename(&dir, &template, &typed).map(|to| Done::Renamed(template, to))
                    });
                }
                Some(Question::Delete(name)) => {
                    self.run(jobs, ctx, "delete template", move || {
                        templates::delete(&dir, &name).map(|()| Done::Deleted(name))
                    });
                }
                None => {}
            }
        } else if closed || modal.should_close() {
            self.question = None;
        }
    }
}

/// The window's width: room for a name, its details and the three places to insert.
const WIDTH: f32 = 440.0;

/// What a row was asked to do.
enum RowAsk {
    Insert(Spot),
    Rename,
    Delete,
}

/// One template: its name, length and clips (or why it can't be used), the
/// three places it can go, and rename and delete.
fn row(ui: &mut egui::Ui, theme: &Theme, item: &Listed, busy: bool) -> Option<RowAsk> {
    let mut asked = None;
    ui.horizontal(|ui| {
        match &item.contents {
            Ok(contents) if !contents.missing_media.is_empty() => {
                ui_kit::status_dot(ui, ui_kit::status::WARNING);
            }
            Ok(_) => {}
            Err(_) => ui_kit::status_dot(ui, ui_kit::status::ERROR),
        }
        ui.label(RichText::new(&item.name).family(ui_kit::fonts::semibold()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if ui.button("Delete…").clicked() {
                    asked = Some(RowAsk::Delete);
                }
                if ui.button("Rename…").clicked() {
                    asked = Some(RowAsk::Rename);
                }
            });
        });
    });
    match &item.contents {
        Ok(contents) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(details(&contents.template)).color(theme.muted));
                if !contents.missing_media.is_empty() {
                    let paths: Vec<String> = contents
                        .missing_media
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect();
                    ui.label(RichText::new(missing_text(paths.len())).color(ui_kit::status::WARNING))
                        .on_hover_text(format!(
                            "Not where the template says:\n{}\n\nIt can still be dropped in: its clips show as missing media until the files are found again.",
                            paths.join("\n")
                        ));
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Insert").color(theme.text_secondary));
                ui.add_enabled_ui(!busy, |ui| {
                    for spot in Spot::ALL {
                        if ui.button(spot.button()).on_hover_text(spot.tip()).clicked() {
                            asked = Some(RowAsk::Insert(spot));
                        }
                    }
                });
            });
        }
        Err(err) => {
            ui.label(RichText::new(err.what()).color(theme.muted))
                .on_hover_text(about(err));
        }
    }
    asked
}

/// Why an error happened and what to do about it, as one piece of text.
fn about(err: &diag::AppError) -> String {
    format!("{} {}", err.why_text().unwrap_or_default(), err.fix_text())
        .trim()
        .to_owned()
}

/// "7.5 s · 5 clips on 3 tracks".
fn details(template: &Template) -> String {
    let clips = template.clip_count();
    let tracks = template.tracks().filter(|t| !t.clips.is_empty()).count();
    format!(
        "{} · {clips} clip{} on {tracks} track{}",
        ui_kit::time_text::length(template.length.as_secs_f64()),
        if clips == 1 { "" } else { "s" },
        if tracks == 1 { "" } else { "s" },
    )
}

fn missing_text(files: usize) -> String {
    format!(
        "· {files} file{} can't be found",
        if files == 1 { "" } else { "s" }
    )
}

/// The line under the name field: which stretch "Save as a template" takes.
fn what_is_saved(length: Option<Time>, region: Option<Region>) -> String {
    let long = |t: Time| ui_kit::time_text::length(t.as_secs_f64());
    match (region, length) {
        (Some(region), _) => format!(
            "Saves the clips inside the region ({} long). Clear the region to save the whole timeline.",
            long(region.length())
        ),
        (None, Some(length)) if length > Time::ZERO => format!(
            "Saves the whole timeline ({} long). Mark a region (Shift-drag in the ruler) to save only that part.",
            long(length)
        ),
        _ => "The timeline is empty: there is nothing to save yet.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_are_the_start_the_edit_point_and_the_end() {
        let (edit_point, length) = (Time::seconds(12), Time::seconds(80));
        assert_eq!(Spot::Start.time(edit_point, length), Time::ZERO);
        assert_eq!(Spot::EditPoint.time(edit_point, length), Time::seconds(12));
        assert_eq!(Spot::End.time(edit_point, length), Time::seconds(80));
    }

    #[test]
    fn the_window_says_which_stretch_it_saves() {
        let region = Region::between(Time::seconds(5), Time::from_ratio(25, 2).unwrap());
        assert!(what_is_saved(Some(Time::seconds(80)), region).contains("the region (7.5 s long)"));
        assert!(what_is_saved(Some(Time::seconds(80)), None).contains("whole timeline (1:20 long)"));
        assert!(what_is_saved(Some(Time::ZERO), None).contains("nothing to save"));
        assert!(what_is_saved(None, None).contains("nothing to save"));
    }

    #[test]
    fn rows_count_clips_and_files_in_plain_words() {
        use project::{Clip, ClipId, Command, Fingerprint, MediaId, MediaKind, MediaRef};
        let p = Project::new_automatic("Channel");
        let p = Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: "sounds/jingle.flac".into(),
                fingerprint: Fingerprint {
                    size: 900,
                    sample_hash: "jingle".into(),
                },
                kind: MediaKind::Audio,
                duration: Time::seconds(60),
                info: Default::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let (seq, media) = (p.active().unwrap(), *p.media.keys().next().unwrap());
        let clip = |start: i64| Clip {
            id: ClipId(0),
            media,
            stream: 0,
            start: Time::seconds(start),
            source_in: Time::ZERO,
            duration: Time::from_ratio(37, 10).unwrap(),
            link: None,
            speed: Default::default(),
            look: Default::default(),
            sound: Default::default(),
        };
        let mut placed = p.clone();
        for start in [0, 4] {
            placed = Command::PlaceClip {
                sequence: seq.id,
                track: seq.audio_tracks[0].id,
                clip: clip(start),
            }
            .apply(&placed)
            .unwrap();
        }
        let whole = Template::capture(&placed, seq.id, None).unwrap();
        assert_eq!(details(&whole), "7.7 s · 2 clips on 1 track");
        let part = Template::capture(&placed, seq.id, Region::between(Time::ZERO, Time::seconds(2))).unwrap();
        assert_eq!(details(&part), "2.0 s · 1 clip on 1 track");
        assert_eq!(missing_text(1), "· 1 file can't be found");
        assert_eq!(missing_text(2), "· 2 files can't be found");
    }
}
