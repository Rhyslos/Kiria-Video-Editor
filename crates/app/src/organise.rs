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

/// What the menus keep between frames: a new label's or list's name being
/// asked for (a small dialog, so the field keeps the keys: a field inside a
/// menu loses them when the menu closes), and the note being written.
#[derive(Debug, Default)]
pub struct Organiser {
    naming: Option<Naming>,
    note_draft: String,
    editing: Option<MediaId>,
}

/// A name being asked for, and for which files.
#[derive(Debug)]
struct Naming {
    what: NewName,
    media: Vec<MediaId>,
    ask: ui_kit::dialog::NameAsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewName {
    Label,
    List,
}

/// Whether every one, some or none of a set of files has something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    All,
    Some,
    None,
}

impl Share {
    pub fn of(media: &[MediaId], has: impl Fn(MediaId) -> bool) -> Share {
        let count = media.iter().filter(|m| has(**m)).count();
        match count {
            0 => Share::None,
            n if n == media.len() => Share::All,
            _ => Share::Some,
        }
    }

    /// What a click on its tick does: every file gets it, unless every one
    /// has it already, when every one loses it.
    pub fn clicked_on(self) -> bool {
        self != Share::All
    }
}

impl Organiser {
    /// Labels, Lists and Note as three submenus of a right-click menu, for
    /// one file or several (`media`; Note only for one).
    pub fn menus(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        global: &GlobalLabels,
        media: &[MediaId],
    ) -> Option<Organised> {
        let mut asked = None;
        ui.menu_button("Labels", |ui| {
            asked = self.labels_menu(ui, project, global, media);
        });
        ui.menu_button("Lists", |ui| {
            asked = asked.take().or(self.lists_menu(ui, project, media));
        });
        if let [one] = media {
            ui.menu_button("Note", |ui| {
                asked = asked.take().or(self.note_menu(ui, project, *one));
            });
        }
        asked
    }

    /// The Labels menu's contents: every label to tick (the project's, then
    /// those offered everywhere it hasn't got yet, marked "everywhere";
    /// half-ticked when some of the files have it), "New label…" (asks for
    /// the name in a dialog), and "Every project".
    pub fn labels_menu(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        global: &GlobalLabels,
        media: &[MediaId],
    ) -> Option<Organised> {
        let library = &project.library;
        let mut asked = None;
        let choices = label_choices(library, global);
        for choice in &choices {
            let (name, color, everywhere) = choice.shown();
            let share = match choice.project_label() {
                Some(l) => Share::of(media, |m| library.has_label(m, l)),
                None => Share::None,
            };
            let mut on = share == Share::All;
            let text = label_text(ui, name, color, everywhere);
            let tick = egui::Checkbox::new(&mut on, text).indeterminate(share == Share::Some);
            if ui.add(tick).clicked() {
                asked = Some(Organised::Edit(Box::new(
                    choice.command(media, share.clicked_on()),
                )));
            }
        }
        if !choices.is_empty() {
            ui.separator();
        }
        if ui.button("New label…").clicked() {
            self.ask_label(media.to_vec());
            ui.close();
        }
        if let Some(change) = everywhere_menu(ui, library, global) {
            asked = Some(Organised::Everywhere(change));
        }
        asked
    }

    /// The Lists menu's contents: every list to tick (half-ticked when some
    /// of the files are in it), and "New list…".
    pub fn lists_menu(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        media: &[MediaId],
    ) -> Option<Organised> {
        let library = &project.library;
        let mut asked = None;
        for list in &library.lists {
            let share = Share::of(media, |m| list.media.contains(&m));
            let mut member = share == Share::All;
            let tick = egui::Checkbox::new(&mut member, &list.name).indeterminate(share == Share::Some);
            if ui.add(tick).clicked() {
                asked = Some(Organised::Edit(Box::new(Command::SetListMember {
                    list: list.id,
                    media: media.to_vec(),
                    member: share.clicked_on(),
                })));
            }
        }
        if !library.lists.is_empty() {
            ui.separator();
        }
        if ui.button("New list…").clicked() {
            self.ask_list(media.to_vec());
            ui.close();
        }
        asked
    }

    /// Asks for a new label's name; it goes on `media` (none: an empty label).
    pub fn ask_label(&mut self, media: Vec<MediaId>) {
        let note = match media.len() {
            0 => "A label on no file yet: give it to files from their right-click menu.".to_owned(),
            1 => "It goes on this file.".to_owned(),
            n => format!("It goes on these {n} files."),
        };
        self.naming = Some(Naming {
            what: NewName::Label,
            media,
            ask: ui_kit::dialog::NameAsk::new("New label", "").note(note),
        });
    }

    /// Asks for a new list's name; `media` go in it (none: an empty list).
    pub fn ask_list(&mut self, media: Vec<MediaId>) {
        let note = match media.len() {
            0 => "A list of files picked by hand, in any order.".to_owned(),
            1 => "This file goes in it.".to_owned(),
            n => format!("These {n} files go in it."),
        };
        self.naming = Some(Naming {
            what: NewName::List,
            media,
            ask: ui_kit::dialog::NameAsk::new("New list", "").note(note),
        });
    }

    /// The name dialog, while one is asked for: call every frame. A name
    /// given is the command that makes the label or list (one undo step).
    pub fn dialog(
        &mut self,
        ctx: &egui::Context,
        theme: &ui_kit::Theme,
        project: &Project,
        global: &GlobalLabels,
    ) -> Option<Organised> {
        let naming = self.naming.as_mut()?;
        let answer = ui_kit::dialog::name_dialog(ctx, theme, "organise-name", &mut naming.ask)?;
        let naming = self.naming.take()?;
        let ui_kit::dialog::NameAnswer::Given(name) = answer else {
            return None;
        };
        let command = match naming.what {
            NewName::Label => new_label(&project.library, global, name, naming.media),
            NewName::List => Command::AddMediaList {
                name,
                media: naming.media,
            },
        };
        Some(Organised::Edit(Box::new(command)))
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

    /// Ticking (`on`) or unticking it for `media`, as one command. A label
    /// offered everywhere is copied into the project the first time it is used.
    pub fn command(&self, media: &[MediaId], on: bool) -> Command {
        match self {
            LabelChoice::Project { label, .. } => Command::SetMediaLabel {
                media: media.to_vec(),
                label: label.id,
                on,
            },
            LabelChoice::Everywhere(g) => Command::AddMediaLabel {
                label: MediaLabel {
                    id: MediaLabelId(0),
                    name: g.name.clone(),
                    color: g.color,
                },
                media: media.to_vec(),
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
        let p = choices[1].command(&[MediaId(1)], true).apply(&p).unwrap();
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

    #[test]
    fn a_tick_for_several_files_gives_it_to_all_unless_all_have_it() {
        let has = [MediaId(1), MediaId(3)];
        let share = |media: &[MediaId]| Share::of(media, |m| has.contains(&m));
        assert_eq!(share(&[MediaId(1), MediaId(3)]), Share::All);
        assert_eq!(share(&[MediaId(1), MediaId(2)]), Share::Some);
        assert_eq!(share(&[MediaId(2)]), Share::None);
        assert!(!Share::All.clicked_on(), "all have it: all lose it");
        assert!(Share::Some.clicked_on(), "some have it: all get it");
        assert!(Share::None.clicked_on());
    }

    #[test]
    fn a_new_label_named_in_the_dialog_goes_on_every_file_as_one_step() {
        let mut p = Project::new_automatic("T");
        for id in 1..=3 {
            p.media.insert(MediaId(id), media(id, &format!("/r/{id}.wav")));
        }
        let files = vec![MediaId(1), MediaId(2), MediaId(3)];
        let mut organiser = Organiser::default();
        organiser.ask_label(files.clone());
        assert!(organiser.naming.is_some());
        let ctx = egui::Context::default();
        ui_kit::setup(&ctx, ui_kit::ThemeName::Ash);
        let theme = ui_kit::ThemeName::Ash.theme();
        let global = GlobalLabels::default();
        let mut run = |events: Vec<egui::Event>| {
            let mut got = None;
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..egui::RawInput::default()
                },
                |ui| got = organiser.dialog(ui.ctx(), &theme, &p, &global),
            )
            .drop_without_applying_deltas();
            got
        };
        assert_eq!(run(Vec::new()), None);
        assert_eq!(run(vec![egui::Event::Text("B-roll".into())]), None);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let Some(Organised::Edit(command)) = run(vec![enter]) else {
            panic!("the label is made")
        };
        let after = command.apply(&p).unwrap();
        for file in files {
            assert_eq!(
                after.library.labels_of(file)[0].name,
                "B-roll",
                "on every file at once"
            );
        }
        assert!(organiser.naming.is_none());
    }
}
