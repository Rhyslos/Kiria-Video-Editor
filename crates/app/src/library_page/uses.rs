//! How each file is to be added from the Library: its marked part and the
//! level of each of its sound streams (`Use`).
//!
//! Where they are kept is a setting (`settings::LibraryMarks`): for the
//! session (the usual: nothing is written to the project), or in the
//! project (`project::MediaUse`, format 15), where each change is a
//! `SetMediaUses` command, so one undo step per mark or level, and undo
//! brings back the marks before it. `Uses` holds the page's working copy
//! either way; when they are kept in the project it is the project's,
//! read again every frame (so undo shows at once), and every change also
//! goes out as a command. Switching from the session to the project writes
//! the session's marks into the project at once, as one step
//! (`Uses::write_all`); switching back keeps them for the session.

use project::{Command, MediaId, MediaUse, Project};
use std::collections::{BTreeMap, HashMap};
use ui_kit::scrub::Marks;

/// How one file is to be added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Use {
    pub marks: Marks,
    /// The change of level in thousandths of a decibel, by sound stream number.
    pub levels: BTreeMap<u32, i32>,
}

impl Use {
    pub fn is_empty(&self) -> bool {
        !self.marks.is_set() && self.levels.values().all(|l| *l == 0)
    }

    pub fn from_project(kept: &MediaUse) -> Use {
        Use {
            marks: Marks {
                mark_in: kept.mark_in.map(project::Time::as_secs_f64),
                mark_out: kept.mark_out.map(project::Time::as_secs_f64),
            },
            levels: kept
                .levels
                .iter()
                .map(|(s, l)| (*s, *l))
                .filter(|(_, l)| *l != 0)
                .collect(),
        }
    }

    /// As the project keeps it, inside a file `length` long (marks on the
    /// wrong side of each other, or outside the file, are dropped).
    pub fn to_project(&self, length: f64) -> MediaUse {
        let inside = |m: Option<f64>| {
            m.filter(|s| (0.0..=length).contains(s))
                .map(crate::editor::seconds)
        };
        let (mut mark_in, mut mark_out) = (inside(self.marks.mark_in), inside(self.marks.mark_out));
        if let (Some(a), Some(b)) = (mark_in, mark_out)
            && a >= b
        {
            (mark_in, mark_out) = (None, None);
        }
        MediaUse {
            mark_in,
            mark_out,
            levels: self
                .levels
                .iter()
                .filter(|(_, l)| **l != 0)
                .map(|(s, l)| (*s, *l))
                .collect(),
        }
    }
}

/// The page's working copy of every file's use, and where it is kept.
#[derive(Debug, Default)]
pub struct Uses {
    files: HashMap<MediaId, Use>,
    /// Kept in the project: changes go out as commands.
    saved: bool,
    /// Commands for the window to apply, when kept in the project.
    outbox: Vec<Command>,
}

impl Uses {
    pub fn get(&self, media: MediaId) -> Use {
        self.files.get(&media).cloned().unwrap_or_default()
    }

    pub fn saved(&self) -> bool {
        self.saved
    }

    /// Changes `media`'s use (a file `length` long); kept in the project,
    /// the change also goes out as one command.
    pub fn change(&mut self, media: MediaId, length: f64, change: impl FnOnce(&mut Use)) {
        let mut used = self.get(media);
        let before = used.clone();
        change(&mut used);
        if used == before {
            return;
        }
        if self.saved {
            self.outbox.push(Command::SetMediaUses {
                uses: vec![(media, used.to_project(length))],
            });
        }
        if used.is_empty() {
            self.files.remove(&media);
        } else {
            self.files.insert(media, used);
        }
    }

    /// Every frame: where the uses are kept, and, when that is the
    /// project, its uses as they are now (after undo, too).
    pub fn follow(&mut self, saved: bool, project: &Project) {
        self.saved = saved;
        if saved {
            self.files = project
                .library
                .uses
                .iter()
                .map(|(m, kept)| (*m, Use::from_project(kept)))
                .collect();
        }
    }

    /// The command that writes every use kept for the session into
    /// `project`, as one step: when switching to keeping them in the
    /// project. `None` when the project already has them all.
    pub fn write_all(&self, project: &Project) -> Option<Command> {
        let mut uses: Vec<(MediaId, MediaUse)> = Vec::new();
        for (media, used) in &self.files {
            let Some(file) = project.media.get(media) else {
                continue;
            };
            let kept = used.to_project(file.duration.as_secs_f64());
            if project.library.use_of(*media) != kept {
                uses.push((*media, kept));
            }
        }
        uses.sort_by_key(|(m, _)| *m);
        (!uses.is_empty()).then_some(Command::SetMediaUses { uses })
    }

    /// The commands made since last asked, for the window to apply.
    pub fn take_outbox(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, MediaInfo, MediaKind, MediaRef, Time};

    fn project_with_file() -> (Project, MediaId) {
        let p = Project::new_automatic("T");
        let p = Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: "/r/a.wav".into(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: String::new(),
                },
                kind: MediaKind::Audio,
                duration: Time::seconds(10),
                info: MediaInfo::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let id = *p.media.keys().next().unwrap();
        (p, id)
    }

    #[test]
    fn for_the_session_nothing_goes_to_the_project() {
        let (p, media) = project_with_file();
        let mut uses = Uses::default();
        uses.follow(false, &p);
        uses.change(media, 10.0, |u| u.marks.set_in(2.0));
        assert_eq!(uses.get(media).marks.mark_in, Some(2.0));
        assert!(uses.take_outbox().is_empty());
        // Switching to the project writes them all at once.
        let write = uses.write_all(&p).unwrap();
        let p = write.apply(&p).unwrap();
        assert_eq!(p.library.use_of(media).mark_in, Some(Time::seconds(2)));
        assert!(uses.write_all(&p).is_none(), "nothing left to write");
    }

    #[test]
    fn in_the_project_each_change_is_a_command_and_undo_shows() {
        let (p, media) = project_with_file();
        let mut uses = Uses::default();
        uses.follow(true, &p);
        uses.change(media, 10.0, |u| {
            u.levels.insert(1, 2_000);
        });
        uses.change(media, 10.0, |u| u.marks.set_out(4.0));
        let commands = uses.take_outbox();
        assert_eq!(commands.len(), 2, "one step per change");
        let mut after = p.clone();
        for c in commands {
            after = c.apply(&after).unwrap();
        }
        uses.follow(true, &after);
        assert_eq!(uses.get(media).levels.get(&1), Some(&2_000));
        assert_eq!(uses.get(media).marks.mark_out, Some(4.0));
        // Undone: the project as before, and the page follows it.
        uses.follow(true, &p);
        assert_eq!(uses.get(media), Use::default());
    }

    #[test]
    fn marks_outside_the_file_are_not_kept() {
        let used = Use {
            marks: Marks {
                mark_in: Some(3.0),
                mark_out: Some(30.0),
            },
            levels: [(0, 0)].into_iter().collect(),
        };
        let kept = used.to_project(10.0);
        assert_eq!(kept.mark_in, Some(Time::seconds(3)));
        assert_eq!(kept.mark_out, None);
        assert!(kept.levels.is_empty(), "a level left as it is isn't kept");
        assert_eq!(Use::from_project(&kept).marks.mark_in, Some(3.0));
    }
}
