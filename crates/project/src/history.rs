//! Undo and redo.
//!
//! The history keeps the snapshot before every change. Because snapshots
//! share unchanged data, unlimited undo costs little memory.

use crate::command::Command;
use crate::model::Project;

/// The current project plus everything needed to undo and redo.
#[derive(Debug, Clone)]
pub struct History {
    current: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl History {
    pub fn new(project: Project) -> History {
        History {
            current: project,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn current(&self) -> &Project {
        &self.current
    }

    /// Applies a command. On error nothing changes.
    pub fn apply(&mut self, command: &Command) -> diag::Result<()> {
        let next = command.apply(&self.current)?;
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.redo.clear();
        Ok(())
    }

    /// Steps back one change. Returns false when there is nothing to undo.
    /// The timeline that is open stays open when it was there before the
    /// change (see [`History::open`]).
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(previous) => {
                let previous = self.keeping_open(previous);
                self.redo.push(std::mem::replace(&mut self.current, previous));
                true
            }
            None => false,
        }
    }

    /// Steps forward again. Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                let next = self.keeping_open(next);
                self.undo.push(std::mem::replace(&mut self.current, next));
                true
            }
            None => false,
        }
    }

    /// Opens another timeline of the project. Which timeline is open is not
    /// an edit: it makes no undo step and is not written to the journal
    /// (the editor keeps it with the timeline's layout), and undo and redo
    /// leave it open while it exists in the state they go back or on to.
    /// Returns false, changing nothing, when the project has no such timeline.
    pub fn open(&mut self, sequence: crate::ids::SequenceId) -> bool {
        if self.current.sequence(sequence).is_none() {
            return false;
        }
        self.current.active_sequence = sequence;
        true
    }

    /// `snapshot` with the timeline open now still open, when it has it.
    fn keeping_open(&self, mut snapshot: Project) -> Project {
        let open = self.current.active_sequence;
        if snapshot.sequence(open).is_some() {
            snapshot.active_sequence = open;
        }
        snapshot
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SequenceSettings;

    fn rename(name: &str) -> Command {
        Command::RenameProject { name: name.into() }
    }

    #[test]
    fn undo_and_redo_walk_the_snapshots() {
        let mut h = History::new(Project::new("A", SequenceSettings::GAME_CLIP));
        h.apply(&rename("B")).unwrap();
        h.apply(&rename("C")).unwrap();
        assert!(h.undo());
        assert_eq!(h.current().name, "B");
        assert!(h.undo());
        assert_eq!(h.current().name, "A");
        assert!(!h.undo());
        assert!(h.redo());
        assert_eq!(h.current().name, "B");
    }

    #[test]
    fn a_new_change_clears_redo() {
        let mut h = History::new(Project::new("A", SequenceSettings::GAME_CLIP));
        h.apply(&rename("B")).unwrap();
        h.undo();
        h.apply(&rename("D")).unwrap();
        assert!(!h.can_redo());
    }

    #[test]
    fn a_failed_command_changes_nothing() {
        let mut h = History::new(Project::new("A", SequenceSettings::GAME_CLIP));
        assert!(h.apply(&rename("   ")).is_err());
        assert_eq!(h.current().name, "A");
        assert!(!h.can_undo());
    }
}
