//! The one shortcut table every page reads.
//!
//! Shortcuts are matched on the key's physical position, so they work on
//! every keyboard layout: S splits from the same spot on a Norwegian, US or
//! Arabic keyboard.
//!
//! The table holds the bindings in use, not only the defaults: changing a
//! shortcut on the keyboard shortcuts page changes it here, and menus,
//! tooltips, search and the page itself all read it back from here. What
//! differs from the defaults is what settings keep ([`SavedBindings`]).
//!
//! No two commands ever share a combination. Every context is live at once
//! today (the viewer and the timeline are on one page, and keys act wherever
//! the pointer is), so one combination can do one thing in the whole table;
//! [`ShortcutTable::clash`] is the one place that decides it.

mod actions;
pub mod mouse;
mod saved;

pub use actions::{Action, Context};
pub use saved::{SavedBinding, SavedBindings};

use egui::{Event, Key, Modifiers};

/// A key plus modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    /// Ctrl on Windows and Linux.
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Shortcut {
    pub const fn key(key: Key) -> Shortcut {
        Shortcut {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    pub const fn ctrl(key: Key) -> Shortcut {
        Shortcut {
            ctrl: true,
            ..Shortcut::key(key)
        }
    }

    pub const fn shift(key: Key) -> Shortcut {
        Shortcut {
            shift: true,
            ..Shortcut::key(key)
        }
    }

    pub const fn alt(key: Key) -> Shortcut {
        Shortcut {
            alt: true,
            ..Shortcut::key(key)
        }
    }

    pub const fn ctrl_shift(key: Key) -> Shortcut {
        Shortcut {
            shift: true,
            ..Shortcut::ctrl(key)
        }
    }

    /// `key` with the modifiers held in `modifiers`.
    pub fn held(key: Key, modifiers: Modifiers) -> Shortcut {
        Shortcut {
            key,
            ctrl: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }

    /// As shown in menus and tooltips, for example "Ctrl+Shift+Z".
    pub fn label(&self) -> String {
        self.written(self.key.name())
    }

    /// "Ctrl+Shift+" and so on, then `key`.
    pub(crate) fn written(&self, key: &str) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(key);
        parts.join("+")
    }

    /// Reads back what [`Shortcut::label`] writes; `None` for anything else.
    pub fn parse(text: &str) -> Option<Shortcut> {
        let mut parts: Vec<&str> = text.split('+').collect();
        let mut shortcut = Shortcut::key(Key::from_name(parts.pop()?)?);
        for part in parts {
            let modifier = match part {
                "Ctrl" => &mut shortcut.ctrl,
                "Shift" => &mut shortcut.shift,
                "Alt" => &mut shortcut.alt,
                _ => return None,
            };
            if std::mem::replace(modifier, true) {
                return None;
            }
        }
        Some(shortcut)
    }

    fn matches(&self, key: Key, modifiers: Modifiers) -> bool {
        *self == Shortcut::held(key, modifiers)
    }
}

/// One line of the table: a command and the combination that triggers it.
/// A command with two shortcuts has two lines; one with none has a line
/// without keys, so it can be given some.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub action: Action,
    /// The combination in use; `None` when the command has no shortcut.
    pub keys: Option<Shortcut>,
    /// What it is when nothing was changed.
    usual: Option<Shortcut>,
}

impl Binding {
    /// The combination this line has until someone changes it.
    pub fn usual(&self) -> Option<Shortcut> {
        self.usual
    }

    /// The combination was changed (or taken away, or given).
    pub fn changed(&self) -> bool {
        self.keys != self.usual
    }

    /// The combination as text, or "none".
    pub fn label(&self) -> String {
        self.keys.map_or_else(|| "none".to_owned(), |k| k.label())
    }
}

/// A combination that another command already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clash {
    /// The line of the table that has it.
    pub line: usize,
    /// The command on that line.
    pub action: Action,
}

/// The default combinations. A command may be here twice (two shortcuts)
/// or not at all (none until someone gives it one).
const USUAL: &[(Action, Shortcut)] = &[
    (Action::PlayPause, Shortcut::key(Key::Space)),
    (Action::Split, Shortcut::key(Key::S)),
    (Action::Delete, Shortcut::key(Key::Delete)),
    (Action::RippleDelete, Shortcut::key(Key::Backspace)),
    (Action::AddLabel, Shortcut::key(Key::M)),
    (Action::Undo, Shortcut::ctrl(Key::Z)),
    (Action::Redo, Shortcut::ctrl_shift(Key::Z)),
    (Action::Redo, Shortcut::ctrl(Key::Y)),
    (Action::Save, Shortcut::ctrl(Key::S)),
    (Action::Search, Shortcut::ctrl(Key::K)),
    (Action::OpenSettings, Shortcut::ctrl(Key::Comma)),
    (Action::Import, Shortcut::ctrl(Key::I)),
    (Action::PreviousFrame, Shortcut::key(Key::ArrowLeft)),
    (Action::NextFrame, Shortcut::key(Key::ArrowRight)),
    (Action::GoToStart, Shortcut::key(Key::Home)),
    (Action::GoToEnd, Shortcut::key(Key::End)),
    (Action::ClearSelection, Shortcut::key(Key::Escape)),
    (Action::ZoomToFit, Shortcut::key(Key::Backslash)),
    // The key with = and +, with or without Shift, and the one beside it.
    (Action::ZoomIn, Shortcut::key(Key::Equals)),
    (Action::ZoomIn, Shortcut::shift(Key::Equals)),
    (Action::ZoomOut, Shortcut::key(Key::Minus)),
    // Alt is for height: the same three keys, and Alt+wheel on the timeline.
    (Action::TallerTracks, Shortcut::alt(Key::Equals)),
    (Action::ShorterTracks, Shortcut::alt(Key::Minus)),
    (Action::UsualTrackHeights, Shortcut::alt(Key::Backslash)),
    (Action::SelectTool, Shortcut::key(Key::V)),
    (Action::BladeTool, Shortcut::key(Key::B)),
    (Action::ToggleSnapping, Shortcut::key(Key::N)),
    (Action::RippleTool, Shortcut::key(Key::R)),
    (Action::RollTool, Shortcut::key(Key::T)),
    (Action::SlipTool, Shortcut::key(Key::Y)),
    (Action::SlideTool, Shortcut::key(Key::U)),
    (Action::ToggleLoop, Shortcut::key(Key::L)),
    (Action::SaveFrame, Shortcut::ctrl_shift(Key::E)),
    (Action::RemoveSilence, Shortcut::ctrl_shift(Key::R)),
    (Action::FullScreenViewer, Shortcut::key(Key::F)),
    (Action::PictureHandles, Shortcut::key(Key::Q)),
    (Action::Versions, Shortcut::ctrl_shift(Key::V)),
    (Action::Templates, Shortcut::ctrl(Key::T)),
    (Action::SaveTemplate, Shortcut::ctrl_shift(Key::S)),
    (Action::RoughCut, Shortcut::ctrl_shift(Key::T)),
    (Action::KeyboardShortcuts, Shortcut::ctrl(Key::Slash)),
];

/// Every binding, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutTable {
    /// In the order of [`Action::ALL`]; a command's lines stay together.
    lines: Vec<Binding>,
}

impl Default for ShortcutTable {
    fn default() -> Self {
        let mut lines = Vec::new();
        for action in Action::ALL {
            let before = lines.len();
            for (_, keys) in USUAL.iter().filter(|(a, _)| a == action) {
                lines.push(Binding {
                    action: *action,
                    keys: Some(*keys),
                    usual: Some(*keys),
                });
            }
            if lines.len() == before {
                lines.push(Binding {
                    action: *action,
                    keys: None,
                    usual: None,
                });
            }
        }
        ShortcutTable { lines }
    }
}

impl ShortcutTable {
    /// Every line of the table, commands without a shortcut included: what
    /// the keyboard shortcuts page lists. Lines are known by their place here.
    pub fn lines(&self) -> &[Binding] {
        &self.lines
    }

    /// Every command that has a combination, with it.
    pub fn bindings(&self) -> impl Iterator<Item = (Action, Shortcut)> + '_ {
        self.lines.iter().filter_map(|b| Some((b.action, b.keys?)))
    }

    /// The first shortcut bound to `action`, for tooltips.
    pub fn shortcut_for(&self, action: Action) -> Option<Shortcut> {
        self.bindings().find(|(a, _)| *a == action).map(|(_, s)| s)
    }

    /// The first shortcut bound to `action` as text ("Ctrl+Z"), or nothing
    /// when it has none: for hints written into sentences and buttons.
    pub fn keys_for(&self, action: Action) -> String {
        self.shortcut_for(action).map_or_else(String::new, |s| s.label())
    }

    /// The first shortcut bound to `action`, or the command's name when it
    /// has none: for sentences such as "Ctrl+Z undoes it".
    pub fn keys_or_name(&self, action: Action) -> String {
        self.shortcut_for(action)
            .map_or_else(|| action.label().to_owned(), |s| s.label())
    }

    /// The command's name with its shortcut, if it has one: "Undo (Ctrl+Z)".
    pub fn tip(&self, action: Action) -> String {
        match self.shortcut_for(action) {
            Some(keys) => format!("{} ({})", action.label(), keys.label()),
            None => action.label().to_owned(),
        }
    }

    /// The actions triggered by `key` with `modifiers`.
    pub fn actions_for(&self, key: Key, modifiers: Modifiers) -> impl Iterator<Item = Action> + '_ {
        self.bindings()
            .filter(move |(_, s)| s.matches(key, modifiers))
            .map(|(a, _)| a)
    }

    /// The actions pressed this frame. While a text field has focus only
    /// Ctrl shortcuts count, so typing a space or an "s" never edits the timeline.
    pub fn pressed(&self, ctx: &egui::Context) -> Vec<Action> {
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some((physical_key.unwrap_or(*key), *modifiers)),
                    _ => None,
                })
                .filter(|(_, modifiers)| !typing || modifiers.command)
                .flat_map(|(key, modifiers)| self.actions_for(key, modifiers).collect::<Vec<_>>())
                .collect()
        })
    }

    /// The other line that already has `keys`, if giving them to `line`
    /// would put two commands on one combination.
    pub fn clash(&self, line: usize, keys: Shortcut) -> Option<Clash> {
        self.lines
            .iter()
            .enumerate()
            .find(|(i, b)| *i != line && b.keys == Some(keys))
            .map(|(i, b)| Clash {
                line: i,
                action: b.action,
            })
    }

    /// Gives `line` the combination `keys`. Refused, with nothing changed,
    /// when another line has them: see [`ShortcutTable::swap`].
    pub fn rebind(&mut self, line: usize, keys: Shortcut) -> Result<(), Clash> {
        if let Some(clash) = self.clash(line, keys) {
            return Err(clash);
        }
        if let Some(binding) = self.lines.get_mut(line) {
            binding.keys = Some(keys);
        }
        Ok(())
    }

    /// Gives `line` the combination `keys`, and the line that had them what
    /// `line` had before (which may be nothing).
    pub fn swap(&mut self, line: usize, keys: Shortcut) {
        let Some(before) = self.lines.get(line).map(|b| b.keys) else {
            return;
        };
        if let Some(clash) = self.clash(line, keys)
            && let Some(other) = self.lines.get_mut(clash.line)
        {
            other.keys = before;
        }
        if let Some(binding) = self.lines.get_mut(line) {
            binding.keys = Some(keys);
        }
    }

    /// Leaves `line` without a shortcut.
    pub fn clear(&mut self, line: usize) {
        if let Some(binding) = self.lines.get_mut(line) {
            binding.keys = None;
        }
    }

    /// Puts `line` back to its usual combination. Refused when another line
    /// has taken it meanwhile, like [`ShortcutTable::rebind`].
    pub fn reset(&mut self, line: usize) -> Result<(), Clash> {
        match self.lines.get(line).map(|b| b.usual) {
            Some(Some(usual)) => self.rebind(line, usual),
            Some(None) => {
                self.clear(line);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Every line back to its usual combination.
    pub fn reset_all(&mut self) {
        for binding in &mut self.lines {
            binding.keys = binding.usual;
        }
    }

    /// Any line differs from its usual combination.
    pub fn changed(&self) -> bool {
        self.lines.iter().any(Binding::changed)
    }

    /// Which of a command's lines `line` is: 0 for its first shortcut.
    fn nth(&self, line: usize) -> usize {
        let action = self.lines[line].action;
        self.lines[..line].iter().filter(|b| b.action == action).count()
    }

    /// The line that is the `nth` shortcut of `action`.
    fn line_of(&self, action: Action, nth: usize) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, b)| b.action == action)
            .map(|(i, _)| i)
            .nth(nth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(table: &ShortcutTable, action: Action) -> usize {
        table.line_of(action, 0).unwrap()
    }

    #[test]
    fn the_agreed_keys_are_bound() {
        let table = ShortcutTable::default();
        let plain = Modifiers::NONE;
        let first = |key| table.actions_for(key, plain).next();
        assert_eq!(first(Key::Space), Some(Action::PlayPause));
        assert_eq!(first(Key::S), Some(Action::Split));
        assert_eq!(first(Key::Delete), Some(Action::Delete));
        assert_eq!(first(Key::Backspace), Some(Action::RippleDelete));
        assert_eq!(first(Key::M), Some(Action::AddLabel));
        assert_eq!(
            table.actions_for(Key::I, Modifiers::COMMAND).next(),
            Some(Action::Import)
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let table = ShortcutTable::default();
        assert_eq!(
            table.actions_for(Key::S, Modifiers::COMMAND).next(),
            Some(Action::Save)
        );
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(table.actions_for(Key::Z, ctrl_shift).next(), Some(Action::Redo));
        assert_eq!(
            table.actions_for(Key::Z, Modifiers::COMMAND).next(),
            Some(Action::Undo)
        );
    }

    #[test]
    fn the_timeline_zooms_from_the_keys_beside_backspace() {
        let table = ShortcutTable::default();
        let first = |key, modifiers| table.actions_for(key, modifiers).next();
        assert_eq!(first(Key::Equals, Modifiers::NONE), Some(Action::ZoomIn));
        assert_eq!(
            first(Key::Equals, Modifiers::SHIFT),
            Some(Action::ZoomIn),
            "+ is Shift and = on many keyboards"
        );
        assert_eq!(first(Key::Minus, Modifiers::NONE), Some(Action::ZoomOut));
        assert_eq!(first(Key::Backslash, Modifiers::NONE), Some(Action::ZoomToFit));
    }

    #[test]
    fn alt_with_the_zoom_keys_changes_track_heights() {
        let table = ShortcutTable::default();
        let first = |key| table.actions_for(key, Modifiers::ALT).next();
        assert_eq!(first(Key::Equals), Some(Action::TallerTracks));
        assert_eq!(first(Key::Minus), Some(Action::ShorterTracks));
        assert_eq!(first(Key::Backslash), Some(Action::UsualTrackHeights));
        assert_eq!(Shortcut::alt(Key::Minus).label(), "Alt+Minus");
    }

    #[test]
    fn templates_have_their_keys() {
        let table = ShortcutTable::default();
        let first = |key, modifiers| table.actions_for(key, modifiers).next();
        assert_eq!(first(Key::T, Modifiers::COMMAND), Some(Action::Templates));
        assert_eq!(
            first(Key::S, Modifiers::COMMAND | Modifiers::SHIFT),
            Some(Action::SaveTemplate)
        );
        assert_eq!(
            first(Key::T, Modifiers::NONE),
            Some(Action::RollTool),
            "plain T is still a tool"
        );
        assert_eq!(first(Key::S, Modifiers::COMMAND), Some(Action::Save));
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(Shortcut::ctrl_shift(Key::Z).label(), "Ctrl+Shift+Z");
        assert_eq!(Shortcut::key(Key::M).label(), "M");
    }

    /// No combination is on two lines.
    fn assert_no_key_twice(table: &ShortcutTable) {
        let bound: Vec<(Action, Shortcut)> = table.bindings().collect();
        for (i, (action, a)) in bound.iter().enumerate() {
            for (other, b) in &bound[i + 1..] {
                assert_ne!(a, b, "{} is on {action:?} and {other:?}", a.label());
            }
        }
    }

    #[test]
    fn no_key_is_bound_twice() {
        assert_no_key_twice(&ShortcutTable::default());
    }

    #[test]
    fn every_command_has_a_line_and_the_usual_table_is_unchanged() {
        let table = ShortcutTable::default();
        for action in Action::ALL {
            assert!(table.lines().iter().any(|b| b.action == *action), "{action:?}");
        }
        assert_eq!(table.bindings().count(), USUAL.len());
        assert!(!table.changed());
        assert_eq!(table.shortcut_for(Action::Stop), None, "no shortcut yet");
        assert_eq!(table.lines()[line(&table, Action::Stop)].label(), "none");
    }

    #[test]
    fn a_shortcut_is_read_back_from_its_text() {
        for key in Key::ALL {
            for modifiers in [
                Modifiers::NONE,
                Modifiers::COMMAND,
                Modifiers::SHIFT | Modifiers::ALT,
                Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT,
            ] {
                let shortcut = Shortcut::held(*key, modifiers);
                assert_eq!(Shortcut::parse(&shortcut.label()), Some(shortcut), "{key:?}");
            }
        }
        for nonsense in ["", "Ctrl+", "Ctrl+Ctrl+Z", "Hyper+Z", "Ctrl+Nokey", "Z+Ctrl"] {
            assert_eq!(Shortcut::parse(nonsense), None, "{nonsense}");
        }
    }

    #[test]
    fn rebinding_moves_a_command_to_free_keys_everywhere_it_is_shown() {
        let mut table = ShortcutTable::default();
        let split = line(&table, Action::Split);
        let keys = Shortcut::ctrl(Key::B);
        assert_eq!(table.rebind(split, keys), Ok(()));
        assert_eq!(table.shortcut_for(Action::Split), Some(keys));
        assert_eq!(table.keys_for(Action::Split), "Ctrl+B");
        assert_eq!(table.tip(Action::Split), "Split at the edit needle (Ctrl+B)");
        assert_eq!(table.keys_or_name(Action::Split), "Ctrl+B");
        assert_eq!(table.keys_or_name(Action::Stop), "Stop playing here");
        assert_eq!(table.keys_for(Action::Stop), "");
        assert_eq!(table.tip(Action::Stop), "Stop playing here");
        assert_eq!(
            table.actions_for(Key::B, Modifiers::COMMAND).next(),
            Some(Action::Split)
        );
        assert_eq!(
            table.actions_for(Key::S, Modifiers::NONE).next(),
            None,
            "the old key is free"
        );
        assert!(table.lines()[split].changed());
        assert!(table.changed());
        // The same keys again is no clash with itself.
        assert_eq!(table.rebind(split, keys), Ok(()));
        assert_no_key_twice(&table);
    }

    #[test]
    fn keys_another_command_has_are_refused_and_nothing_changes() {
        let mut table = ShortcutTable::default();
        let before = table.clone();
        let split = line(&table, Action::Split);
        let save = line(&table, Action::Save);
        let clash = table.rebind(split, Shortcut::ctrl(Key::S));
        assert_eq!(
            clash,
            Err(Clash {
                line: save,
                action: Action::Save
            })
        );
        assert_eq!(table, before);
        // A command's own second shortcut clashes with its first, too.
        let redo_again = table.line_of(Action::Redo, 1).unwrap();
        assert_eq!(
            table
                .rebind(redo_again, Shortcut::ctrl_shift(Key::Z))
                .map_err(|c| c.action),
            Err(Action::Redo)
        );
    }

    #[test]
    fn swapping_trades_the_two_combinations() {
        let mut table = ShortcutTable::default();
        let split = line(&table, Action::Split);
        table.swap(split, Shortcut::ctrl(Key::S));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::ctrl(Key::S)));
        assert_eq!(table.shortcut_for(Action::Save), Some(Shortcut::key(Key::S)));
        assert_no_key_twice(&table);

        // A command without a shortcut takes one: the other is left with none.
        let stop = line(&table, Action::Stop);
        table.swap(stop, Shortcut::key(Key::Space));
        assert_eq!(table.shortcut_for(Action::Stop), Some(Shortcut::key(Key::Space)));
        assert_eq!(table.shortcut_for(Action::PlayPause), None);
        assert_no_key_twice(&table);

        // Free keys: a swap is a plain change.
        table.swap(stop, Shortcut::key(Key::F9));
        assert_eq!(table.shortcut_for(Action::Stop), Some(Shortcut::key(Key::F9)));
        assert_eq!(table.shortcut_for(Action::PlayPause), None);
    }

    #[test]
    fn clearing_and_resetting_one_line() {
        let mut table = ShortcutTable::default();
        let split = line(&table, Action::Split);
        table.clear(split);
        assert_eq!(table.shortcut_for(Action::Split), None);
        assert!(table.lines()[split].changed());
        assert_eq!(table.reset(split), Ok(()));
        assert_eq!(table, ShortcutTable::default());

        // A command that usually has none goes back to none.
        let stop = line(&table, Action::Stop);
        assert_eq!(table.rebind(stop, Shortcut::key(Key::F9)), Ok(()));
        assert_eq!(table.reset(stop), Ok(()));
        assert_eq!(table, ShortcutTable::default());
    }

    #[test]
    fn resetting_a_line_whose_usual_keys_were_taken_is_a_clash() {
        let mut table = ShortcutTable::default();
        let split = line(&table, Action::Split);
        let stop = line(&table, Action::Stop);
        assert_eq!(table.rebind(split, Shortcut::key(Key::F9)), Ok(()));
        assert_eq!(table.rebind(stop, Shortcut::key(Key::S)), Ok(()));
        assert_eq!(table.reset(split).map_err(|c| c.action), Err(Action::Stop));
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::key(Key::F9)));
        assert_no_key_twice(&table);
    }

    #[test]
    fn reset_all_brings_back_the_usual_table() {
        let mut table = ShortcutTable::default();
        table.swap(line(&table, Action::Split), Shortcut::ctrl(Key::S));
        table.clear(line(&table, Action::Undo));
        table.reset_all();
        assert_eq!(table, ShortcutTable::default());
        assert!(!table.changed());
    }

    #[test]
    fn lines_that_do_not_exist_change_nothing() {
        let mut table = ShortcutTable::default();
        let nowhere = table.lines().len();
        table.swap(nowhere, Shortcut::key(Key::S));
        table.clear(nowhere);
        assert_eq!(table.reset(nowhere), Ok(()));
        assert_eq!(table.rebind(nowhere, Shortcut::key(Key::F9)), Ok(()));
        assert_eq!(table, ShortcutTable::default());
    }
}
