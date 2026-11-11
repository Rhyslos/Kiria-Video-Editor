//! The keyboard shortcuts page: a drawn keyboard with every key that does
//! something marked, Ctrl / Shift / Alt to see the combinations, pointing
//! at a key to see what it does, and a searchable list of every command.
//! Everything comes from the one shortcut table.

use crate::Theme;
use crate::shortcuts::{Action, Shortcut, ShortcutTable};
use egui::{Align2, Color32, FontId, Key, Rect, Sense, Stroke, Ui, pos2, vec2};

/// One key on the drawing: its printed label, the key it sends (if any)
/// and its width in key units.
type Cap = (&'static str, Option<Key>, f32);

/// A US-style keyboard, row by row, with the editing keys and arrows on
/// the right. Shortcuts are stored by key position, so on other layouts
/// the same places do the same things.
const ROWS: [&[Cap]; 6] = [
    &[
        ("Esc", Some(Key::Escape), 1.0),
        ("", None, 0.5),
        ("F1", Some(Key::F1), 1.0),
        ("F2", Some(Key::F2), 1.0),
        ("F3", Some(Key::F3), 1.0),
        ("F4", Some(Key::F4), 1.0),
        ("", None, 0.5),
        ("F5", Some(Key::F5), 1.0),
        ("F6", Some(Key::F6), 1.0),
        ("F7", Some(Key::F7), 1.0),
        ("F8", Some(Key::F8), 1.0),
        ("", None, 0.5),
        ("F9", Some(Key::F9), 1.0),
        ("F10", Some(Key::F10), 1.0),
        ("F11", Some(Key::F11), 1.0),
        ("F12", Some(Key::F12), 1.0),
    ],
    &[
        ("`", Some(Key::Backtick), 1.0),
        ("1", Some(Key::Num1), 1.0),
        ("2", Some(Key::Num2), 1.0),
        ("3", Some(Key::Num3), 1.0),
        ("4", Some(Key::Num4), 1.0),
        ("5", Some(Key::Num5), 1.0),
        ("6", Some(Key::Num6), 1.0),
        ("7", Some(Key::Num7), 1.0),
        ("8", Some(Key::Num8), 1.0),
        ("9", Some(Key::Num9), 1.0),
        ("0", Some(Key::Num0), 1.0),
        ("-", Some(Key::Minus), 1.0),
        ("=", Some(Key::Equals), 1.0),
        ("Backspace", Some(Key::Backspace), 2.0),
    ],
    &[
        ("Tab", Some(Key::Tab), 1.5),
        ("Q", Some(Key::Q), 1.0),
        ("W", Some(Key::W), 1.0),
        ("E", Some(Key::E), 1.0),
        ("R", Some(Key::R), 1.0),
        ("T", Some(Key::T), 1.0),
        ("Y", Some(Key::Y), 1.0),
        ("U", Some(Key::U), 1.0),
        ("I", Some(Key::I), 1.0),
        ("O", Some(Key::O), 1.0),
        ("P", Some(Key::P), 1.0),
        ("[", Some(Key::OpenBracket), 1.0),
        ("]", Some(Key::CloseBracket), 1.0),
        ("\\", Some(Key::Backslash), 1.5),
    ],
    &[
        ("Caps", None, 1.75),
        ("A", Some(Key::A), 1.0),
        ("S", Some(Key::S), 1.0),
        ("D", Some(Key::D), 1.0),
        ("F", Some(Key::F), 1.0),
        ("G", Some(Key::G), 1.0),
        ("H", Some(Key::H), 1.0),
        ("J", Some(Key::J), 1.0),
        ("K", Some(Key::K), 1.0),
        ("L", Some(Key::L), 1.0),
        (";", Some(Key::Semicolon), 1.0),
        ("'", Some(Key::Quote), 1.0),
        ("Enter", Some(Key::Enter), 2.25),
    ],
    &[
        ("Shift", None, 2.25),
        ("Z", Some(Key::Z), 1.0),
        ("X", Some(Key::X), 1.0),
        ("C", Some(Key::C), 1.0),
        ("V", Some(Key::V), 1.0),
        ("B", Some(Key::B), 1.0),
        ("N", Some(Key::N), 1.0),
        ("M", Some(Key::M), 1.0),
        (",", Some(Key::Comma), 1.0),
        (".", Some(Key::Period), 1.0),
        ("/", Some(Key::Slash), 1.0),
        ("Shift", None, 2.75),
    ],
    &[
        ("Ctrl", None, 1.5),
        ("", None, 1.25),
        ("Alt", None, 1.25),
        ("Space", Some(Key::Space), 6.0),
        ("Alt", None, 1.25),
        ("", None, 1.25),
        ("Ctrl", None, 1.5),
    ],
];

/// The editing keys and arrows, drawn right of the main block: (label, key,
/// column, row) with rows counted like [`ROWS`].
const SIDE: [(&str, Key, f32, f32); 10] = [
    ("Ins", Key::Insert, 0.0, 1.0),
    ("Home", Key::Home, 1.0, 1.0),
    ("PgUp", Key::PageUp, 2.0, 1.0),
    ("Del", Key::Delete, 0.0, 2.0),
    ("End", Key::End, 1.0, 2.0),
    ("PgDn", Key::PageDown, 2.0, 2.0),
    ("↑", Key::ArrowUp, 1.0, 4.0),
    ("←", Key::ArrowLeft, 0.0, 5.0),
    ("↓", Key::ArrowDown, 1.0, 5.0),
    ("→", Key::ArrowRight, 2.0, 5.0),
];

/// The page's state: what is held and what was searched.
#[derive(Debug, Clone, Default)]
pub struct ShortcutsPage {
    pub search: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    /// A key clicked to keep its actions shown.
    pinned: Option<Key>,
}

impl ShortcutsPage {
    /// The actions of `key` with the modifiers chosen on the page.
    fn actions_of(&self, table: &ShortcutTable, key: Key) -> Vec<Action> {
        table
            .bindings()
            .iter()
            .filter(|(_, s)| {
                s.key == key && s.ctrl == self.ctrl && s.shift == self.shift && s.alt == self.alt
            })
            .map(|(a, _)| *a)
            .collect()
    }

    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, table: &ShortcutTable) {
        ui.horizontal(|ui| {
            ui.label("Holding:");
            ui.toggle_value(&mut self.ctrl, "Ctrl");
            ui.toggle_value(&mut self.shift, "Shift");
            ui.toggle_value(&mut self.alt, "Alt");
            ui.label(
                egui::RichText::new("Point at a key to see what it does; click to keep it shown.")
                    .color(theme.muted),
            );
        });
        ui.add_space(6.0);
        let unit = ((ui.available_width() - 20.0) / 19.5).clamp(22.0, 44.0);
        let (rect, _) = ui.allocate_exact_size(vec2(unit * 19.5, unit * 6.4), Sense::hover());
        let mut hovered = None;
        for (row, caps) in ROWS.iter().enumerate() {
            let mut x = rect.left();
            let y = rect.top() + row as f32 * unit * 1.05 + if row > 0 { unit * 0.2 } else { 0.0 };
            for (label, key, width) in caps.iter() {
                let r = Rect::from_min_size(pos2(x, y), vec2(unit * width - 3.0, unit - 3.0));
                if !label.is_empty()
                    && let Some(k) = self.draw_key(ui, theme, table, r, label, *key)
                {
                    hovered = Some(k);
                }
                x += unit * width;
            }
        }
        for (label, key, col, row) in SIDE {
            let x = rect.left() + unit * (15.75 + col);
            let y = rect.top() + row * unit * 1.05 + unit * 0.2;
            let r = Rect::from_min_size(pos2(x, y), vec2(unit - 3.0, unit - 3.0));
            if let Some(k) = self.draw_key(ui, theme, table, r, label, Some(key)) {
                hovered = Some(k);
            }
        }
        if let Some(key) = hovered.or(self.pinned) {
            let actions = self.actions_of(table, key);
            let text = if actions.is_empty() {
                format!("{key:?}: nothing yet")
            } else {
                let names: Vec<&str> = actions.iter().map(|a| a.label()).collect();
                format!(
                    "{}: {}",
                    Shortcut {
                        key,
                        ctrl: self.ctrl,
                        shift: self.shift,
                        alt: self.alt
                    }
                    .label(),
                    names.join(" · ")
                )
            };
            ui.label(egui::RichText::new(text).family(crate::fonts::semibold()));
        } else {
            ui.label(" ");
        }
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Find a command"));
        ui.add_space(4.0);
        let q = self.search.to_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("all-shortcuts").striped(true).show(ui, |ui| {
                    for (action, shortcut) in table.bindings() {
                        if !action.label().to_lowercase().contains(&q)
                            && !shortcut.label().to_lowercase().contains(&q)
                        {
                            continue;
                        }
                        ui.label(action.label());
                        ui.label(egui::RichText::new(shortcut.label()).family(egui::FontFamily::Monospace));
                        ui.end_row();
                    }
                });
            });
    }

    /// One key: marked when it does something with the chosen modifiers.
    /// Returns the key when the pointer is on it.
    fn draw_key(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        table: &ShortcutTable,
        r: Rect,
        label: &str,
        key: Option<Key>,
    ) -> Option<Key> {
        let used = key.is_some_and(|k| !self.actions_of(table, k).is_empty());
        let held = matches!(label, "Ctrl" if self.ctrl)
            || matches!(label, "Shift" if self.shift)
            || matches!(label, "Alt" if self.alt);
        let response = ui.interact(
            r,
            ui.scope_id().with(("key", r.min.x as i32, r.min.y as i32)),
            Sense::click(),
        );
        let pinned = key.is_some() && self.pinned == key;
        let fill = if used || held {
            theme
                .accent
                .gamma_multiply(if response.hovered() || pinned { 0.6 } else { 0.35 })
        } else if response.hovered() {
            theme.line_strong
        } else {
            theme.raised
        };
        let painter = ui.painter();
        painter.rect_filled(r, 5.0, fill);
        painter.rect_stroke(
            r,
            5.0,
            Stroke::new(
                if pinned { 2.0 } else { 1.0 },
                if pinned { theme.accent } else { theme.line },
            ),
            egui::StrokeKind::Inside,
        );
        let colour = if used || held { theme.text } else { theme.faint };
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional((r.height() * 0.32).clamp(9.0, 13.0)),
            colour,
        );
        if used {
            painter.circle_filled(pos2(r.right() - 6.0, r.top() + 6.0), 2.5, Color32::WHITE);
        }
        if response.clicked() {
            if let Some(k) = key {
                self.pinned = if self.pinned == Some(k) { None } else { Some(k) };
            }
            match label {
                "Ctrl" => self.ctrl = !self.ctrl,
                "Shift" => self.shift = !self.shift,
                "Alt" => self.alt = !self.alt,
                _ => {}
            }
        }
        if response.hovered() { key } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bound_key_is_on_the_drawing() {
        let drawn: Vec<Key> = ROWS
            .iter()
            .flat_map(|r| r.iter().filter_map(|(_, k, _)| *k))
            .chain(SIDE.iter().map(|(_, k, _, _)| *k))
            .collect();
        for (action, shortcut) in ShortcutTable::default().bindings() {
            assert!(
                drawn.contains(&shortcut.key),
                "{action:?} on {:?} can't be seen",
                shortcut.key
            );
        }
    }

    #[test]
    fn the_page_finds_what_a_key_does() {
        let table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        assert_eq!(page.actions_of(&table, Key::Space), [Action::PlayPause]);
        page.ctrl = true;
        assert_eq!(page.actions_of(&table, Key::S), [Action::Save]);
    }
}
