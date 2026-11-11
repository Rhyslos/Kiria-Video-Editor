//! The one shortcut table every page reads.
//!
//! Shortcuts are matched on the key's physical position, so they work on
//! every keyboard layout: S splits from the same spot on a Norwegian, US or
//! Arabic keyboard.

use egui::{Event, Key, Modifiers};
use serde::{Deserialize, Serialize};

/// Everything a shortcut can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    PlayPause,
    Split,
    Delete,
    RippleDelete,
    AddLabel,
    Undo,
    Redo,
    Save,
    Search,
    OpenSettings,
    Import,
    PreviousFrame,
    NextFrame,
    GoToStart,
    GoToEnd,
    Stop,
    ClearSelection,
    ZoomToFit,
    SelectTool,
    BladeTool,
    ToggleSnapping,
    RippleTool,
    RollTool,
    SlipTool,
    SlideTool,
    ToggleLoop,
    SaveFrame,
    RemoveSilence,
    FullScreenViewer,
    ClearRegion,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::PlayPause => "Play / pause",
            Action::Split => "Split at the edit needle",
            Action::Delete => "Delete the selected clip",
            Action::RippleDelete => "Delete and close the gap",
            Action::AddLabel => "Add a timeline label",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Save => "Save",
            Action::Search => "Search",
            Action::OpenSettings => "Settings",
            Action::Import => "Import media",
            Action::PreviousFrame => "Back one frame",
            Action::NextFrame => "Forward one frame",
            Action::GoToStart => "Go to the start",
            Action::GoToEnd => "Go to the end",
            Action::Stop => "Stop playing here",
            Action::ClearSelection => "Select nothing",
            Action::ZoomToFit => "Show the whole timeline",
            Action::SelectTool => "Select tool: pick, move and trim clips",
            Action::BladeTool => "Blade tool: cut clips where clicked",
            Action::ToggleSnapping => "Snapping on or off",
            Action::RippleTool => "Ripple tool: trims move everything after",
            Action::RollTool => "Roll tool: move the cut between two clips",
            Action::SlipTool => "Slip tool: show another part of a clip",
            Action::SlideTool => "Slide tool: move a clip between its neighbours",
            Action::FullScreenViewer => "Viewer full screen (Escape returns)",
            Action::RemoveSilence => "Remove silence…",
            Action::SaveFrame => "Save this frame as a picture…",
            Action::ToggleLoop => "Loop playback inside the region",
            Action::ClearRegion => "Clear the region",
        }
    }
}

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

    pub const fn ctrl_shift(key: Key) -> Shortcut {
        Shortcut {
            shift: true,
            ..Shortcut::ctrl(key)
        }
    }

    /// As shown in menus and tooltips, for example "Ctrl+Shift+Z".
    pub fn label(&self) -> String {
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
        parts.push(self.key.name());
        parts.join("+")
    }

    fn matches(&self, key: Key, modifiers: Modifiers) -> bool {
        self.key == key
            && self.ctrl == modifiers.command
            && self.shift == modifiers.shift
            && self.alt == modifiers.alt
    }
}

/// Every binding, in one place.
#[derive(Debug, Clone)]
pub struct ShortcutTable {
    bindings: Vec<(Action, Shortcut)>,
}

impl Default for ShortcutTable {
    fn default() -> Self {
        ShortcutTable {
            bindings: vec![
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
            ],
        }
    }
}

impl ShortcutTable {
    /// All bindings, for the keyboard shortcuts page and menus.
    pub fn bindings(&self) -> &[(Action, Shortcut)] {
        &self.bindings
    }

    /// The first shortcut bound to `action`, for tooltips.
    pub fn shortcut_for(&self, action: Action) -> Option<Shortcut> {
        self.bindings.iter().find(|(a, _)| *a == action).map(|(_, s)| *s)
    }

    /// The actions triggered by `key` with `modifiers`.
    pub fn actions_for(&self, key: Key, modifiers: Modifiers) -> impl Iterator<Item = Action> + '_ {
        self.bindings
            .iter()
            .filter(move |(_, s)| s.matches(key, modifiers))
            .map(|(a, _)| *a)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn labels_read_naturally() {
        assert_eq!(Shortcut::ctrl_shift(Key::Z).label(), "Ctrl+Shift+Z");
        assert_eq!(Shortcut::key(Key::M).label(), "M");
    }

    #[test]
    fn no_key_is_bound_twice() {
        let table = ShortcutTable::default();
        for (i, (_, a)) in table.bindings().iter().enumerate() {
            for (_, b) in &table.bindings()[i + 1..] {
                assert_ne!(a, b, "{} is bound twice", a.label());
            }
        }
    }
}
