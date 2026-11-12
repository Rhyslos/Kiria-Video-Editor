//! The timeline tabs: one tab for each timeline of the project, in the row
//! above the timeline beside its tools, the open one marked. Every page
//! with a timeline shows the same tabs.
//!
//! A click opens a timeline; a double-click types a new name in place;
//! dragging a tab onto the timeline places that timeline there as one
//! piece (a nested timeline, [`DraggedTimeline`]); "+" makes a new one. The
//! right-click menu opens, renames, copies, places in the open timeline,
//! shows the settings of, or removes a timeline.

use crate::theme::Theme;
use crate::timeline::DraggedTimeline;
use egui::{Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

/// One timeline as its tab shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTab {
    pub id: u64,
    pub name: String,
    /// Shown when the pointer rests on the tab: its size, rate and length.
    pub info: String,
    /// It is the timeline that is open.
    pub open: bool,
    /// It can't be placed in the open timeline (it is the open one, or
    /// it plays it): why, for its menu.
    pub cannot_place: Option<String>,
}

/// What was done with the tabs.
#[derive(Debug, Clone, PartialEq)]
pub enum TabsEvent {
    /// A tab was clicked: open that timeline.
    Open(u64),
    /// "+": make a new timeline.
    New,
    /// A new name was typed for a timeline.
    Rename(u64, String),
    Duplicate(u64),
    Remove(u64),
    /// "Place in the open timeline": at its end.
    Place(u64),
    /// "Settings…": the timeline's size and rate.
    Settings(u64),
}

/// The widest a tab grows; longer names are cut short.
const WIDEST: f32 = 180.0;
const HEIGHT: f32 = 24.0;

/// What the tabs keep between frames.
#[derive(Debug, Default)]
pub struct TimelineTabs {
    /// The tab whose name is being typed, and the text so far.
    renaming: Option<(u64, String)>,
    /// The tab the right-click menu was opened on.
    menu_on: Option<u64>,
}

impl TimelineTabs {
    /// Starts typing a new name for the timeline `id` in its tab (as a
    /// double-click does), for "Rename the timeline" elsewhere.
    pub fn rename(&mut self, id: u64, name: &str) {
        self.renaming = Some((id, name.to_owned()));
    }

    /// Draws the tabs in the rest of the row; `new_tip` is the "+" button's
    /// tooltip (with its shortcut).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        tabs: &[TimelineTab],
        new_tip: &str,
    ) -> Vec<TabsEvent> {
        let mut events = Vec::new();
        if self
            .renaming
            .as_ref()
            .is_some_and(|(id, _)| !tabs.iter().any(|t| t.id == *id))
        {
            self.renaming = None;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for tab in tabs {
                if self.renaming.as_ref().is_some_and(|(id, _)| *id == tab.id) {
                    self.name_field(ui, tab, &mut events);
                } else {
                    self.tab(ui, theme, tab, &mut events);
                }
            }
            ui.add_space(4.0);
            let (rect, r) = ui.allocate_exact_size(vec2(HEIGHT, HEIGHT), Sense::click());
            let r = r.on_hover_text(new_tip);
            if r.hovered() {
                ui.painter().rect_filled(rect, 6.0, theme.line);
            }
            let (c, arm) = (rect.center(), 5.0);
            let stroke = Stroke::new(1.6, theme.text_secondary);
            ui.painter()
                .line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
            ui.painter()
                .line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], stroke);
            if r.clicked() {
                events.push(TabsEvent::New);
            }
        });
        events
    }

    fn tab(&mut self, ui: &mut Ui, theme: &Theme, tab: &TimelineTab, events: &mut Vec<TabsEvent>) {
        let font = FontId::proportional(12.0);
        let colour = if tab.open {
            theme.text
        } else {
            theme.text_secondary
        };
        let galley = ui.painter().layout(tab.name.clone(), font, colour, WIDEST - 20.0);
        let width = (galley.size().x + 20.0).clamp(48.0, WIDEST);
        let (rect, r) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let r = r.on_hover_text(format!(
            "{}\n{}\nClick to open, double-click to rename, drag onto the timeline to place it there.",
            tab.name, tab.info
        ));
        let fill = if tab.open {
            theme.raised
        } else if r.hovered() {
            theme.line
        } else {
            Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect_filled(rect, 6.0, fill);
        if tab.open {
            p.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left() + 6.0, rect.bottom() - 2.0),
                    pos2(rect.right() - 6.0, rect.bottom()),
                ),
                1.0,
                theme.accent,
            );
        }
        let text_at = pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
        ui.painter_at(rect.shrink2(vec2(8.0, 0.0)))
            .galley(text_at, galley, colour);

        if r.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), DraggedTimeline(tab.id));
        }
        if r.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if r.double_clicked() {
            self.renaming = Some((tab.id, tab.name.clone()));
        } else if r.clicked() && !tab.open {
            events.push(TabsEvent::Open(tab.id));
        }
        if r.secondary_clicked() {
            self.menu_on = Some(tab.id);
        }
        r.context_menu(|ui| {
            if self.menu_on != Some(tab.id) {
                return;
            }
            if !tab.open && ui.button("Open").clicked() {
                events.push(TabsEvent::Open(tab.id));
                ui.close();
            }
            if ui.button("Rename…").clicked() {
                self.renaming = Some((tab.id, tab.name.clone()));
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                events.push(TabsEvent::Duplicate(tab.id));
                ui.close();
            }
            let place = ui.add_enabled(
                tab.cannot_place.is_none(),
                egui::Button::new("Place in the open timeline"),
            );
            let place = match &tab.cannot_place {
                Some(why) => place.on_disabled_hover_text(why),
                None => place,
            };
            if place.clicked() {
                events.push(TabsEvent::Place(tab.id));
                ui.close();
            }
            if ui.button("Settings…").clicked() {
                events.push(TabsEvent::Settings(tab.id));
                ui.close();
            }
            ui.separator();
            if ui.button("Remove").clicked() {
                events.push(TabsEvent::Remove(tab.id));
                ui.close();
            }
        });
    }

    /// The field a new name is typed in: Enter or a click elsewhere gives
    /// it, Escape leaves the name as it was.
    fn name_field(&mut self, ui: &mut Ui, tab: &TimelineTab, events: &mut Vec<TabsEvent>) {
        let Some((id, text)) = self.renaming.as_mut() else {
            return;
        };
        let edit = egui::TextEdit::singleline(text)
            .desired_width(WIDEST - 20.0)
            .font(FontId::proportional(12.0));
        let r = ui.add(edit);
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if r.lost_focus() {
            let name = text.trim().to_owned();
            let id = *id;
            self.renaming = None;
            if !escape && !name.is_empty() && name != tab.name {
                events.push(TabsEvent::Rename(id, name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs() -> Vec<TimelineTab> {
        ["Main sequence", "Short cut"]
            .iter()
            .enumerate()
            .map(|(i, name)| TimelineTab {
                id: i as u64 + 1,
                name: (*name).into(),
                info: String::new(),
                open: i == 0,
                cannot_place: (i == 0).then(|| "It is open.".into()),
            })
            .collect()
    }

    /// Runs one frame of the tabs with `events`, returning what they said.
    fn frame(ctx: &egui::Context, state: &mut TimelineTabs, events: Vec<egui::Event>) -> Vec<TabsEvent> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 60.0))),
            events,
            ..Default::default()
        };
        let mut out = Vec::new();
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| {
                    out = state.show(ui, &Theme::ASH, &tabs(), "New timeline")
                });
        });
        // Nothing draws these frames.
        output.drop_without_applying_deltas();
        out
    }

    fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(ctx: &egui::Context, state: &mut TimelineTabs, at: egui::Pos2) -> Vec<TabsEvent> {
        frame(ctx, state, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, state, vec![press(at, true)]);
        frame(ctx, state, vec![press(at, false)])
    }

    #[test]
    fn a_click_opens_another_timeline_and_plus_makes_one() {
        let ctx = egui::Context::default();
        let mut state = TimelineTabs::default();
        frame(&ctx, &mut state, Vec::new());
        // The open tab first, then the second; "+" after them.
        let second = pos2(130.0, 12.0);
        assert_eq!(click(&ctx, &mut state, second), vec![TabsEvent::Open(2)]);
        assert!(
            click(&ctx, &mut state, pos2(20.0, 12.0)).is_empty(),
            "already open"
        );
        // The + comes after the tabs, wherever their names end.
        let plus = (150..400)
            .step_by(4)
            .map(|x| click(&ctx, &mut state, pos2(x as f32, 12.0)))
            .find(|events| events.contains(&TabsEvent::New));
        assert_eq!(plus, Some(vec![TabsEvent::New]), "the + after the tabs");
    }

    #[test]
    fn a_typed_name_renames_and_escape_leaves_it() {
        let ctx = egui::Context::default();
        let mut state = TimelineTabs::default();
        frame(&ctx, &mut state, Vec::new());
        state.rename(2, "Short cut");
        frame(&ctx, &mut state, Vec::new());
        frame(&ctx, &mut state, vec![egui::Event::Text(" 2".into())]);
        let events = frame(
            &ctx,
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(events, vec![TabsEvent::Rename(2, "Short cut 2".into())]);
        state.rename(2, "Short cut");
        frame(&ctx, &mut state, Vec::new());
        let events = frame(
            &ctx,
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(events.is_empty(), "{events:?}");
    }
}
