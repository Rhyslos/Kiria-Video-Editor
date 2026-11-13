//! What an undo or redo changed, in words for the status line, and the
//! timeline it changed: when that is not the one on screen, the window
//! opens it (which timeline is open is view state, so this is no edit),
//! so a change never happens out of sight.

use super::Editor;
use project::{Project, Sequence, SequenceId};

/// One step of the history as the person sees it.
#[derive(Debug, PartialEq)]
pub(crate) struct Step {
    /// The one timeline whose clips, tracks or labels the step changed,
    /// when it changed exactly one that is still there.
    pub timeline: Option<SequenceId>,
    /// What the change did, going forward ("removed 2 clips in Alt cut").
    pub says: String,
}

/// The change that takes `from` to `to`.
pub(crate) fn step(from: &Project, to: &Project) -> Step {
    let name = |id: SequenceId| {
        to.sequence(id)
            .or_else(|| from.sequence(id))
            .map_or_else(String::new, |s| s.name.clone())
    };
    let made: Vec<_> = to
        .sequences
        .iter()
        .filter(|s| from.sequence(s.id).is_none())
        .collect();
    let gone: Vec<_> = from
        .sequences
        .iter()
        .filter(|s| to.sequence(s.id).is_none())
        .collect();
    if let Some(s) = made.first() {
        return Step {
            timeline: Some(s.id),
            says: format!("made the timeline {}", s.name),
        };
    }
    if let Some(s) = gone.first() {
        return Step {
            timeline: None,
            says: format!("removed the timeline {}", s.name),
        };
    }
    let changed: Vec<(&Sequence, &Sequence)> = to
        .sequences
        .iter()
        .filter_map(|after| from.sequence(after.id).map(|before| (before, after)))
        .filter(|(before, after)| !same(before, after))
        .collect();
    match changed.as_slice() {
        [(before, after)] => Step {
            timeline: Some(after.id),
            says: format!("{} in {}", in_timeline(before, after), name(after.id)),
        },
        [] if from.media != to.media || from.library != to.library => Step {
            timeline: None,
            says: "changed the project's media".into(),
        },
        [] if from.name != to.name => Step {
            timeline: None,
            says: format!("renamed the project {}", to.name),
        },
        [] => Step {
            timeline: None,
            says: "a change".into(),
        },
        several => Step {
            timeline: None,
            says: format!("changed {} timelines", several.len()),
        },
    }
}

/// Whether a timeline is the same in both, open or not.
fn same(a: &Sequence, b: &Sequence) -> bool {
    a == b
}

/// What changed inside one timeline, in a few words.
fn in_timeline(before: &Sequence, after: &Sequence) -> String {
    if before.name != after.name {
        return format!("renamed the timeline {}", after.name);
    }
    let ids = |s: &Sequence| {
        s.tracks()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    let (old, new) = (ids(before), ids(after));
    let added = new.iter().filter(|c| !old.contains(c)).count();
    let removed = old.iter().filter(|c| !new.contains(c)).count();
    let clips = |n: usize| {
        if n == 1 {
            "1 clip".to_owned()
        } else {
            format!("{n} clips")
        }
    };
    let tracks = |s: &Sequence| s.tracks().count();
    match (added, removed) {
        (0, 0) if tracks(before) != tracks(after) => "changed the tracks".into(),
        (0, 0) if before.labels != after.labels => "changed the labels".into(),
        (0, 0) => "changed clips".into(),
        (a, 0) => format!("added {}", clips(a)),
        (0, r) => format!("removed {}", clips(r)),
        _ => "changed clips".into(),
    }
}

impl Editor {
    /// Undo (`forward` false) or redo, then: the timeline it changed is
    /// opened when another is on screen, and the status line says what was
    /// undone or redone, and where.
    pub(super) fn step_history(&mut self, forward: bool) -> diag::Result<()> {
        let Some(before) = self.document.as_ref().map(|d| d.project().clone()) else {
            return self.with_document(|_| Ok(()));
        };
        let mut moved = false;
        self.with_document(|d| {
            moved = if forward { d.redo()? } else { d.undo()? };
            Ok(())
        })?;
        let Some(document) = self.document.as_mut() else {
            return Ok(());
        };
        if !moved {
            self.status = if forward {
                "Nothing to redo.".into()
            } else {
                "Nothing to undo.".into()
            };
            return Ok(());
        }
        let after = document.project();
        // Undo reverses a change: say what the change itself did.
        let step = if forward {
            step(&before, after)
        } else {
            step(after, &before)
        };
        let open = after.active_sequence;
        let mut opened = String::new();
        if let Some(id) = step.timeline.filter(|id| *id != open)
            && document.open_sequence(id)
        {
            let name = document.project().sequence(id).map_or("", |s| s.name.as_str());
            opened = format!(" {name} is open now.");
        }
        let done = if forward { "Redone" } else { "Undone" };
        self.status = format!("{done}: {}.{opened}", step.says);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Command;

    #[test]
    fn a_step_names_what_changed_and_where() {
        let p = crate::inspector::tests::project_with_a_clip();
        let main = p.active_sequence;
        let other = Command::DuplicateSequence {
            sequence: main,
            name: "Alt cut".into(),
        }
        .apply(&p)
        .unwrap();
        let made = step(&p, &other);
        assert!(made.says.contains("Alt cut"), "{}", made.says);
        let alt = other.sequences.iter().find(|s| s.name == "Alt cut").unwrap().id;
        assert_eq!(made.timeline, Some(alt));
        // A clip removed there, with the main timeline open.
        let mut shown = other.clone();
        shown.active_sequence = main;
        let clip = shown.sequence(alt).unwrap().video_tracks[0].clips[0].id;
        let removed = Command::RemoveClips {
            sequence: alt,
            clips: vec![clip],
            ripple: false,
        }
        .apply(&shown)
        .unwrap();
        let s = step(&shown, &removed);
        assert_eq!(s.timeline, Some(alt), "the timeline to open");
        assert!(s.says.starts_with("removed "), "{}", s.says);
        assert!(s.says.ends_with("in Alt cut"), "{}", s.says);
    }
}
