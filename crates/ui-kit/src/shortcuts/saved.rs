//! Changed shortcuts as settings keep them: only what differs from the
//! usual table, by name, so settings written by another version of the app
//! still load.

use super::{Action, Shortcut, ShortcutTable};
use serde::{Deserialize, Serialize};

/// One changed line of the table.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedBinding {
    /// The command's name ([`Action::name`]).
    pub command: String,
    /// Which of the command's shortcuts: 0 for its first.
    pub nth: usize,
    /// The combination as text ("Ctrl+Shift+Z", by key position), or
    /// `None` for a command left without one.
    pub keys: Option<String>,
}

/// Every changed line. Empty when the table is the usual one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SavedBindings(pub Vec<SavedBinding>);

impl ShortcutTable {
    /// What to keep in settings: the lines that differ from the usual table.
    pub fn saved(&self) -> SavedBindings {
        SavedBindings(
            self.lines
                .iter()
                .enumerate()
                .filter(|(_, b)| b.changed())
                .map(|(i, b)| SavedBinding {
                    command: b.action.name().to_owned(),
                    nth: self.nth(i),
                    keys: b.keys.map(|k| k.label()),
                })
                .collect(),
        )
    }

    /// The usual table with `saved` changes made again.
    ///
    /// Settings can be old, from a newer version or edited by hand, so this
    /// takes what still makes sense and never ends with one combination on
    /// two commands: a command or key that is not known is skipped, and a
    /// combination that something else has goes to whoever comes first,
    /// the other keeping its usual keys if they are free and none if not.
    pub fn restored(saved: &SavedBindings) -> ShortcutTable {
        let mut table = ShortcutTable::default();
        // What each changed line should end up with.
        let mut wanted: Vec<(usize, Option<Shortcut>)> = Vec::new();
        for entry in &saved.0 {
            let Some(line) =
                Action::from_name(&entry.command).and_then(|action| table.line_of(action, entry.nth))
            else {
                continue;
            };
            let keys = match &entry.keys {
                None => None,
                Some(text) => match Shortcut::parse(text) {
                    Some(keys) => Some(keys),
                    None => continue,
                },
            };
            if !wanted.iter().any(|(l, _)| *l == line) {
                wanted.push((line, keys));
            }
        }
        // Changed lines let go of their keys first, so two lines can have
        // traded places.
        for (line, _) in &wanted {
            table.clear(*line);
        }
        let mut unplaced = Vec::new();
        for (line, keys) in wanted {
            if let Some(keys) = keys
                && table.rebind(line, keys).is_err()
            {
                unplaced.push(line);
            }
        }
        for line in unplaced {
            // Its usual keys, unless another line has them now.
            let _ = table.reset(line);
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    fn line(table: &ShortcutTable, action: Action) -> usize {
        table.line_of(action, 0).unwrap()
    }

    fn entry(command: &str, nth: usize, keys: Option<&str>) -> SavedBinding {
        SavedBinding {
            command: command.into(),
            nth,
            keys: keys.map(str::to_owned),
        }
    }

    fn keys_are_single(table: &ShortcutTable) -> bool {
        let bound: Vec<Shortcut> = table.bindings().map(|(_, s)| s).collect();
        bound.iter().enumerate().all(|(i, a)| !bound[i + 1..].contains(a))
    }

    #[test]
    fn the_usual_table_saves_nothing() {
        assert_eq!(ShortcutTable::default().saved(), SavedBindings::default());
        assert_eq!(
            ShortcutTable::restored(&SavedBindings::default()),
            ShortcutTable::default()
        );
    }

    #[test]
    fn changes_come_back_as_they_were_saved() {
        let mut table = ShortcutTable::default();
        // A swap, a second shortcut moved, one taken away and one given.
        table.swap(line(&table, Action::Split), Shortcut::ctrl(Key::S));
        let redo_again = table.line_of(Action::Redo, 1).unwrap();
        assert_eq!(table.rebind(redo_again, Shortcut::key(Key::F5)), Ok(()));
        table.clear(line(&table, Action::AddLabel));
        assert_eq!(
            table.rebind(line(&table, Action::Stop), Shortcut::alt(Key::Space)),
            Ok(())
        );

        let saved = table.saved();
        assert_eq!(
            saved.0,
            [
                entry("Redo", 1, Some("F5")),
                entry("Save", 0, Some("S")),
                entry("Stop", 0, Some("Alt+Space")),
                entry("Split", 0, Some("Ctrl+S")),
                entry("AddLabel", 0, None),
            ],
            "only what changed, by name"
        );
        assert_eq!(ShortcutTable::restored(&saved), table);
    }

    #[test]
    fn what_is_not_known_is_skipped() {
        let saved = SavedBindings(vec![
            entry("FlyToTheMoon", 0, Some("Ctrl+M")),
            entry("Split", 7, Some("Ctrl+M")),
            entry("Split", 0, Some("Ctrl+Whatever")),
            entry("AddLabel", 0, Some("Ctrl+M")),
        ]);
        let table = ShortcutTable::restored(&saved);
        assert_eq!(table.shortcut_for(Action::AddLabel), Some(Shortcut::ctrl(Key::M)));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::S)));
        assert_eq!(table.saved().0, [entry("AddLabel", 0, Some("Ctrl+M"))]);
    }

    #[test]
    fn saved_keys_never_put_two_commands_on_one_combination() {
        // Ctrl+S is Save's and stays there; Split keeps its S.
        let table = ShortcutTable::restored(&SavedBindings(vec![entry("Split", 0, Some("Ctrl+S"))]));
        assert_eq!(table.shortcut_for(Action::Save), Some(Shortcut::ctrl(Key::S)));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::S)));

        // Two lines asking for the same keys: the first has them. The second
        // can't have its usual S either (Stop took it), so it has none.
        let table = ShortcutTable::restored(&SavedBindings(vec![
            entry("Stop", 0, Some("S")),
            entry("AddLabel", 0, Some("F9")),
            entry("Split", 0, Some("F9")),
            entry("Split", 0, Some("F10")),
        ]));
        assert_eq!(table.shortcut_for(Action::Stop), Some(Shortcut::key(Key::S)));
        assert_eq!(table.shortcut_for(Action::AddLabel), Some(Shortcut::key(Key::F9)));
        assert_eq!(table.shortcut_for(Action::Split), None);
        assert!(keys_are_single(&table));
    }

    #[test]
    fn any_mix_of_saved_lines_keeps_combinations_single() {
        // Every pair of commands sent to every one of a few combinations.
        let combos = ["S", "Ctrl+S", "Space", "F9", "Ctrl+Shift+Z"];
        let commands = ["Split", "Save", "Redo", "Stop", "PlayPause"];
        for a in commands {
            for b in commands {
                for first in combos {
                    for second in combos {
                        let saved = SavedBindings(vec![
                            entry(a, 0, Some(first)),
                            entry(b, 1, Some(second)),
                            entry(b, 0, Some(second)),
                        ]);
                        let table = ShortcutTable::restored(&saved);
                        assert!(keys_are_single(&table), "{saved:?}");
                        // And what it saves restores to itself.
                        assert_eq!(ShortcutTable::restored(&table.saved()), table, "{saved:?}");
                    }
                }
            }
        }
    }
}
