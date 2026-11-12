//! A file's labels, lists and note, as the media drawer's right-click menu
//! and the Library page (its cards' menus and its detail panel) offer them.
//! Built once here, so a file is organised the same way wherever it shows.
//! Each change is one command, so one undo step; offering a label in every
//! project is not an edit (`Organised::Everywhere`).

use project::{
    Command, GlobalLabel, GlobalLabels, LabelColor, Library, MediaId, MediaLabel, MediaLabelId, Project,
};

/// What a file's Labels, Lists and Note menus asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Organised {
    /// A change to the project: one undo step.
    Edit(Box<Command>),
    /// A change to the labels offered in every project (not an edit).
    Everywhere(Everywhere),
}

/// A change to the labels offered in every project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Everywhere {
    /// Offer this project label in every project.
    Offer(String, LabelColor),
    /// Stop offering it; projects that use it keep their own copy.
    StopOffering(String),
}

/// What is being typed in the menus: a new label's or list's name, and
/// the note of the file being written.
#[derive(Debug, Default)]
pub struct Organiser {
    name_draft: String,
    note_draft: String,
    editing: Option<MediaId>,
}

impl Organiser {
    /// Labels, Lists and Note as three submenus of a right-click menu.
    pub fn menus(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        global: &GlobalLabels,
        media: MediaId,
    ) -> Option<Organised> {
        let mut asked = None;
        ui.menu_button("Labels", |ui| {
            asked = self.labels_menu(ui, project, global, media);
        });
        ui.menu_button("Lists", |ui| {
            asked = asked.take().or(self.lists_menu(ui, project, media));
        });
        ui.menu_button("Note", |ui| {
            asked = asked.take().or(self.note_menu(ui, project, media));
        });
        asked
    }

    /// The Labels menu's contents: every label to tick (the project's, then
    /// those offered everywhere it hasn't got yet, marked "everywhere"), a
    /// field for a new one, and "Every project".
    pub fn labels_menu(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        global: &GlobalLabels,
        media: MediaId,
    ) -> Option<Organised> {
        let library = &project.library;
        let mut asked = None;
        let choices = label_choices(library, global);
        for choice in &choices {
            let (name, color, everywhere) = choice.shown();
            let mut on = choice
                .project_label()
                .is_some_and(|l| library.has_label(media, l));
            let text = label_text(ui, name, color, everywhere);
            if ui.checkbox(&mut on, text).changed() {
                asked = Some(Organised::Edit(Box::new(choice.command(media, on))));
            }
        }
        if !choices.is_empty() {
            ui.separator();
        }
        if let Some(name) = name_field(ui, &mut self.name_draft, "New label") {
            asked = Some(Organised::Edit(Box::new(new_label(
                library,
                global,
                name,
                vec![media],
            ))));
        }
        if let Some(change) = everywhere_menu(ui, library, global) {
            asked = Some(Organised::Everywhere(change));
        }
        if asked.is_some() {
            self.name_draft.clear();
        }
        asked
    }

    /// The Lists menu's contents: every list to tick, and a field for a new one.
    pub fn lists_menu(&mut self, ui: &mut egui::Ui, project: &Project, media: MediaId) -> Option<Organised> {
        let library = &project.library;
        let mut asked = None;
        for list in &library.lists {
            let mut member = list.media.contains(&media);
            if ui.checkbox(&mut member, &list.name).changed() {
                asked = Some(Organised::Edit(Box::new(Command::SetListMember {
                    list: list.id,
                    media: vec![media],
                    member,
                })));
            }
        }
        if !library.lists.is_empty() {
            ui.separator();
        }
        if let Some(name) = name_field(ui, &mut self.name_draft, "New list") {
            asked = Some(Organised::Edit(Box::new(Command::AddMediaList {
                name,
                media: vec![media],
            })));
            self.name_draft.clear();
        }
        asked
    }

    /// The Note menu's contents: the note to write, and "Keep note".
    pub fn note_menu(&mut self, ui: &mut egui::Ui, project: &Project, media: MediaId) -> Option<Organised> {
        if self.editing != Some(media) {
            self.editing = Some(media);
            self.note_draft = project.library.note(media).to_owned();
        }
        ui.add(
            egui::TextEdit::multiline(&mut self.note_draft)
                .hint_text("What is in it, what to use it for")
                .desired_rows(4)
                .desired_width(240.0),
        );
        if ui.button("Keep note").clicked() {
            self.editing = None;
            return Some(Organised::Edit(Box::new(Command::SetMediaNote {
                media,
                note: std::mem::take(&mut self.note_draft),
            })));
        }
        None
    }
}

/// A label's name in its colour, with a quiet "everywhere" after it when it
/// is offered in every project.
pub fn label_text(ui: &egui::Ui, name: &str, color: LabelColor, everywhere: bool) -> egui::text::LayoutJob {
    let mut text = egui::text::LayoutJob::default();
    text.append(
        name,
        0.0,
        egui::TextFormat::simple(
            egui::FontId::proportional(13.0),
            crate::timeline_panel::label_colour(color),
        ),
    );
    if everywhere {
        text.append(
            "everywhere",
            8.0,
            egui::TextFormat::simple(egui::FontId::proportional(11.0), ui.visuals().weak_text_color()),
        );
    }
    text
}

/// A new label called `name` on `media` (none for an empty label): a name
/// offered everywhere keeps its colour, any other takes the next free one.
pub fn new_label(library: &Library, global: &GlobalLabels, name: String, media: Vec<MediaId>) -> Command {
    let color = global
        .find(&name)
        .map_or_else(|| library.next_color(), |g| g.color);
    Command::AddMediaLabel {
        label: MediaLabel {
            id: MediaLabelId(0),
            name,
            color,
        },
        media,
    }
}

/// One line of the Labels menu: a label of the project (marked when it is
/// offered everywhere too), or one offered everywhere that this project
/// doesn't have yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelChoice<'a> {
    Project { label: &'a MediaLabel, everywhere: bool },
    Everywhere(&'a GlobalLabel),
}

impl LabelChoice<'_> {
    /// Name, colour and whether it is offered everywhere.
    pub fn shown(&self) -> (&str, LabelColor, bool) {
        match self {
            LabelChoice::Project { label, everywhere } => (&label.name, label.color, *everywhere),
            LabelChoice::Everywhere(g) => (&g.name, g.color, true),
        }
    }

    pub fn project_label(&self) -> Option<MediaLabelId> {
        match self {
            LabelChoice::Project { label, .. } => Some(label.id),
            LabelChoice::Everywhere(_) => None,
        }
    }

    /// Ticking (`on`) or unticking it for `media`. A label offered
    /// everywhere is copied into the project the first time it is used.
    pub fn command(&self, media: MediaId, on: bool) -> Command {
        match self {
            LabelChoice::Project { label, .. } => Command::SetMediaLabel {
                media: vec![media],
                label: label.id,
                on,
            },
            LabelChoice::Everywhere(g) => Command::AddMediaLabel {
                label: MediaLabel {
                    id: MediaLabelId(0),
                    name: g.name.clone(),
                    color: g.color,
                },
                media: vec![media],
            },
        }
    }
}

/// The Labels menu's lines: the project's labels, then those offered
/// everywhere that it doesn't have (by name, whatever the capitals).
pub fn label_choices<'a>(library: &'a Library, global: &'a GlobalLabels) -> Vec<LabelChoice<'a>> {
    let mut choices: Vec<LabelChoice<'a>> = library
        .labels
        .iter()
        .map(|label| LabelChoice::Project {
            label,
            everywhere: global.find(&label.name).is_some(),
        })
        .collect();
    let has = |name: &str| {
        library
            .labels
            .iter()
            .any(|l| l.name.to_lowercase() == name.to_lowercase())
    };
    choices.extend(
        global
            .labels
            .iter()
            .filter(|g| !has(&g.name))
            .map(LabelChoice::Everywhere),
    );
    choices
}

/// "Every project": offer a project label everywhere, or stop offering one.
fn everywhere_menu(ui: &mut egui::Ui, library: &Library, global: &GlobalLabels) -> Option<Everywhere> {
    let offerable: Vec<&MediaLabel> = library
        .labels
        .iter()
        .filter(|l| global.find(&l.name).is_none())
        .collect();
    if offerable.is_empty() && global.labels.is_empty() {
        return None;
    }
    let mut asked = None;
    ui.menu_button("Every project", |ui| {
        for label in offerable {
            if ui
                .button(format!("Use \"{}\" in every project", label.name))
                .on_hover_text("Offered in every project's Labels menu, with this colour")
                .clicked()
            {
                asked = Some(Everywhere::Offer(label.name.clone(), label.color));
            }
        }
        for g in &global.labels {
            if ui
                .button(format!("Stop offering \"{}\" everywhere", g.name))
                .on_hover_text("Projects that use it keep it as their own label")
                .clicked()
            {
                asked = Some(Everywhere::StopOffering(g.name.clone()));
            }
        }
    });
    asked
}

/// A field for a new name and its button; the name when it was asked for
/// (Enter or the button).
pub fn name_field(ui: &mut egui::Ui, draft: &mut String, hint: &str) -> Option<String> {
    let mut asked = false;
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(draft)
                .hint_text(hint)
                .desired_width(130.0),
        );
        asked = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        asked |= ui
            .add_enabled(!draft.trim().is_empty(), egui::Button::new("Add"))
            .clicked();
    });
    (asked && !draft.trim().is_empty()).then(|| draft.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaInfo, MediaKind, MediaRef, Time};

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
    fn labels_offered_everywhere_join_the_project_s_own() {
        let mut p = Project::new_automatic("T");
        p.media.insert(MediaId(1), media(1, "/r/a.mkv"));
        let p = Command::AddMediaLabel {
            label: MediaLabel {
                id: MediaLabelId(0),
                name: "Drone".into(),
                color: LabelColor::Green,
            },
            media: vec![],
        }
        .apply(&p)
        .unwrap();
        let mut global = GlobalLabels::default();
        global.add("drone", LabelColor::Red).unwrap();
        global.add("Interview", LabelColor::Orange).unwrap();
        let choices = label_choices(&p.library, &global);
        assert_eq!(choices.len(), 2, "a name the project has shows once");
        assert!(
            matches!(choices[0], LabelChoice::Project { label, everywhere: true } if label.name == "Drone"),
            "the project's own, marked as offered everywhere"
        );
        let LabelChoice::Everywhere(interview) = choices[1] else {
            panic!("the one the project hasn't got yet")
        };
        assert_eq!(interview.name, "Interview");
        // Ticking it copies it into the project, so the project file needs
        // nothing from outside.
        let p = choices[1].command(MediaId(1), true).apply(&p).unwrap();
        let on = p.library.labels_of(MediaId(1));
        assert_eq!(on[0].name, "Interview");
        assert_eq!(on[0].color, LabelColor::Orange);
        let global = GlobalLabels::default();
        let choices = label_choices(&p.library, &global);
        assert!(
            matches!(
                choices[1],
                LabelChoice::Project {
                    everywhere: false,
                    ..
                }
            ),
            "taken out of the set, the project keeps its copy"
        );
    }

    #[test]
    fn a_new_label_keeps_the_colour_it_has_everywhere() {
        let p = Project::new_automatic("T");
        let mut global = GlobalLabels::default();
        global.add("Drone", LabelColor::Purple).unwrap();
        let Command::AddMediaLabel { label, media } = new_label(&p.library, &global, "drone".into(), vec![])
        else {
            panic!("a new label")
        };
        assert_eq!(label.color, LabelColor::Purple);
        assert!(media.is_empty(), "an empty label, for the Library's \"+\"");
        let Command::AddMediaLabel { label, .. } = new_label(&p.library, &global, "Music".into(), vec![])
        else {
            panic!("a new label")
        };
        assert_eq!(label.color, p.library.next_color());
    }
}
