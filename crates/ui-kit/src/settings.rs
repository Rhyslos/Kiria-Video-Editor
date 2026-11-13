//! Settings, drawn one way in every program: a window under the top bar
//! with the categories as tabs across its top, a search box at the right
//! end of the tabs, and the chosen category below in cards (a titled
//! section on a raised surface with a rounded edge), one row per setting:
//! the label with a line of description under it on the left, the control
//! on the right, all controls in one column, thin dividers between rows.
//!
//! A program describes what it has as data ([`Category`], [`Section`],
//! [`Setting`]) and draws each row's control when asked ([`Row`]), with the
//! controls in [`controls`]. Search and [`SettingsView::open_at`] work on the
//! data, so other parts (the editor's Ctrl+K search) can open Settings at
//! any setting by its id. Changes go wherever the program keeps its
//! settings; nothing here stores them.

mod catalogue;
pub mod controls;

pub use catalogue::{Category, Place, Section, Setting, found, place_of, repeated_ids, search};
pub use controls::{action, auto_number, dropdown, number, segmented, toggle, value};

use crate::Theme;
use crate::tab_strip::{StripTab, tab_rows};
use egui::{Align, FontId, Id, Layout, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

/// The largest Settings gets; smaller screens get the room there is.
pub const MAX_SIZE: egui::Vec2 = vec2(1080.0, 820.0);
/// How wide the column of cards is at most, so a label and its control
/// stay close enough to read as one row.
pub const COLUMN_WIDTH: f32 = 820.0;
/// The search box at the end of the tabs.
const SEARCH_WIDTH: f32 = 220.0;
/// The height of a row of tabs.
const TAB_ROW: f32 = 40.0;
/// Room inside a row around its text and control.
const ROW_PAD: egui::Vec2 = vec2(16.0, 11.0);
/// At most this share of a row goes to its control.
const CONTROL_SHARE: f32 = 0.58;
/// The flash on a row that was jumped to, in seconds.
const FLASH: f64 = 1.6;

/// What a program's Settings window remembers between frames: the tab
/// open, what is typed in the search box and a row to jump to.
#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    tab: usize,
    query: String,
    /// Scroll this row into view on the next frame it is drawn.
    jump: Option<&'static str>,
    /// This row is lit, fading out, since this time (`ui.input` time); set
    /// when the row is first drawn after a jump.
    flash: Option<(&'static str, Option<f64>)>,
    focus_search: bool,
}

/// What a row's drawing hands back to the view, besides the control itself.
pub struct Row<'a> {
    /// Where the control goes: a column laid out from the right.
    pub ui: &'a mut Ui,
    pub theme: &'a Theme,
    pub setting: &'static Setting,
    describe: Option<String>,
    note: Option<String>,
    disabled: Option<&'a str>,
}

impl std::fmt::Debug for Row<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("setting", &self.setting.id)
            .finish_non_exhaustive()
    }
}

impl<'a> Row<'a> {
    /// The setting's id, to match on.
    pub fn id(&self) -> &'static str {
        self.setting.id
    }

    /// Shows `words` under the label instead of the setting's description
    /// (such as what the value chosen now does).
    pub fn describe(&mut self, words: impl Into<String>) {
        self.describe = Some(words.into());
    }

    /// A second quiet line under the description (such as what is in use now).
    pub fn note(&mut self, words: impl Into<String>) {
        self.note = Some(words.into());
    }

    /// Greys the row out and its control off, `why` shown when pointed at.
    pub fn disable(&mut self, why: &'a str) {
        self.ui.disable();
        self.disabled = Some(why);
    }
}

impl SettingsView {
    /// Opens the category, section or setting `id` names: its tab, scrolled
    /// to it, lit for a moment. Clears the search. Returns false (and
    /// changes nothing) when no such id is in `categories`.
    pub fn open_at(&mut self, categories: &[Category], id: &str) -> bool {
        let Some(place) = place_of(categories, id) else {
            return false;
        };
        self.go_to(categories, place);
        true
    }

    /// The tab open now, as an index into the categories.
    pub fn tab(&self) -> usize {
        self.tab
    }

    /// What is typed in the search box.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Types `query` into the search box.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }

    /// Puts the keyboard in the search box on the next frame.
    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    /// The row being jumped to, until it has been scrolled into view.
    pub fn jumping_to(&self) -> Option<&'static str> {
        self.jump
    }

    fn go_to(&mut self, categories: &[Category], place: Place) {
        let Some(setting) = place.setting(categories) else {
            return;
        };
        self.tab = place.category;
        self.query.clear();
        self.jump = Some(setting.id);
        self.flash = Some((setting.id, None));
    }

    /// Draws the whole view in `ui`: the tabs and search box across the top,
    /// the cards below. `draw` adds each row's control (see [`Row`]).
    /// Returns true when the close button was clicked.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        categories: &'static [Category],
        mut draw: impl FnMut(&mut Row<'_>),
    ) -> bool {
        self.tab = self.tab.min(categories.len().saturating_sub(1));
        let whole = ui.available_rect_before_wrap();
        let mut closed = false;

        // The title and the close button.
        let title = Rect::from_min_size(whole.min, vec2(whole.width(), 52.0));
        let mut top = ui.new_child(
            UiBuilder::new()
                .max_rect(title.shrink2(vec2(20.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        top.label(
            egui::RichText::new("Settings")
                .font(FontId::new(17.0, crate::fonts::semibold()))
                .color(theme.text),
        );
        top.with_layout(Layout::right_to_left(Align::Center), |ui| {
            closed = controls::close_button(ui, theme).clicked();
        });

        // The tabs, and the search box at their right end.
        let found = search(categories, &self.query);
        let searching = !self.query.trim().is_empty();
        // Laid out by hand: the search box a fixed width at the right end
        // of the first row, the tabs in what is left, in as many rows as
        // they need, so the two never overlap and every tab is in sight.
        let first_row = Rect::from_min_size(pos2(whole.left(), title.bottom()), vec2(whole.width(), TAB_ROW));
        let inner = first_row.shrink2(vec2(14.0, 0.0));
        let search_rect = Rect::from_min_size(
            pos2(inner.right() - SEARCH_WIDTH, inner.center().y - 12.0),
            vec2(SEARCH_WIDTH, 30.0),
        );
        let mut search_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(search_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let enter = self.search_box(&mut search_ui, theme);
        let counts: Vec<usize> = (0..categories.len())
            .map(|c| found.iter().filter(|p| p.category == c).count())
            .collect();
        // Searching, only the tabs with something found are shown, each
        // with how many.
        let shown: Vec<usize> = (0..categories.len())
            .filter(|c| !searching || counts[*c] > 0)
            .collect();
        let tabs: Vec<StripTab<'_>> = shown
            .iter()
            .map(|&c| StripTab {
                text: categories[c].label,
                count: searching.then_some(counts[c]),
                selected: !searching && c == self.tab,
            })
            .collect();
        // A tab is 31 points tall; a row leaves room around it.
        let strip_room = Rect::from_min_max(
            pos2(inner.left(), inner.top() + (TAB_ROW - 31.0) / 2.0),
            pos2(search_rect.left() - 16.0, whole.bottom()),
        );
        let mut strip_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(strip_room)
                .layout(Layout::left_to_right(Align::Min)),
        );
        let responses = tab_rows(&mut strip_ui, theme, &tabs);
        let tabs_bottom = strip_ui.min_rect().bottom() + (TAB_ROW - 31.0) / 2.0;
        let bar = Rect::from_min_max(
            first_row.min,
            pos2(whole.right(), tabs_bottom.max(first_row.bottom())),
        );
        for (&c, response) in shown.iter().zip(&responses) {
            if response.clicked() {
                // Searching, a tab goes to its first match; otherwise it opens.
                match found.iter().find(|p| p.category == c) {
                    Some(place) if searching => self.go_to(categories, *place),
                    _ => {
                        self.tab = c;
                        self.query.clear();
                    }
                }
            }
        }
        if enter && let Some(first) = found.first() {
            self.go_to(categories, *first);
        }
        ui.painter()
            .hline(whole.x_range(), bar.bottom(), Stroke::new(1.0, theme.line));

        // The cards.
        let body = Rect::from_min_max(pos2(whole.left(), bar.bottom() + 1.0), whole.max);
        let mut body_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        let searching = !self.query.trim().is_empty();
        egui::ScrollArea::vertical()
            .id_salt(("settings-body", if searching { usize::MAX } else { self.tab }))
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| {
                // The column starts under the first tab, so the page reads
                // down one edge.
                let room = ui.available_width();
                let left = 24.0;
                let column = (room - 2.0 * left).min(COLUMN_WIDTH);
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(left);
                    ui.vertical(|ui| {
                        ui.set_width(column);
                        if searching {
                            self.results(ui, theme, categories, &found, &mut draw);
                        } else if let Some(category) = categories.get(self.tab) {
                            self.category(ui, theme, category, &mut draw);
                        }
                        ui.add_space(28.0);
                    });
                });
            });
        // Room taken without an interaction of its own, which would sit
        // over the controls.
        ui.advance_cursor_after_rect(whole);
        closed
    }

    /// The search box. Returns true when Enter was pressed in it.
    fn search_box(&mut self, ui: &mut Ui, theme: &Theme) -> bool {
        let edit = egui::TextEdit::singleline(&mut self.query)
            .id_salt("settings-search")
            .hint_text("Search settings")
            .desired_width(SEARCH_WIDTH - 46.0)
            .frame(egui::Frame::NONE);
        let mut enter = false;
        egui::Frame::new()
            .fill(theme.panel)
            .stroke(Stroke::new(1.0, theme.line_strong))
            .corner_radius(7)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(SEARCH_WIDTH - 18.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⌕").size(14.0).color(theme.muted));
                    let response = ui.add(edit);
                    if std::mem::take(&mut self.focus_search) {
                        response.request_focus();
                    }
                    enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                })
            });
        enter
    }

    /// One category: its name, then its sections.
    fn category(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        category: &'static Category,
        draw: &mut impl FnMut(&mut Row<'_>),
    ) {
        ui.label(
            egui::RichText::new(category.label)
                .font(FontId::new(20.0, crate::fonts::semibold()))
                .color(theme.text),
        );
        ui.add_space(14.0);
        for section in category.sections {
            let rows: Vec<&'static Setting> = section.settings.iter().collect();
            self.section(ui, theme, section.title, &rows, draw);
        }
    }

    /// What the search found, under each category's name, in its sections.
    fn results(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        categories: &'static [Category],
        found: &[Place],
        draw: &mut impl FnMut(&mut Row<'_>),
    ) {
        if found.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("No setting matches “{}”.", self.query.trim()))
                        .size(14.0)
                        .color(theme.text_secondary),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Try another word, or fewer words.")
                        .size(12.0)
                        .color(theme.muted),
                );
            });
            return;
        }
        for (c, category) in categories.iter().enumerate() {
            if !found.iter().any(|p| p.category == c) {
                continue;
            }
            ui.label(
                egui::RichText::new(category.label)
                    .font(FontId::new(15.0, crate::fonts::semibold()))
                    .color(theme.text),
            );
            ui.add_space(10.0);
            for (s, section) in category.sections.iter().enumerate() {
                let rows: Vec<&'static Setting> = found
                    .iter()
                    .filter(|p| p.category == c && p.section == s)
                    .filter_map(|p| section.settings.get(p.setting))
                    .collect();
                if !rows.is_empty() {
                    self.section(ui, theme, section.title, &rows, draw);
                }
            }
            ui.add_space(6.0);
        }
    }

    /// One card: the section's title above it, its rows inside with a thin
    /// line between each two.
    fn section(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        title: &str,
        rows: &[&'static Setting],
        draw: &mut impl FnMut(&mut Row<'_>),
    ) {
        ui.label(
            egui::RichText::new(title)
                .font(FontId::new(13.0, crate::fonts::semibold()))
                .color(theme.text_secondary),
        );
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(theme.card)
            .stroke(Stroke::new(1.0, theme.card_edge))
            .corner_radius(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, setting) in rows.iter().enumerate() {
                    if i > 0 {
                        let y = ui.cursor().top();
                        let x = ui.max_rect().x_range();
                        ui.painter().hline(
                            (x.min + ROW_PAD.x)..=(x.max - ROW_PAD.x),
                            y,
                            Stroke::new(1.0, theme.divider),
                        );
                    }
                    self.row(ui, theme, setting, draw);
                }
            });
        ui.add_space(22.0);
    }

    /// One row: the control drawn first, in a column from the right; then
    /// the label, its badge and its description in the room left of it.
    fn row(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        setting: &'static Setting,
        draw: &mut impl FnMut(&mut Row<'_>),
    ) {
        let width = ui.available_width();
        let top = ui.cursor().min;
        let inner = width - 2.0 * ROW_PAD.x;
        let height_id = Id::unique(("settings-row-height", setting.id));
        // The control is centred on the row's height as last drawn.
        let last = ui
            .data(|d| d.get_temp::<f32>(height_id))
            .unwrap_or(CONTROL_HEIGHT_GUESS);
        let backdrop = ui.painter().add(egui::Shape::Noop);

        let control_room = Rect::from_min_size(
            top + ROW_PAD + vec2(inner * (1.0 - CONTROL_SHARE), 0.0),
            vec2(inner * CONTROL_SHARE, last),
        );
        let mut control_ui = ui.new_child(
            UiBuilder::new()
                .id_salt(("settings-row", setting.id))
                .max_rect(control_room)
                .layout(Layout::right_to_left(Align::Center)),
        );
        control_ui.spacing_mut().item_spacing.x = 8.0;
        // Buttons, boxes and dropdowns sunk into the card like the
        // segmented control's track, edged so they read as controls.
        {
            let widgets = &mut control_ui.visuals_mut().widgets;
            for (w, fill, edge) in [
                (&mut widgets.inactive, theme.bg, theme.line_strong),
                (&mut widgets.hovered, theme.raised, theme.line_strong),
                (&mut widgets.active, theme.active, theme.line_strong),
                (&mut widgets.open, theme.raised, theme.line_strong),
            ] {
                w.bg_fill = fill;
                w.weak_bg_fill = fill;
                w.bg_stroke = Stroke::new(1.0, edge);
            }
        }
        let (describe, note, disabled) = {
            let mut row = Row {
                ui: &mut control_ui,
                theme,
                setting,
                describe: None,
                note: None,
                disabled: None,
            };
            draw(&mut row);
            (row.describe, row.note, row.disabled.map(str::to_owned))
        };
        let control = control_ui.min_rect();
        let control_width = if control.is_positive() {
            control.width()
        } else {
            0.0
        };

        // The words, in the room the control leaves.
        let text_width = (inner - control_width - 24.0).max(140.0);
        let (label_colour, words_colour) = if disabled.is_some() {
            (theme.muted, theme.faint)
        } else {
            (theme.text, theme.muted)
        };
        let painter = ui.painter();
        let label = painter.layout(
            setting.label.to_owned(),
            FontId::proportional(13.5),
            label_colour,
            text_width - if setting.restart { 64.0 } else { 0.0 },
        );
        let description = describe.unwrap_or_else(|| setting.description.to_owned());
        let lines: Vec<_> = [Some(description), note]
            .into_iter()
            .flatten()
            .filter(|t| !t.is_empty())
            .map(|t| painter.layout(t, FontId::proportional(11.5), words_colour, text_width))
            .collect();
        let text_height = label.size().y + lines.iter().map(|g| g.size().y + 3.0).sum::<f32>();
        let content = text_height.max(control.height());
        if (content - last).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(height_id, content));
            ui.ctx().request_repaint();
        }
        let rect = Rect::from_min_size(top, vec2(width, content + 2.0 * ROW_PAD.y));
        let mut y = top.y + ROW_PAD.y + (content - text_height) / 2.0;
        let x = top.x + ROW_PAD.x;
        let label_size = label.size();
        painter.galley(pos2(x, y), label, label_colour);
        let mut badge = None;
        if setting.restart {
            badge = Some(controls::restart_badge(
                ui,
                theme,
                pos2(x + label_size.x + 8.0, y + label_size.y / 2.0),
            ));
        }
        y += label_size.y + 3.0;
        for line in lines {
            let h = line.size().y;
            ui.painter().galley(pos2(x, y), line, words_colour);
            y += h + 3.0;
        }

        ui.advance_cursor_after_rect(rect);
        if let Some(badge) = badge {
            ui.interact(badge, Id::unique(("restart-badge", setting.id)), Sense::hover())
                .on_hover_text(controls::RESTART_TIP);
        }
        if let Some(why) = disabled {
            let words = Rect::from_min_size(top, vec2(text_width + ROW_PAD.x, rect.height()));
            ui.interact(words, Id::unique(("settings-why", setting.id)), Sense::hover())
                .on_hover_text(why);
        }

        // A row jumped to: scrolled into view, lit, fading.
        if self.jump == Some(setting.id) {
            ui.scroll_to_rect(rect, Some(Align::Center));
            self.jump = None;
        }
        let now = ui.input(|i| i.time);
        if let Some((id, since)) = &mut self.flash
            && *id == setting.id
        {
            let since = *since.get_or_insert(now);
            let age = now - since;
            if age < FLASH {
                let strength = 1.0 - (age / FLASH) as f32;
                ui.painter().set(
                    backdrop,
                    egui::Shape::rect_filled(
                        rect.shrink(1.0),
                        7.0,
                        theme.accent.gamma_multiply(0.16 * strength),
                    ),
                );
                ui.painter().rect_stroke(
                    rect.shrink(1.0),
                    7.0,
                    Stroke::new(1.0, theme.accent.gamma_multiply(0.7 * strength)),
                    StrokeKind::Inside,
                );
                ui.ctx().request_repaint();
            } else {
                self.flash = None;
            }
        }
    }
}

/// The height a row's control is centred on before it was first drawn.
const CONTROL_HEIGHT_GUESS: f32 = 34.0;

/// Settings' window: a large panel under the top bar, centred, with the
/// view in it. Escape closes it while it is the window in front (an open
/// dropdown takes Escape first). `open` is set to false when it closes.
pub fn window(ctx: &egui::Context, theme: &Theme, open: &mut bool, add: impl FnOnce(&mut Ui) -> bool) {
    if !*open {
        return;
    }
    let area = crate::layout::window_area(ctx.content_rect());
    let size = vec2(area.width().min(MAX_SIZE.x), area.height().min(MAX_SIZE.y));
    let rect = Rect::from_min_size(pos2(area.center().x - size.x / 2.0, area.top()), size);
    let id = Id::unique("settings-window");
    let frame = egui::Frame::new()
        .fill(theme.bg)
        .stroke(Stroke::new(1.0, theme.line_strong))
        .corner_radius(10)
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: egui::Color32::from_black_alpha(110),
        });
    let mut closed = false;
    egui::Window::new("Settings")
        .id(id)
        .title_bar(false)
        // Never faded in: egui fades a window in again whenever it missed
        // being drawn for a pass, which showed the page through Settings
        // for a moment, now and then.
        .fade_in(false)
        .frame(frame)
        .fixed_rect(rect)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            closed = add(ui);
        });
    let in_front = ctx.memory(|m| m.areas().top_layer_id(egui::Order::Middle))
        == Some(egui::LayerId::new(egui::Order::Middle, id));
    let escape =
        in_front && !egui::Popup::is_any_open(ctx) && ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if closed || escape {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTIONS: &[Section] = &[Section {
        id: "editing-needle",
        title: "Edit needle",
        settings: &[
            Setting::new(
                "edit-at-playhead",
                "Edit at the playhead",
                "One line instead of two.",
            ),
            Setting::new(
                "click-moves",
                "Clicking the timeline moves",
                "The playhead or the needle.",
            ),
        ],
    }];
    const CATEGORIES: &[Category] = &[
        Category {
            id: "look",
            label: "Look",
            sections: &[Section {
                id: "appearance",
                title: "Appearance",
                settings: &[Setting::new("theme", "Theme", "Colours.")],
            }],
        },
        Category {
            id: "editing",
            label: "Editing",
            sections: SECTIONS,
        },
    ];

    /// One frame of the view at 1280 × 800 with `events`; `draw` adds the controls.
    fn view_frame(
        ctx: &egui::Context,
        view: &mut SettingsView,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&mut Row<'_>),
    ) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            view.show(ui, &Theme::ASH, CATEGORIES, &mut draw);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn a_control_in_a_row_takes_its_click() {
        let ctx = egui::Context::default();
        crate::setup(&ctx, crate::ThemeName::Ash);
        let mut view = SettingsView::default();
        view.open_at(CATEGORIES, "edit-at-playhead");
        let mut on = false;
        let mut at = Rect::NOTHING;
        for _ in 0..3 {
            view_frame(&ctx, &mut view, Vec::new(), |row| {
                if row.id() == "edit-at-playhead" {
                    at = toggle(row.ui, row.theme, &mut on).rect;
                }
            });
        }
        assert!(at.is_positive());
        let press = |pressed| egui::Event::PointerButton {
            pos: at.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        view_frame(
            &ctx,
            &mut view,
            vec![egui::Event::PointerMoved(at.center()), press(true), press(false)],
            |row| {
                if row.id() == "edit-at-playhead" {
                    toggle(row.ui, row.theme, &mut on);
                }
            },
        );
        assert!(on, "nothing of the row lies over its control");
        // The control sits at the right of the row, in the column of controls.
        assert!(at.right() > 1280.0 / 2.0);
    }

    #[test]
    fn open_at_picks_the_tab_and_the_row_and_clears_the_search() {
        let mut view = SettingsView::default();
        view.set_query("theme");
        assert!(view.open_at(CATEGORIES, "click-moves"));
        assert_eq!(view.tab(), 1);
        assert_eq!(view.query(), "");
        assert_eq!(view.jumping_to(), Some("click-moves"));
        assert!(
            view.open_at(CATEGORIES, "look"),
            "a category's id opens its first row"
        );
        assert_eq!((view.tab(), view.jumping_to()), (0, Some("theme")));
        assert!(!view.open_at(CATEGORIES, "nowhere"));
        assert_eq!(view.tab(), 0, "an unknown id changes nothing");
    }

    #[test]
    fn the_view_draws_every_row_of_the_open_tab_and_scrolls_to_the_one_asked_for() {
        let ctx = egui::Context::default();
        crate::setup(&ctx, crate::ThemeName::Ash);
        let theme = Theme::ASH;
        let mut view = SettingsView::default();
        view.open_at(CATEGORIES, "click-moves");
        let mut drawn = Vec::new();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            view.show(ui, &theme, CATEGORIES, |row| {
                drawn.push(row.id());
                let mut on = false;
                toggle(row.ui, row.theme, &mut on);
            });
        })
        .drop_without_applying_deltas();
        assert_eq!(drawn, ["edit-at-playhead", "click-moves"]);
        assert_eq!(view.jumping_to(), None, "scrolled to once");

        // Searching draws only what is found, from any tab.
        view.set_query("colours");
        drawn.clear();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            view.show(ui, &theme, CATEGORIES, |row| drawn.push(row.id()));
        })
        .drop_without_applying_deltas();
        assert_eq!(drawn, ["theme"]);
    }
}
