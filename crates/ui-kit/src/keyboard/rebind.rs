//! Changing a shortcut, step by step: pick a line of the table, give it
//! keys, and if another command has them, swap or cancel. Nothing here
//! draws; the page shows what [`Rebinding::state`] says.

use crate::shortcuts::{Clash, Shortcut, ShortcutTable};

/// Where a change stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for the new keys of this line of the table.
    Listening { line: usize },
    /// The keys asked for belong to another command: swap or cancel.
    /// Other keys can still be tried.
    Clash {
        line: usize,
        keys: Shortcut,
        with: Clash,
    },
}

impl State {
    /// The line being changed.
    pub fn line(&self) -> usize {
        match self {
            State::Listening { line } | State::Clash { line, .. } => *line,
        }
    }
}

/// The change under way on the shortcuts page, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rebinding(Option<State>);

impl Rebinding {
    pub fn state(&self) -> Option<State> {
        self.0
    }

    /// Starts waiting for new keys for `line`.
    pub fn begin(&mut self, line: usize) {
        self.0 = Some(State::Listening { line });
    }

    pub fn cancel(&mut self) {
        self.0 = None;
    }

    /// Gives the line being changed `keys`. True when the table changed;
    /// when another command has the keys nothing changes and the state
    /// says which.
    pub fn offer(&mut self, table: &mut ShortcutTable, keys: Shortcut) -> bool {
        let Some(line) = self.0.map(|s| s.line()) else {
            return false;
        };
        let before = table.lines().get(line).and_then(|b| b.keys);
        match table.rebind(line, keys) {
            Ok(()) => {
                self.0 = None;
                before != Some(keys)
            }
            Err(with) => {
                self.0 = Some(State::Clash { line, keys, with });
                false
            }
        }
    }

    /// After a clash: the line takes the keys and the command that had
    /// them gets this line's old ones. True when the table changed.
    pub fn swap(&mut self, table: &mut ShortcutTable) -> bool {
        let Some(State::Clash { line, keys, .. }) = self.0 else {
            return false;
        };
        table.swap(line, keys);
        self.0 = None;
        true
    }

    /// Leaves the line being changed without a shortcut.
    pub fn clear(&mut self, table: &mut ShortcutTable) -> bool {
        let Some(line) = self.0.take().map(|s| s.line()) else {
            return false;
        };
        let had = table.lines().get(line).is_some_and(|b| b.keys.is_some());
        table.clear(line);
        had
    }

    /// Puts `line` back to its usual keys. If another command has taken
    /// them, that is a clash like any other.
    pub fn reset(&mut self, table: &mut ShortcutTable, line: usize) -> bool {
        let Some(binding) = table.lines().get(line).copied() else {
            return false;
        };
        match (table.reset(line), binding.usual()) {
            (Err(with), Some(keys)) => {
                self.0 = Some(State::Clash { line, keys, with });
                false
            }
            _ => {
                self.0 = None;
                binding.changed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use egui::Key;

    fn line(table: &ShortcutTable, action: Action) -> usize {
        table.lines().iter().position(|b| b.action == action).unwrap()
    }

    #[test]
    fn free_keys_are_taken_at_once() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        assert!(
            !rebinding.offer(&mut table, Shortcut::key(Key::F9)),
            "nothing was picked"
        );
        assert_eq!(table, ShortcutTable::default());

        let split = line(&table, Action::Split);
        rebinding.begin(split);
        assert_eq!(rebinding.state(), Some(State::Listening { line: split }));
        assert!(rebinding.offer(&mut table, Shortcut::key(Key::F9)));
        assert_eq!(rebinding.state(), None);
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::F9)));
    }

    #[test]
    fn the_keys_a_line_already_has_change_nothing() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        rebinding.begin(line(&table, Action::Split));
        assert!(!rebinding.offer(&mut table, Shortcut::key(Key::S)));
        assert_eq!(rebinding.state(), None);
        assert_eq!(table, ShortcutTable::default());
    }

    #[test]
    fn taken_keys_wait_for_swap_or_cancel() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        let (split, save) = (line(&table, Action::Split), line(&table, Action::Save));
        rebinding.begin(split);
        assert!(!rebinding.offer(&mut table, Shortcut::ctrl(Key::S)));
        assert_eq!(
            rebinding.state(),
            Some(State::Clash {
                line: split,
                keys: Shortcut::ctrl(Key::S),
                with: Clash {
                    line: save,
                    action: Action::Save
                },
            })
        );
        assert_eq!(
            table,
            ShortcutTable::default(),
            "nothing changes before the answer"
        );

        // Cancel: as it was.
        let mut cancelled = rebinding;
        cancelled.cancel();
        assert_eq!(cancelled.state(), None);
        assert_eq!(table, ShortcutTable::default());

        // Swap: they trade.
        assert!(rebinding.swap(&mut table));
        assert_eq!(rebinding.state(), None);
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::ctrl(Key::S)));
        assert_eq!(table.shortcut_for(Action::Save), Some(Shortcut::key(Key::S)));
        assert!(!rebinding.swap(&mut table), "nothing left to swap");
    }

    #[test]
    fn other_keys_can_be_tried_after_a_clash() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        rebinding.begin(line(&table, Action::Split));
        assert!(!rebinding.offer(&mut table, Shortcut::ctrl(Key::S)));
        assert!(rebinding.offer(&mut table, Shortcut::key(Key::F9)));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::F9)));
        assert_eq!(table.shortcut_for(Action::Save), Some(Shortcut::ctrl(Key::S)));
    }

    #[test]
    fn a_line_can_be_left_without_keys() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        rebinding.begin(line(&table, Action::Split));
        assert!(rebinding.clear(&mut table));
        assert_eq!(table.shortcut_for(Action::Split), None);
        assert_eq!(rebinding.state(), None);
        rebinding.begin(line(&table, Action::Split));
        assert!(!rebinding.clear(&mut table), "it had none already");
    }

    #[test]
    fn reset_brings_back_the_usual_keys_or_says_who_has_them() {
        let mut table = ShortcutTable::default();
        let mut rebinding = Rebinding::default();
        let (split, stop) = (line(&table, Action::Split), line(&table, Action::Stop));
        assert!(!rebinding.reset(&mut table, split), "nothing to put back");

        rebinding.begin(split);
        assert!(rebinding.offer(&mut table, Shortcut::key(Key::F9)));
        assert!(rebinding.reset(&mut table, split));
        assert_eq!(table, ShortcutTable::default());

        // Split moves away and Stop takes S: putting Split back is a clash.
        rebinding.begin(split);
        rebinding.offer(&mut table, Shortcut::key(Key::F9));
        rebinding.begin(stop);
        rebinding.offer(&mut table, Shortcut::key(Key::S));
        assert!(!rebinding.reset(&mut table, split));
        assert_eq!(
            rebinding.state(),
            Some(State::Clash {
                line: split,
                keys: Shortcut::key(Key::S),
                with: Clash {
                    line: stop,
                    action: Action::Stop
                },
            })
        );
        assert!(rebinding.swap(&mut table));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::S)));
        assert_eq!(table.shortcut_for(Action::Stop), Some(Shortcut::key(Key::F9)));
    }
}
