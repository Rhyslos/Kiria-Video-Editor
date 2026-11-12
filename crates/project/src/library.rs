//! How a project's media is organised (format 13): labels on files (Drone,
//! Interview, B-roll…), custom lists ("Opening sequence", "Unused drone")
//! and a note per file.
//!
//! This is project data like any other: it changes only through
//! [`Command`](crate::Command)s, so every change is undoable, written to the
//! autosave journal and recovered after a crash. The `library` crate's
//! search index is built from it and is never the source of truth.

use crate::command::missing;
use crate::ids::{MediaId, MediaLabelId, MediaListId};
use crate::model::{LabelColor, Project};
use diag::AppError;
use imbl::{OrdMap, OrdSet, Vector};
use serde::{Deserialize, Serialize};

/// The longest name a label or list can have, in characters.
pub const NAME_LIMIT: usize = 40;

/// The longest note, in characters.
pub const NOTE_LIMIT: usize = 4_000;

/// Labels, lists and notes. Empty for projects saved before format 13.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    /// Every label, in the order they were made.
    pub labels: Vector<MediaLabel>,
    /// The labels on each file; files without any are left out.
    pub tagged: OrdMap<MediaId, OrdSet<MediaLabelId>>,
    /// Each file's note; files without one are left out.
    pub notes: OrdMap<MediaId, String>,
    /// The custom lists, in the order they were made.
    pub lists: Vector<MediaList>,
}

/// A named, coloured label any number of files can have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaLabel {
    pub id: MediaLabelId,
    pub name: String,
    pub color: LabelColor,
}

/// A custom list of files, in the order they were put in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaList {
    pub id: MediaListId,
    pub name: String,
    pub media: Vector<MediaId>,
}

impl Library {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.tagged.is_empty() && self.notes.is_empty() && self.lists.is_empty()
    }

    pub fn label(&self, id: MediaLabelId) -> Option<&MediaLabel> {
        self.labels.iter().find(|l| l.id == id)
    }

    pub fn list(&self, id: MediaListId) -> Option<&MediaList> {
        self.lists.iter().find(|l| l.id == id)
    }

    /// The labels on a file, in the order the labels were made.
    pub fn labels_of(&self, media: MediaId) -> Vec<&MediaLabel> {
        let Some(on) = self.tagged.get(&media) else {
            return Vec::new();
        };
        self.labels.iter().filter(|l| on.contains(&l.id)).collect()
    }

    pub fn has_label(&self, media: MediaId, label: MediaLabelId) -> bool {
        self.tagged.get(&media).is_some_and(|on| on.contains(&label))
    }

    pub fn note(&self, media: MediaId) -> &str {
        self.notes.get(&media).map_or("", String::as_str)
    }

    /// The lists a file is in.
    pub fn lists_of(&self, media: MediaId) -> Vec<&MediaList> {
        self.lists.iter().filter(|l| l.media.contains(&media)).collect()
    }

    /// The colour a new label gets: the first not taken yet, else the
    /// next round.
    pub fn next_color(&self) -> LabelColor {
        LabelColor::ALL
            .into_iter()
            .find(|c| !self.labels.iter().any(|l| l.color == *c))
            .unwrap_or(LabelColor::ALL[self.labels.len() % LabelColor::ALL.len()])
    }

    /// What breaks the rules, in plain words, given the project's files.
    pub(crate) fn problems(&self, has_media: impl Fn(MediaId) -> bool) -> Vec<String> {
        let mut problems = Vec::new();
        let mut names = std::collections::HashSet::new();
        for label in &self.labels {
            if !names.insert(label.name.to_lowercase()) {
                problems.push(format!("two labels are called {}", label.name));
            }
        }
        let mut ids = std::collections::HashSet::new();
        for label in &self.labels {
            if !ids.insert(label.id) {
                problems.push(format!("two labels have the id {}", label.id));
            }
        }
        for (media, on) in &self.tagged {
            if !has_media(*media) {
                problems.push(format!("labels are kept for missing media {media}"));
            }
            if on.is_empty() {
                problems.push(format!("{media} is kept with no labels"));
            }
            for label in on {
                if !ids.contains(label) {
                    problems.push(format!("{media} has the missing label {label}"));
                }
            }
        }
        for (media, note) in &self.notes {
            if !has_media(*media) {
                problems.push(format!("a note is kept for missing media {media}"));
            }
            if note.trim().is_empty() {
                problems.push(format!("{media} is kept with an empty note"));
            }
        }
        let mut list_names = std::collections::HashSet::new();
        let mut list_ids = std::collections::HashSet::new();
        for list in &self.lists {
            if !list_names.insert(list.name.to_lowercase()) {
                problems.push(format!("two lists are called {}", list.name));
            }
            if !list_ids.insert(list.id) {
                problems.push(format!("two lists have the id {}", list.id));
            }
            let mut seen = std::collections::HashSet::new();
            for media in &list.media {
                if !has_media(*media) {
                    problems.push(format!("the list {} holds missing media {media}", list.name));
                }
                if !seen.insert(*media) {
                    problems.push(format!("the list {} holds {media} twice", list.name));
                }
            }
        }
        problems
    }
}

/// A label or list name, trimmed; refused when empty, too long or taken
/// (whatever its capitals) by another than `except`.
fn checked_name<'a, T: PartialEq>(
    name: &str,
    what: &str,
    mut taken: impl Iterator<Item = (T, &'a str)>,
    except: Option<T>,
) -> diag::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::new(format!("A {what} needs a name")).fix("Type at least one letter."));
    }
    if name.chars().count() > NAME_LIMIT {
        return Err(AppError::new(format!("That {what} name is too long"))
            .why(format!(
                "Names are up to {NAME_LIMIT} characters, so they fit on a tab."
            ))
            .fix("Use a shorter name."));
    }
    let lower = name.to_lowercase();
    if taken.any(|(id, other)| Some(&id) != except.as_ref() && other.to_lowercase() == lower) {
        return Err(AppError::new(format!("There is already a {what} called {name}"))
            .why("Two of them with one name couldn't be told apart.")
            .fix("Pick another name, or use the one there is."));
    }
    Ok(name.to_owned())
}

fn check_media(p: &Project, media: &[MediaId]) -> diag::Result<()> {
    if media.iter().all(|m| p.media.contains_key(m)) {
        Ok(())
    } else {
        Err(missing("media file"))
    }
}

/// Makes a label (its `id` is ignored and a new one given) and puts it on `media`.
pub(crate) fn add_label(p: &mut Project, label: &MediaLabel, media: &[MediaId]) -> diag::Result<()> {
    check_media(p, media)?;
    let name = checked_name(
        &label.name,
        "label",
        p.library.labels.iter().map(|l| (l.id, l.name.as_str())),
        None,
    )?;
    let id: MediaLabelId = p.allocate_id();
    p.library.labels.push_back(MediaLabel {
        id,
        name,
        color: label.color,
    });
    set_label(p, media, id, true)
}

pub(crate) fn edit_label(
    p: &mut Project,
    label: MediaLabelId,
    name: &str,
    color: LabelColor,
) -> diag::Result<()> {
    let name = checked_name(
        name,
        "label",
        p.library.labels.iter().map(|l| (l.id, l.name.as_str())),
        Some(label),
    )?;
    let l = p
        .library
        .labels
        .iter_mut()
        .find(|l| l.id == label)
        .ok_or_else(|| missing("label"))?;
    l.name = name;
    l.color = color;
    Ok(())
}

/// Removes a label from the project and from every file that had it.
pub(crate) fn remove_label(p: &mut Project, label: MediaLabelId) -> diag::Result<()> {
    let before = p.library.labels.len();
    p.library.labels.retain(|l| l.id != label);
    if p.library.labels.len() == before {
        return Err(missing("label"));
    }
    let had: Vec<MediaId> = p
        .library
        .tagged
        .iter()
        .filter(|(_, on)| on.contains(&label))
        .map(|(m, _)| *m)
        .collect();
    for media in had {
        untag(&mut p.library, media, label);
    }
    Ok(())
}

fn untag(library: &mut Library, media: MediaId, label: MediaLabelId) {
    if let Some(on) = library.tagged.get_mut(&media) {
        on.remove(&label);
        if on.is_empty() {
            library.tagged.remove(&media);
        }
    }
}

/// Puts a label on files (`on`) or takes it off.
pub(crate) fn set_label(
    p: &mut Project,
    media: &[MediaId],
    label: MediaLabelId,
    on: bool,
) -> diag::Result<()> {
    check_media(p, media)?;
    if p.library.label(label).is_none() {
        return Err(missing("label"));
    }
    for m in media {
        if on {
            p.library.tagged.entry(*m).or_default().insert(label);
        } else {
            untag(&mut p.library, *m, label);
        }
    }
    Ok(())
}

/// Sets a file's note; an empty one removes it.
pub(crate) fn set_note(p: &mut Project, media: MediaId, note: &str) -> diag::Result<()> {
    check_media(p, &[media])?;
    let note = note.trim();
    if note.chars().count() > NOTE_LIMIT {
        return Err(AppError::new("That note is too long")
            .why(format!("Notes are up to {NOTE_LIMIT} characters."))
            .fix("Shorten it, or keep longer writing in a document beside the project."));
    }
    if note.is_empty() {
        p.library.notes.remove(&media);
    } else {
        p.library.notes.insert(media, note.to_owned());
    }
    Ok(())
}

/// Makes a list called `name` holding `media`.
pub(crate) fn add_list(p: &mut Project, name: &str, media: &[MediaId]) -> diag::Result<()> {
    check_media(p, media)?;
    let name = checked_name(
        name,
        "list",
        p.library.lists.iter().map(|l| (l.id, l.name.as_str())),
        None,
    )?;
    let id: MediaListId = p.allocate_id();
    let mut members = Vector::new();
    for m in media {
        if !members.contains(m) {
            members.push_back(*m);
        }
    }
    p.library.lists.push_back(MediaList {
        id,
        name,
        media: members,
    });
    Ok(())
}

pub(crate) fn rename_list(p: &mut Project, list: MediaListId, name: &str) -> diag::Result<()> {
    let name = checked_name(
        name,
        "list",
        p.library.lists.iter().map(|l| (l.id, l.name.as_str())),
        Some(list),
    )?;
    p.library
        .lists
        .iter_mut()
        .find(|l| l.id == list)
        .ok_or_else(|| missing("list"))?
        .name = name;
    Ok(())
}

/// Removes a list; its files stay in the project.
pub(crate) fn remove_list(p: &mut Project, list: MediaListId) -> diag::Result<()> {
    let before = p.library.lists.len();
    p.library.lists.retain(|l| l.id != list);
    if p.library.lists.len() == before {
        return Err(missing("list"));
    }
    Ok(())
}

/// Puts files in a list (at its end, `member`) or takes them out.
pub(crate) fn set_member(
    p: &mut Project,
    list: MediaListId,
    media: &[MediaId],
    member: bool,
) -> diag::Result<()> {
    check_media(p, media)?;
    let l = p
        .library
        .lists
        .iter_mut()
        .find(|l| l.id == list)
        .ok_or_else(|| missing("list"))?;
    for m in media {
        if member {
            if !l.media.contains(m) {
                l.media.push_back(*m);
            }
        } else {
            l.media.retain(|x| x != m);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::model::{Fingerprint, MediaKind, MediaRef};
    use crate::time::Time;

    fn with_files(n: usize) -> (Project, Vec<MediaId>) {
        let mut p = Project::new_automatic("T");
        for i in 0..n {
            p = Command::AddMedia {
                media: MediaRef {
                    id: MediaId(0),
                    path: format!("clip{i}.mkv").into(),
                    fingerprint: Fingerprint {
                        size: i as u64,
                        sample_hash: String::new(),
                    },
                    kind: MediaKind::Video,
                    duration: Time::seconds(5),
                    info: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        let ids = p.media.keys().copied().collect();
        (p, ids)
    }

    fn label(name: &str) -> MediaLabel {
        MediaLabel {
            id: MediaLabelId(0),
            name: name.into(),
            color: LabelColor::Green,
        }
    }

    #[test]
    fn labels_are_made_put_on_taken_off_and_removed() {
        let (p, m) = with_files(3);
        let p = Command::AddMediaLabel {
            label: label(" Drone "),
            media: vec![m[0], m[1]],
        }
        .apply(&p)
        .unwrap();
        let drone = p.library.labels[0].id;
        assert_eq!(p.library.labels[0].name, "Drone", "trimmed");
        assert!(p.library.has_label(m[0], drone) && p.library.has_label(m[1], drone));
        assert!(!p.library.has_label(m[2], drone));

        let err = Command::AddMediaLabel {
            label: label("drone"),
            media: vec![],
        }
        .apply(&p)
        .unwrap_err();
        assert!(
            err.what().contains("already"),
            "names are unique whatever their capitals"
        );

        let p = Command::SetMediaLabel {
            media: vec![m[0]],
            label: drone,
            on: false,
        }
        .apply(&p)
        .unwrap();
        assert!(!p.library.has_label(m[0], drone));
        assert!(!p.library.tagged.contains_key(&m[0]), "no empty sets are kept");

        let p = Command::EditMediaLabel {
            label: drone,
            name: "Aerial".into(),
            color: LabelColor::Blue,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.library.labels_of(m[1])[0].name, "Aerial");

        let p = Command::RemoveMediaLabel { label: drone }.apply(&p).unwrap();
        assert!(p.library.labels.is_empty() && p.library.tagged.is_empty());
        assert!(p.problems().is_empty(), "{:?}", p.problems());
    }

    #[test]
    fn notes_and_lists() {
        let (p, m) = with_files(2);
        let p = Command::SetMediaNote {
            media: m[1],
            note: "  Great save at 12:30 ".into(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.library.note(m[1]), "Great save at 12:30");
        let cleared = Command::SetMediaNote {
            media: m[1],
            note: " ".into(),
        }
        .apply(&p)
        .unwrap();
        assert!(cleared.library.notes.is_empty(), "an empty note is removed");

        let p = Command::AddMediaList {
            name: "Opening sequence".into(),
            media: vec![m[1], m[1]],
        }
        .apply(&p)
        .unwrap();
        let list = p.library.lists[0].id;
        assert_eq!(p.library.lists[0].media.len(), 1, "a file is in a list once");
        let p = Command::SetListMember {
            list,
            media: vec![m[0]],
            member: true,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(
            p.library.lists[0].media.iter().copied().collect::<Vec<_>>(),
            [m[1], m[0]]
        );
        let p = Command::RenameMediaList {
            list,
            name: "Intro".into(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.library.lists_of(m[0])[0].name, "Intro");
        assert!(p.problems().is_empty(), "{:?}", p.problems());
        let p = Command::RemoveMediaList { list }.apply(&p).unwrap();
        assert!(p.library.lists.is_empty());
        assert_eq!(
            p.media.len(),
            2,
            "removing a list leaves its files in the project"
        );
    }

    #[test]
    fn unknown_files_and_long_names_are_refused() {
        let (p, _) = with_files(1);
        assert!(
            Command::SetMediaNote {
                media: MediaId(999),
                note: "x".into()
            }
            .apply(&p)
            .is_err()
        );
        assert!(
            Command::AddMediaList {
                name: "x".repeat(NAME_LIMIT + 1),
                media: vec![]
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn new_labels_take_the_next_free_colour() {
        let (p, _) = with_files(0);
        assert_eq!(p.library.next_color(), LabelColor::ALL[0]);
        let p = Command::AddMediaLabel {
            label: MediaLabel {
                id: MediaLabelId(0),
                name: "A".into(),
                color: LabelColor::ALL[0],
            },
            media: vec![],
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.library.next_color(), LabelColor::ALL[1]);
    }
}
