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
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(previous) => {
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
                self.undo.push(std::mem::replace(&mut self.current, next));
                true
            }
            None => false,
        }
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
