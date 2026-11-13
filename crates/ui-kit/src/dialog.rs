//! Dialogs: one look for every window that asks something and waits.
//!
//! A dialog is modal: it dims the page, a click outside it closes it
//! without reaching what is underneath, Escape closes it, and while one is
//! up the shortcut table gives nothing to the page behind it
//! ([`crate::ShortcutTable::pressed`]). [`name_dialog`] asks for a name
//! (a new label, list or timeline) with its field focused at once, so what
//! is typed goes into it and never reaches a shortcut. [`palette`] is the
//! same kind of window near the top of the screen, for the search palette.

use crate::Theme;
use egui::{Align2, Context, Frame, Id, Margin, Modal, ModalResponse, RichText, Stroke, Ui, Vec2};

/// Shows a dialog in front of everything, with the shared frame, padding and
/// dimmed backdrop. `id` must be the same every frame. Click outside or
/// Escape asks it to close (see [`ModalResponse::should_close`]).
pub fn dialog<R>(ctx: &Context, theme: &Theme, id: &str, add: impl FnOnce(&mut Ui) -> R) -> ModalResponse<R> {
    modal(theme, id).show(ctx, add)
}

/// A dialog near the top of the window, under the top bar, rather than in
/// the middle: for the search palette, whose list grows downward as it finds.
pub fn palette<R>(
    ctx: &Context,
    theme: &Theme,
    id: &str,
    add: impl FnOnce(&mut Ui) -> R,
) -> ModalResponse<R> {
    let area = Modal::default_area(Id::unique(id)).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 80.0));
    modal(theme, id).area(area).show(ctx, add)
}

fn modal(theme: &Theme, id: &str) -> Modal {
    let frame = Frame::new()
        .fill(theme.raised)
        .stroke(Stroke::new(1.0, theme.line_strong))
        .corner_radius(10.0)
        .inner_margin(Margin::same(20));
    Modal::new(Id::unique(id))
        .frame(frame)
        .backdrop_color(theme.viewport.gamma_multiply(0.6))
}

/// Whether a dialog is up (as of the last frame): the page behind it takes
/// no keys and no clicks.
pub fn is_open(ctx: &Context) -> bool {
    ctx.memory(|m| !m.is_above_modal_layer(egui::LayerId::background()))
}

/// A name being asked for: what the dialog says and what has been typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameAsk {
    /// The dialog's title ("New label").
    pub title: String,
    /// One quiet line under the field: what the name is for.
    pub note: String,
    /// The button that gives the name ("Add", "Rename").
    pub button: String,
    pub typed: String,
    focused: bool,
}

impl NameAsk {
    /// A dialog titled `title`, its field starting with `typed`.
    pub fn new(title: impl Into<String>, typed: impl Into<String>) -> NameAsk {
        NameAsk {
            title: title.into(),
            note: String::new(),
            button: "Add".into(),
            typed: typed.into(),
            focused: false,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> NameAsk {
        self.note = note.into();
        self
    }

    pub fn button(mut self, button: impl Into<String>) -> NameAsk {
        self.button = button.into();
        self
    }

    /// The name as given: trimmed, `None` when only spaces were typed.
    pub fn name(&self) -> Option<String> {
        let name = self.typed.trim();
        (!name.is_empty()).then(|| name.to_owned())
    }
}

/// What a [`name_dialog`] was answered with this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameAnswer {
    /// The name (Enter or the button), trimmed and not empty.
    Given(String),
    /// Cancel, Escape or a click outside.
    Cancelled,
}

/// Asks for a name in a small dialog: the title, a field that has the keys
/// from the first frame, a quiet note, Cancel and the button. Show it every
/// frame while `ask` is kept; take it away on an answer.
pub fn name_dialog(ctx: &Context, theme: &Theme, id: &str, ask: &mut NameAsk) -> Option<NameAnswer> {
    let mut answer = None;
    let modal = dialog(ctx, theme, id, |ui| {
        ui.set_width(340.0);
        ui.label(
            RichText::new(&ask.title)
                .family(crate::fonts::semibold())
                .size(16.0),
        );
        ui.add_space(8.0);
        let field = ui.add(egui::TextEdit::singleline(&mut ask.typed).desired_width(f32::INFINITY));
        if !std::mem::replace(&mut ask.focused, true) {
            field.request_focus();
        }
        // Enter takes the focus out of a one-line field: read it either way.
        let enter =
            (field.has_focus() || field.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !ask.note.is_empty() {
            ui.label(RichText::new(&ask.note).size(11.5).color(theme.muted));
        }
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can = ask.name().is_some();
            let given = ui
                .add_enabled_ui(can, |ui| crate::accent_button(ui, theme, &ask.button))
                .inner
                .clicked();
            if (given || enter)
                && let Some(name) = ask.name()
            {
                answer = Some(NameAnswer::Given(name));
            }
            if ui.button("Cancel").clicked() {
                answer = Some(NameAnswer::Cancelled);
            }
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(NameAnswer::Cancelled);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, RawInput};

    fn key(key: Key) -> Vec<Event> {
        vec![
            Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Key {
                key,
                physical_key: Some(key),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    fn frame(ctx: &Context, events: Vec<Event>, ask: &mut NameAsk) -> Option<NameAnswer> {
        let theme = crate::ThemeName::Ash.theme();
        let mut answer = None;
        let input = RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..RawInput::default()
        };
        ctx.run_ui(input, |ui| {
            answer = name_dialog(ui.ctx(), &theme, "name", ask);
        })
        .drop_without_applying_deltas();
        answer
    }

    #[test]
    fn typing_goes_into_the_name_and_never_to_shortcuts() {
        let ctx = Context::default();
        crate::setup(&ctx, crate::ThemeName::Ash);
        let mut ask = NameAsk::new("New label", "");
        assert_eq!(frame(&ctx, Vec::new(), &mut ask), None);
        // The field has the keys from the next frame on.
        assert!(ctx.egui_wants_keyboard_input());
        assert!(is_open(&ctx), "the page behind takes no keys");
        let table = crate::ShortcutTable::default();
        let mut typed = vec![Event::Text("B-roll".into())];
        typed.extend(key(Key::O));
        let mut pressed = Vec::new();
        let input = RawInput {
            events: typed,
            ..RawInput::default()
        };
        let theme = crate::ThemeName::Ash.theme();
        ctx.run_ui(input, |ui| {
            pressed = table.pressed(ui.ctx());
            name_dialog(ui.ctx(), &theme, "name", &mut ask);
        })
        .drop_without_applying_deltas();
        assert!(pressed.is_empty(), "O marks nothing while a name is typed");
        assert_eq!(ask.typed, "B-roll");
        assert_eq!(
            frame(&ctx, key(Key::Enter), &mut ask),
            Some(NameAnswer::Given("B-roll".into()))
        );
    }

    #[test]
    fn escape_cancels_and_an_empty_name_is_not_given() {
        let ctx = Context::default();
        crate::setup(&ctx, crate::ThemeName::Ash);
        let mut ask = NameAsk::new("New list", "   ");
        frame(&ctx, Vec::new(), &mut ask);
        assert_eq!(frame(&ctx, key(Key::Enter), &mut ask), None, "only spaces");
        assert_eq!(
            frame(&ctx, key(Key::Escape), &mut ask),
            Some(NameAnswer::Cancelled)
        );
    }
}
