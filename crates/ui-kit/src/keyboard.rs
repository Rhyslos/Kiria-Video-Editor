//! The keyboard shortcuts page: a drawn keyboard with every key that does
//! something marked in its context's colour, Ctrl / Shift / Alt to see the
//! combinations, pointing at a key to see what it does, context chips that
//! filter, a list of every command where shortcuts are changed, and what
//! the mouse does. Everything comes from the one shortcut table, and
//! changes go back into it.

pub mod cheat_sheet;
mod drawing;
pub mod layout;
mod rebind;

pub use layout::{KeyboardChoice, Layout, LayoutId, SystemKeyboard};
pub use rebind::{Rebinding, State};

use crate::Theme;
use crate::shortcuts::mouse::{MOUSE, MouseAction};
use crate::shortcuts::{Action, Binding, Context, Shortcut, ShortcutTable};
use drawing::{KeyLook, chip, dot};
use egui::{Key, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use layout::Spot;

/// The page's state: what is held, searched, filtered and being changed.
#[derive(Debug, Clone, Default)]
pub struct ShortcutsPage {
    pub search: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    /// Ctrl, Shift and Alt as they are held on the real keyboard right now:
    /// they count like the toggles.
    down: egui::Modifiers,
    /// A key clicked to keep its actions shown.
    pinned: Option<Key>,
    /// The contexts picked on the chips. None picked shows everything.
    filter: Vec<Context>,
    rebinding: Rebinding,
    /// "Reset all" was clicked and waits for a yes.
    asking_reset_all: bool,
}

/// What the page asks of the window it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageResponse {
    /// A shortcut or the keyboard layout changed: save them.
    pub changed: bool,
    /// The cheat sheet was asked for.
    pub cheat_sheet: bool,
}

/// States the page can be opened in for screenshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// Only the Timeline chip picked.
    Filter,
    /// Waiting for new keys for Split.
    Rebinding,
    /// Split asked for Save's keys.
    Clash,
}

impl Preview {
    /// The state called `name` on the command line.
    pub fn from_name(name: &str) -> Option<Preview> {
        match name {
            "filter" => Some(Preview::Filter),
            "rebinding" => Some(Preview::Rebinding),
            "clash" => Some(Preview::Clash),
            _ => None,
        }
    }
}

/// What the keyboard said while the page waited for keys.
enum Pressed {
    Keys(Shortcut),
    /// Escape alone: give up the change.
    Cancel,
}

/// What was done to the drawn keyboard this frame.
#[derive(Default)]
struct KeyboardHits {
    hovered: Option<Key>,
    clicked: Option<Key>,
}

impl ShortcutsPage {
    /// The page as a screenshot wants it.
    pub fn preview(preview: Preview, table: &ShortcutTable) -> ShortcutsPage {
        let mut page = ShortcutsPage::default();
        let split = table.lines().iter().position(|b| b.action == Action::Split);
        match (preview, split) {
            (Preview::Filter, _) => page.filter = vec![Context::Timeline],
            (Preview::Rebinding, Some(line)) => page.rebinding.begin(line),
            (Preview::Clash, Some(line)) => {
                page.rebinding.begin(line);
                // Taken keys change nothing, so a copy of the table will do.
                if let Some(keys) = table.shortcut_for(Action::Save) {
                    page.rebinding.offer(&mut table.clone(), keys);
                }
            }
            (_, None) => {}
        }
        page
    }

    /// The page is waiting for keys: what is pressed now belongs to it,
    /// so the window must not run commands from it.
    pub fn listening(&self) -> bool {
        self.rebinding.state().is_some()
    }

    /// The combination a key makes with the modifiers held on the page.
    fn combination(&self, key: Key) -> Shortcut {
        Shortcut {
            key,
            ctrl: self.ctrl || self.down.command,
            shift: self.shift || self.down.shift,
            alt: self.alt || self.down.alt,
        }
    }

    /// Whether the chips let `context` through.
    fn shows(&self, context: Context) -> bool {
        self.filter.is_empty() || self.filter.contains(&context)
    }

    /// Picks or unpicks a chip.
    fn toggle(&mut self, context: Context) {
        match self.filter.iter().position(|c| *c == context) {
            Some(at) => {
                self.filter.remove(at);
            }
            None => self.filter.push(context),
        }
    }

    fn searched(&self, texts: &[&str]) -> bool {
        let q = self.search.trim().to_lowercase();
        q.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(&q))
    }

    /// The lines of the table the list shows: those the chips and the
    /// search box let through.
    fn listed(&self, table: &ShortcutTable) -> Vec<usize> {
        table
            .lines()
            .iter()
            .enumerate()
            .filter(|(_, b)| self.shows(b.action.context()))
            .filter(|(_, b)| {
                // By the name of the keys, or as the keyboard in use prints them.
                let printed = b.keys.map(|k| table.written(k)).unwrap_or_default();
                let q = self.search.trim().to_lowercase();
                b.action.found_by(&q) || self.searched(&[&b.label(), &printed])
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The mouse actions the Mouse section shows.
    fn listed_mouse(&self, table: &ShortcutTable) -> Vec<(&'static MouseAction, String)> {
        MOUSE
            .iter()
            .map(|m| (m, m.says(table)))
            .filter(|(m, says)| self.shows(m.context) && self.searched(&[m.gesture, says]))
            .collect()
    }

    /// What `key` does with the modifiers chosen on the page, in the
    /// contexts the chips let through: the line of the table and its command.
    fn actions_of(&self, table: &ShortcutTable, key: Key) -> Vec<(usize, Action)> {
        let keys = self.combination(key);
        table
            .lines()
            .iter()
            .enumerate()
            .filter(|(_, b)| b.keys == Some(keys) && self.shows(b.action.context()))
            .map(|(i, b)| (i, b.action))
            .collect()
    }

    /// The keys the search lights up: those of the commands it found, with
    /// the modifiers held on the page.
    fn lit(&self, table: &ShortcutTable) -> Vec<Key> {
        if self.search.trim().is_empty() {
            return Vec::new();
        }
        self.listed(table)
            .into_iter()
            .filter_map(|line| table.lines()[line].keys)
            .filter(|keys| *keys == self.combination(keys.key))
            .map(|keys| keys.key)
            .collect()
    }

    /// The keys pressed for a change under way: taken out of this frame's
    /// input so nothing else acts on them.
    fn pressed_keys(ui: &Ui) -> Option<Pressed> {
        ui.input_mut(|input| {
            let mut found = None;
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if found.is_none() => {
                    found = Some(Shortcut::held(physical_key.unwrap_or(*key), *modifiers));
                    false
                }
                _ => true,
            });
            found.map(|keys| {
                if keys == Shortcut::key(Key::Escape) {
                    Pressed::Cancel
                } else {
                    Pressed::Keys(keys)
                }
            })
        })
    }

    /// Draws the page. Shortcuts changed here are changed in `table`; the
    /// keyboard drawn is `keyboard`'s choice, `system` being what the
    /// operating system reports.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        table: &mut ShortcutTable,
        keyboard: &mut KeyboardChoice,
        system: &SystemKeyboard,
    ) -> PageResponse {
        let mut response = PageResponse::default();
        self.down = ui.input(|input| input.modifiers);
        if self.listening() {
            match Self::pressed_keys(ui) {
                Some(Pressed::Keys(keys)) => response.changed |= self.rebinding.offer(table, keys),
                Some(Pressed::Cancel) => self.rebinding.cancel(),
                None => {}
            }
        }

        self.top_row(ui, theme, table, keyboard, system, &mut response);
        let layout = keyboard.resolve(system);
        // Keys are written as the keyboard drawn prints them, here and everywhere.
        table.set_layout(layout);
        ui.add_space(4.0);
        self.chips(ui, theme);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Holding:");
            ui.toggle_value(&mut self.ctrl, "Ctrl");
            ui.toggle_value(&mut self.shift, "Shift");
            ui.toggle_value(&mut self.alt, "Alt");
            ui.label(
                RichText::new(
                    "Or hold them on your keyboard. Point at a key to see what it does; click to keep it shown.",
                )
                .color(theme.muted),
            );
        });
        ui.add_space(6.0);

        let hits = self.keyboard(ui, theme, table, layout.layout());
        if let Some(key) = hits.clicked {
            if self.listening() {
                // While changing a shortcut, a click on the drawing is the answer.
                response.changed |= self.rebinding.offer(table, self.combination(key));
            } else {
                self.pinned = if self.pinned == Some(key) { None } else { Some(key) };
            }
        }
        ui.add_space(4.0);
        response.changed |= self.banner(ui, theme, table, layout.layout(), hits.hovered);
        ui.add_space(6.0);

        // Under the keyboard: every command on the left, the mouse on the right.
        let below = ui.available_rect_before_wrap();
        let below = Rect::from_min_size(below.min, vec2(below.width(), below.height().max(160.0)));
        let split = below.left() + (below.width() * 0.6).round();
        let (left, right) = (
            Rect::from_min_max(below.min, pos2(split - 8.0, below.bottom())),
            Rect::from_min_max(pos2(split + 8.0, below.top()), below.max),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
            response.changed |= self.commands(ui, theme, table, layout.layout());
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
            self.mouse(ui, theme, table);
        });
        ui.allocate_rect(below, Sense::hover());
        response
    }

    /// Search, the keyboard layout, the cheat sheet and Reset all.
    fn top_row(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        table: &ShortcutTable,
        keyboard: &mut KeyboardChoice,
        system: &SystemKeyboard,
        response: &mut PageResponse,
    ) {
        ui.horizontal(|ui| {
            // While new keys are awaited the box is off, so they are not typed into it.
            ui.add_enabled(
                !self.listening(),
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Find a key or action")
                    .desired_width(220.0),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Keyboard").color(theme.muted));
            let before = *keyboard;
            let automatic = format!("Automatic: {}", keyboard.resolve(system).label());
            egui::ComboBox::from_id_salt("keyboard-layout")
                .selected_text(match keyboard {
                    KeyboardChoice::Automatic => automatic.clone(),
                    KeyboardChoice::Fixed(id) => id.label().to_owned(),
                })
                .width(200.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(keyboard, KeyboardChoice::Automatic, "Automatic (from the system)")
                        .on_hover_text("Follows the keyboard the system uses, switches included.");
                    ui.separator();
                    for id in LayoutId::ALL {
                        ui.selectable_value(keyboard, KeyboardChoice::Fixed(id), id.label());
                    }
                })
                .response
                .on_hover_text(
                    "The keyboard that is drawn and that menus and tips write keys for. Shortcuts stay on the same keys whatever is printed on them.",
                );
            response.changed |= *keyboard != before;
            // What the system reports, when it is not simply what is drawn.
            let note = match (*keyboard, system.layout) {
                (KeyboardChoice::Automatic, Some(_)) => None,
                (KeyboardChoice::Automatic, None) => Some(format!("System: {}", system.summary())),
                (KeyboardChoice::Fixed(id), found) if found != Some(id) => {
                    Some(format!("System: {}", system.summary()))
                }
                (KeyboardChoice::Fixed(_), _) => None,
            };
            if let Some(note) = note {
                ui.label(RichText::new(note).color(theme.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(table.changed(), egui::Button::new("Reset all…"))
                    .on_hover_text("Every shortcut back to its usual keys.")
                    .clicked()
                {
                    self.rebinding.cancel();
                    self.asking_reset_all = true;
                }
                if ui
                    .button("Cheat sheet…")
                    .on_hover_text("Saves a page to print: the keyboard and every shortcut as they are now.")
                    .clicked()
                {
                    response.cheat_sheet = true;
                }
            });
        });
    }

    /// The context chips: click to show only the picked contexts.
    fn chips(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            ui.label("Show:");
            for context in Context::ALL {
                let picked = self.filter.contains(&context);
                if chip(ui, theme, context, picked, !self.filter.is_empty()).clicked() {
                    self.toggle(context);
                }
            }
            if !self.filter.is_empty() && ui.small_button("Everything").clicked() {
                self.filter.clear();
            }
        });
    }

    /// The drawn keyboard.
    fn keyboard(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        table: &ShortcutTable,
        layout: &Layout,
    ) -> KeyboardHits {
        let unit = (ui.available_width() / layout::WIDTH).clamp(22.0, 46.0);
        let size = vec2(unit * layout::WIDTH, unit * layout::HEIGHT);
        // Centred when the window is wider than the keyboard needs.
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width().max(size.x), size.y), Sense::hover());
        let area = Rect::from_center_size(row.center(), size);
        let lit = self.lit(table);
        // Which of Ctrl, Shift and Alt count as held: toggled or pressed.
        let held = self.combination(Key::Space);
        let mut hits = KeyboardHits::default();
        for (index, cap) in layout.shape.caps().iter().enumerate() {
            let place = |r: Rect| {
                Rect::from_min_max(
                    area.min + r.min.to_vec2() * unit,
                    area.min + r.max.to_vec2() * unit,
                )
            };
            let bounds = cap.lower.map_or(cap.rect, |l| cap.rect.union(l));
            let reply = ui.interact(place(bounds), ui.scope_id().with(("key", index)), Sense::click());
            // A tall Enter's corner cell belongs to the key that sits in it.
            let on_key = reply
                .hover_pos()
                .is_some_and(|p| cap.contains(((p - area.min) / unit).to_pos2()));
            let hovered = reply.hovered() && on_key;
            let key = cap.spot.key();
            let mut contexts: Vec<Context> = Vec::new();
            for (_, action) in key.map_or_else(Vec::new, |k| self.actions_of(table, k)) {
                if !contexts.contains(&action.context()) {
                    contexts.push(action.context());
                }
            }
            let look = KeyLook {
                contexts,
                held: matches!(cap.spot, Spot::Ctrl if held.ctrl)
                    || matches!(cap.spot, Spot::Shift if held.shift)
                    || matches!(cap.spot, Spot::Alt if held.alt),
                marked: key.is_some_and(|k| self.pinned == Some(k) || lit.contains(&k)),
                hovered,
            };
            drawing::key(ui, theme, layout, cap, area.min, unit, &look);
            if hovered && let Some(key) = cap.spot.key() {
                hits.hovered = Some(key);
            }
            if reply.clicked() && on_key {
                match cap.spot {
                    Spot::Key(key) => hits.clicked = Some(key),
                    Spot::Ctrl => self.ctrl = !self.ctrl,
                    Spot::Shift => self.shift = !self.shift,
                    Spot::Alt => self.alt = !self.alt,
                    Spot::Extra | Spot::Yen | Spot::Ro | Spot::Plain(_) => {}
                }
            }
        }
        hits
    }

    /// The line under the keyboard: the change under way, the question
    /// before Reset all, or what the pointed-at or pinned key does.
    /// True when a shortcut changed.
    fn banner(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        table: &mut ShortcutTable,
        layout: &Layout,
        hovered: Option<Key>,
    ) -> bool {
        let mut changed = false;
        let frame = egui::Frame::new()
            .fill(theme.panel)
            .stroke(Stroke::new(1.0, theme.line))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6));
        frame.show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| match self.rebinding.state() {
                Some(State::Listening { line }) => {
                    let name = table.lines().get(line).map_or("", |b| b.action.label());
                    ui.label(
                        RichText::new(format!("New keys for \"{name}\":")).family(crate::fonts::semibold()),
                    );
                    ui.label("press them, or click a key on the drawing.");
                    ui.label(RichText::new("Escape cancels.").color(theme.muted));
                    if ui.button("No shortcut").clicked() {
                        changed |= self.rebinding.clear(table);
                    }
                    if ui.button("Cancel").clicked() {
                        self.rebinding.cancel();
                    }
                }
                Some(State::Clash { line, keys, with }) => {
                    ui.label(
                        RichText::new(clash_text(table, layout, line, keys, with.action))
                            .color(crate::status::WARNING),
                    );
                    ui.label(swap_text(table, line, with.action));
                    if ui.button("Swap").clicked() {
                        changed |= self.rebinding.swap(table);
                    }
                    if ui.button("Cancel").clicked() {
                        self.rebinding.cancel();
                    }
                    ui.label(RichText::new("Or press other keys.").color(theme.muted));
                }
                None if self.asking_reset_all => {
                    ui.label("Put every shortcut back to its usual keys?");
                    if ui.button("Reset all").clicked() {
                        table.reset_all();
                        self.asking_reset_all = false;
                        changed = true;
                    }
                    if ui.button("Keep them").clicked() {
                        self.asking_reset_all = false;
                    }
                }
                None => match hovered.or(self.pinned) {
                    Some(key) => self.key_line(ui, theme, table, layout, key),
                    None => {
                        ui.label(
                            RichText::new("To change a shortcut, click its keys in the list below.")
                                .color(theme.muted),
                        );
                    }
                },
            });
        });
        changed
    }

    /// What one key does with the held modifiers, with a way to change it.
    fn key_line(&mut self, ui: &mut Ui, theme: &Theme, table: &ShortcutTable, layout: &Layout, key: Key) {
        let keys = layout.describe(self.combination(key));
        let actions = self.actions_of(table, key);
        if actions.is_empty() {
            ui.label(RichText::new(format!("{keys}:")).family(crate::fonts::semibold()));
            ui.label(RichText::new("nothing yet").color(theme.muted));
        }
        for (line, action) in actions {
            ui.label(RichText::new(format!("{keys}:")).family(crate::fonts::semibold()));
            dot(ui, action.context().colour());
            ui.label(action.label());
            if self.pinned == Some(key) && ui.button("Change…").clicked() {
                self.begin(ui, line);
            }
        }
    }

    /// Starts changing `line`: keys pressed from now on are the answer, so
    /// no text field keeps them.
    fn begin(&mut self, ui: &Ui, line: usize) {
        self.asking_reset_all = false;
        self.rebinding.begin(line);
        ui.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
    }

    /// Every command with its keys, by context. Click the keys to change
    /// them. True when a shortcut changed.
    fn commands(&mut self, ui: &mut Ui, theme: &Theme, table: &mut ShortcutTable, layout: &Layout) -> bool {
        let mut changed = false;
        let listed = self.listed(table);
        let changing = self.rebinding.state().map(|s| s.line());
        ui.label(RichText::new("Keys").family(crate::fonts::semibold()));
        egui::ScrollArea::vertical()
            .id_salt("shortcut-commands")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if listed.is_empty() {
                    ui.label(RichText::new("No command here matches.").color(theme.muted));
                    return;
                }
                // The names take the room the keys and Reset leave.
                let (keys_width, reset_width) = (190.0, 52.0);
                let name_width = (ui.available_width() - keys_width - reset_width - 28.0).max(160.0);
                egui::Grid::new("all-shortcuts")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        let mut heading = None;
                        for line in listed {
                            let binding: Binding = table.lines()[line];
                            let context = binding.action.context();
                            if heading != Some(context) {
                                heading = Some(context);
                                ui.horizontal(|ui| {
                                    dot(ui, context.colour());
                                    ui.label(RichText::new(context.label()).color(theme.muted));
                                });
                                ui.end_row();
                            }
                            ui.horizontal(|ui| {
                                ui.set_min_width(name_width);
                                ui.add(egui::Label::new(binding.action.label()).truncate());
                            });
                            let text = match (changing == Some(line), binding.keys) {
                                (true, _) => RichText::new("press keys…").color(theme.accent),
                                (false, Some(keys)) => {
                                    RichText::new(layout.describe(keys)).family(egui::FontFamily::Monospace)
                                }
                                (false, None) => RichText::new("none").color(theme.muted),
                            };
                            let button = ui
                                .add(
                                    egui::Button::new(text)
                                        .fill(theme.panel)
                                        .stroke(Stroke::new(
                                            1.0,
                                            if changing == Some(line) {
                                                theme.accent
                                            } else {
                                                theme.line_strong
                                            },
                                        ))
                                        .min_size(vec2(keys_width, 0.0)),
                                )
                                .on_hover_text("Click, then press the new keys.");
                            if button.clicked() {
                                self.begin(ui, line);
                            }
                            if binding.changed() {
                                let usual = binding
                                    .usual()
                                    .map_or_else(|| "no shortcut".to_owned(), |k| layout.written(k));
                                if ui
                                    .small_button("Reset")
                                    .on_hover_text(format!("Back to the usual: {usual}."))
                                    .clicked()
                                {
                                    self.asking_reset_all = false;
                                    changed |= self.rebinding.reset(table, line);
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
            });
        changed
    }

    /// The Mouse section: what the mouse does, by context.
    fn mouse(&self, ui: &mut Ui, theme: &Theme, table: &ShortcutTable) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Mouse").family(crate::fonts::semibold()));
            ui.label(RichText::new("shown here; it can't be changed").color(theme.muted));
        });
        egui::ScrollArea::vertical()
            .id_salt("shortcut-mouse")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let listed = self.listed_mouse(table);
                if listed.is_empty() {
                    ui.label(RichText::new("Nothing for the mouse here.").color(theme.muted));
                }
                let mut heading = None;
                for (action, says) in listed {
                    if heading != Some(action.context) {
                        heading = Some(action.context);
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            dot(ui, action.context.colour());
                            ui.label(RichText::new(action.context.label()).color(theme.muted));
                        });
                    }
                    ui.label(action.gesture);
                    ui.label(RichText::new(says).color(theme.text_secondary).size(12.0));
                    ui.add_space(3.0);
                }
            });
    }
}

/// "Ctrl+S is already Save (Anywhere)."
fn clash_text(table: &ShortcutTable, layout: &Layout, line: usize, keys: Shortcut, holder: Action) -> String {
    let own = table.lines().get(line).map(|b| b.action);
    if own == Some(holder) {
        format!(
            "{} is already this command's other shortcut.",
            layout.describe(keys)
        )
    } else {
        format!(
            "{} is already \"{}\" ({}).",
            layout.describe(keys),
            holder.label(),
            holder.context().label()
        )
    }
}

/// What Swap would do to the command that has the keys.
fn swap_text(table: &ShortcutTable, line: usize, holder: Action) -> String {
    match table.lines().get(line).and_then(|b| b.keys) {
        Some(old) => format!(
            "Swap gives \"{}\" {} instead.",
            holder.label(),
            table.written(old)
        ),
        None => format!("Swap leaves \"{}\" without a shortcut.", holder.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(table: &ShortcutTable, action: Action) -> usize {
        table.lines().iter().position(|b| b.action == action).unwrap()
    }

    #[test]
    fn every_bound_key_is_on_every_drawing() {
        for id in LayoutId::ALL {
            let drawn: Vec<Key> = id
                .layout()
                .shape
                .caps()
                .iter()
                .filter_map(|c| c.spot.key())
                .collect();
            for (action, shortcut) in ShortcutTable::default().bindings() {
                assert!(
                    drawn.contains(&shortcut.key),
                    "{action:?} on {:?} can't be seen on {id:?}",
                    shortcut.key
                );
            }
        }
    }

    #[test]
    fn the_page_finds_what_a_key_does() {
        let table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        let actions = |page: &ShortcutsPage, key| -> Vec<Action> {
            page.actions_of(&table, key).into_iter().map(|(_, a)| a).collect()
        };
        assert_eq!(actions(&page, Key::Space), [Action::PlayPause]);
        page.ctrl = true;
        assert_eq!(actions(&page, Key::S), [Action::Save]);
    }

    #[test]
    fn modifiers_held_on_the_real_keyboard_count_like_the_toggles() {
        let table = ShortcutTable::default();
        let mut page = ShortcutsPage {
            down: egui::Modifiers::COMMAND,
            ..Default::default()
        };
        assert_eq!(page.combination(Key::S), Shortcut::ctrl(Key::S));
        assert_eq!(page.actions_of(&table, Key::S)[0].1, Action::Save);
        page.shift = true;
        assert_eq!(page.combination(Key::Z), Shortcut::ctrl_shift(Key::Z));

        // The page reads them from the frame's input.
        let mut page = ShortcutsPage::default();
        let mut table = ShortcutTable::default();
        frame(
            &mut page,
            &mut table,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::ALT)],
        );
        assert_eq!(page.combination(Key::Minus), Shortcut::alt(Key::Minus));
    }

    #[test]
    fn the_page_shows_a_changed_shortcut_on_its_new_key() {
        let mut table = ShortcutTable::default();
        let page = ShortcutsPage::default();
        table.swap(line(&table, Action::Split), Shortcut::key(Key::F9));
        assert!(page.actions_of(&table, Key::S).is_empty());
        assert_eq!(
            page.actions_of(&table, Key::F9),
            [(line(&table, Action::Split), Action::Split)]
        );
    }

    #[test]
    fn chips_filter_the_keyboard_the_list_and_the_mouse() {
        let table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        let everything = page.listed(&table).len();
        assert_eq!(everything, table.lines().len());
        assert_eq!(page.listed_mouse(&table).len(), MOUSE.len());

        page.toggle(Context::Timeline);
        let listed = page.listed(&table);
        assert!(!listed.is_empty() && listed.len() < everything);
        assert!(
            listed
                .iter()
                .all(|l| table.lines()[*l].action.context() == Context::Timeline)
        );
        assert!(
            page.listed_mouse(&table)
                .iter()
                .all(|(m, _)| m.context == Context::Timeline)
        );
        assert_eq!(
            page.actions_of(&table, Key::S).len(),
            1,
            "Split is on the timeline"
        );
        assert!(
            page.actions_of(&table, Key::Space).is_empty(),
            "Play / pause is the viewer's"
        );

        // A second chip adds its context.
        page.toggle(Context::Viewer);
        assert_eq!(page.actions_of(&table, Key::Space).len(), 1);
        assert!(page.listed(&table).len() > listed.len());
        page.ctrl = true;
        assert!(
            page.actions_of(&table, Key::S).is_empty(),
            "Save works anywhere: filtered out"
        );

        // Unpicking both shows everything again.
        page.toggle(Context::Timeline);
        page.toggle(Context::Viewer);
        assert_eq!(page.listed(&table).len(), everything);
        assert_eq!(page.actions_of(&table, Key::S).len(), 1);

        // "While dragging" has mouse actions only.
        page.toggle(Context::Dragging);
        assert!(page.listed(&table).is_empty());
        assert!(!page.listed_mouse(&table).is_empty());
    }

    #[test]
    fn search_narrows_the_list_and_lights_the_keys() {
        let mut table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        assert!(page.lit(&table).is_empty(), "nothing searched, nothing lit");

        // ("split" alone finds Split view too.)
        page.search = "split at".into();
        assert_eq!(page.listed(&table), [line(&table, Action::Split)]);
        assert_eq!(page.lit(&table), [Key::S]);
        // The search follows a changed shortcut.
        table.swap(line(&table, Action::Split), Shortcut::key(Key::F9));
        assert_eq!(page.lit(&table), [Key::F9]);

        // By keys, too; a key only lights with the modifiers that are held.
        page.search = "ctrl+shift+z".into();
        assert_eq!(page.listed(&table).len(), 1);
        assert!(page.lit(&table).is_empty());
        page.ctrl = true;
        page.shift = true;
        assert_eq!(page.lit(&table), [Key::Z]);

        // Chips and search together.
        page.search = "tool".into();
        page.toggle(Context::Anywhere);
        assert!(page.listed(&table).is_empty());

        // The mouse is searched as well.
        let page = ShortcutsPage {
            search: "wheel".into(),
            ..Default::default()
        };
        assert!(!page.listed_mouse(&table).is_empty());
        assert!(page.listed_mouse(&table).len() < MOUSE.len());
    }

    #[test]
    fn the_page_says_who_has_the_keys_and_what_a_swap_does() {
        let mut table = ShortcutTable::default();
        let us = LayoutId::Us.layout();
        let split = line(&table, Action::Split);
        assert_eq!(
            clash_text(&table, us, split, Shortcut::ctrl(Key::S), Action::Save),
            "Ctrl+S is already \"Save\" (Anywhere)."
        );
        assert_eq!(
            swap_text(&table, split, Action::Save),
            "Swap gives \"Save\" S instead."
        );
        table.clear(split);
        assert_eq!(
            swap_text(&table, split, Action::Save),
            "Swap leaves \"Save\" without a shortcut."
        );
        let redo = line(&table, Action::Redo);
        assert_eq!(
            clash_text(&table, us, redo, Shortcut::ctrl(Key::Y), Action::Redo),
            "Ctrl+Y is already this command's other shortcut."
        );
    }

    #[test]
    fn previews_open_the_page_in_each_state_without_changing_the_table() {
        let table = ShortcutTable::default();
        assert_eq!(Preview::from_name("clash"), Some(Preview::Clash));
        assert_eq!(Preview::from_name("nothing"), None);

        let page = ShortcutsPage::preview(Preview::Filter, &table);
        assert_eq!(page.filter, [Context::Timeline]);
        assert!(!page.listening());

        let page = ShortcutsPage::preview(Preview::Rebinding, &table);
        let split = line(&table, Action::Split);
        assert_eq!(page.rebinding.state(), Some(State::Listening { line: split }));
        assert!(page.listening());

        let page = ShortcutsPage::preview(Preview::Clash, &table);
        assert!(matches!(
            page.rebinding.state(),
            Some(State::Clash { with, .. }) if with.action == Action::Save
        ));
    }

    /// Runs the page for one frame with `events` and returns what it asked for.
    fn frame(page: &mut ShortcutsPage, table: &mut ShortcutTable, events: Vec<egui::Event>) -> PageResponse {
        let ctx = egui::Context::default();
        let mut response = PageResponse::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut keyboard = KeyboardChoice::Fixed(LayoutId::Us);
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                // A frame may be laid out twice; what it asked for adds up.
                let asked = page.show(ui, &Theme::ASH, table, &mut keyboard, &SystemKeyboard::default());
                response.changed |= asked.changed;
                response.cheat_sheet |= asked.cheat_sheet;
            });
        });
        // Nothing draws these frames.
        output.drop_without_applying_deltas();
        response
    }

    fn press(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn pressing_keys_while_the_page_listens_changes_the_shortcut() {
        let mut table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        // Not listening: keys are not the page's.
        let response = frame(&mut page, &mut table, vec![press(Key::F9, egui::Modifiers::NONE)]);
        assert!(!response.changed);
        assert_eq!(table, ShortcutTable::default());

        page.rebinding.begin(line(&table, Action::Split));
        let response = frame(
            &mut page,
            &mut table,
            vec![press(Key::B, egui::Modifiers::COMMAND)],
        );
        assert!(response.changed);
        assert!(!page.listening());
        assert_eq!(table.shortcut_for(Action::Split), Some(Shortcut::ctrl(Key::B)));
    }

    #[test]
    fn pressing_taken_keys_waits_and_escape_cancels() {
        let mut table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        page.rebinding.begin(line(&table, Action::Split));
        let response = frame(
            &mut page,
            &mut table,
            vec![press(Key::S, egui::Modifiers::COMMAND)],
        );
        assert!(!response.changed);
        assert!(matches!(page.rebinding.state(), Some(State::Clash { .. })));
        assert_eq!(table, ShortcutTable::default());

        let response = frame(
            &mut page,
            &mut table,
            vec![press(Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(!response.changed);
        assert!(!page.listening());
        assert_eq!(
            table,
            ShortcutTable::default(),
            "Escape is never taken as the new keys"
        );
    }

    #[test]
    fn the_key_position_counts_not_the_character() {
        // A layout where the key at the S position types something else.
        let mut table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        page.rebinding.begin(line(&table, Action::AddLabel));
        let event = egui::Event::Key {
            key: Key::O,
            physical_key: Some(Key::F8),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(frame(&mut page, &mut table, vec![event]).changed);
        assert_eq!(table.shortcut_for(Action::AddLabel), Some(Shortcut::key(Key::F8)));
    }

    /// A window that stays open between frames, for clicks.
    struct Rig {
        ctx: egui::Context,
        page: ShortcutsPage,
        table: ShortcutTable,
    }

    impl Rig {
        fn new() -> Rig {
            Rig {
                ctx: egui::Context::default(),
                page: ShortcutsPage::default(),
                table: ShortcutTable::default(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> bool {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 900.0))),
                events,
                ..Default::default()
            };
            let mut changed = false;
            let mut keyboard = KeyboardChoice::Fixed(LayoutId::Uk);
            let output = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    changed |= self
                        .page
                        .show(
                            ui,
                            &Theme::ASH,
                            &mut self.table,
                            &mut keyboard,
                            &SystemKeyboard::default(),
                        )
                        .changed;
                });
            });
            output.drop_without_applying_deltas();
            changed
        }

        /// Clicks at `at`; true when a shortcut changed.
        fn click(&mut self, at: egui::Pos2) -> bool {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut changed = self.frame(vec![egui::Event::PointerMoved(at)]);
            changed |= self.frame(vec![button(true)]);
            changed | self.frame(vec![button(false)])
        }

        /// Where the drawn key `key` can be clicked: found by clicking
        /// around until that key is pinned.
        fn find(&mut self, key: Key) -> egui::Pos2 {
            for y in (100..500).step_by(12) {
                for x in (10..1190).step_by(12) {
                    let at = pos2(x as f32, y as f32);
                    self.click(at);
                    let pinned = self.page.pinned.take();
                    if pinned == Some(key) {
                        return at;
                    }
                }
            }
            panic!("{key:?} is not on the drawing");
        }
    }

    #[test]
    fn clicking_a_drawn_key_pins_it_or_answers_a_change_under_way() {
        let mut rig = Rig::new();
        let (f9, s) = (rig.find(Key::F9), rig.find(Key::S));
        assert!(!rig.table.changed(), "pinning changes nothing");

        // Not changing anything: a click pins the key, another lets go.
        assert!(!rig.click(f9));
        assert_eq!(rig.page.pinned, Some(Key::F9));
        assert!(!rig.click(f9));
        assert_eq!(rig.page.pinned, None);

        // Changing a shortcut: a click on a free key is the answer.
        let label = line(&rig.table, Action::AddLabel);
        rig.page.rebinding.begin(label);
        assert!(rig.click(f9));
        assert_eq!(
            rig.table.shortcut_for(Action::AddLabel),
            Some(Shortcut::key(Key::F9))
        );
        assert_eq!(rig.page.pinned, None, "the click was not a pin");

        // A click on a taken key is a clash, with the modifiers held on the page.
        rig.page.rebinding.begin(label);
        assert!(!rig.click(s));
        assert!(matches!(
            rig.page.rebinding.state(),
            Some(State::Clash { keys, with, .. })
                if keys == Shortcut::key(Key::S) && with.action == Action::Split
        ));
        rig.page.ctrl = true;
        assert!(!rig.click(s));
        assert!(matches!(
            rig.page.rebinding.state(),
            Some(State::Clash { keys, with, .. })
                if keys == Shortcut::ctrl(Key::S) && with.action == Action::Save
        ));
        // Swap, from the state the click left.
        assert!(rig.page.rebinding.swap(&mut rig.table));
        assert_eq!(
            rig.table.shortcut_for(Action::AddLabel),
            Some(Shortcut::ctrl(Key::S))
        );
        assert_eq!(rig.table.shortcut_for(Action::Save), Some(Shortcut::key(Key::F9)));
    }

    #[test]
    fn the_corner_under_a_tall_enter_belongs_to_the_key_in_it() {
        // On an ISO keyboard the key left of Enter's lower part sits inside
        // Enter's bounding box: clicking it must not hit Enter.
        let mut rig = Rig::new();
        let (enter, hash) = (rig.find(Key::Enter), rig.find(Key::Backslash));
        rig.click(hash);
        assert_eq!(rig.page.pinned, Some(Key::Backslash));
        rig.click(enter);
        assert_eq!(rig.page.pinned, Some(Key::Enter));
    }

    #[test]
    fn every_layout_and_state_draws() {
        for id in LayoutId::ALL {
            for preview in [Preview::Filter, Preview::Rebinding, Preview::Clash] {
                let mut table = ShortcutTable::default();
                let mut page = ShortcutsPage::preview(preview, &table);
                page.pinned = Some(Key::S);
                let ctx = egui::Context::default();
                let mut keyboard = KeyboardChoice::Fixed(id);
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        page.show(
                            ui,
                            &Theme::CARBON,
                            &mut table,
                            &mut keyboard,
                            &SystemKeyboard::default(),
                        );
                    });
                });
                output.drop_without_applying_deltas();
                assert_eq!(keyboard, KeyboardChoice::Fixed(id));
                assert_eq!(table.layout(), id);
            }
        }
    }

    #[test]
    fn automatic_draws_what_the_system_reports_and_writes_keys_for_it() {
        let mut table = ShortcutTable::default();
        let mut page = ShortcutsPage::default();
        let mut keyboard = KeyboardChoice::Automatic;
        let mut show = |system: &SystemKeyboard, table: &mut ShortcutTable| {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    page.show(ui, &Theme::ASH, table, &mut keyboard, system);
                });
            });
            output.drop_without_applying_deltas();
        };
        show(&SystemKeyboard::from_xkb_name("no"), &mut table);
        assert_eq!(table.layout(), LayoutId::Norwegian);
        assert_eq!(table.keys_for(Action::KeyboardShortcuts), "Ctrl+-");
        // A switch on the system follows at the next frame.
        show(&SystemKeyboard::from_windows_language(0x0411), &mut table);
        assert_eq!(table.layout(), LayoutId::Japanese);
        // One the drawing doesn't know: US English.
        show(&SystemKeyboard::from_xkb_name("ru"), &mut table);
        assert_eq!(table.layout(), LayoutId::Us);
    }

    #[test]
    fn the_list_is_searched_by_what_the_keyboard_prints() {
        let mut table = ShortcutTable::default();
        table.set_layout(LayoutId::Norwegian);
        let page = ShortcutsPage {
            search: "ctrl+-".into(),
            ..Default::default()
        };
        assert_eq!(page.listed(&table), [line(&table, Action::KeyboardShortcuts)]);
    }
}
