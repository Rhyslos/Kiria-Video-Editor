//! The parts every right-hand panel is built from: sections that fold away
//! and say in one line what they hold, and rows of a label and a value.
//!
//! A page decides what its sections are; how they look and behave is here,
//! so the Edit page's inspector, the Library's details and a tool's settings
//! read the same.

use crate::Theme;
use crate::keyframe::{self, KeyClicks, Keys};
use egui::{Align, Layout, RichText, Sense, Stroke, Ui};

/// What a section's header was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SectionClicks {
    /// "Reset" was clicked: put the section's values back as they come.
    pub reset: bool,
}

/// A section: a header with its `title`, and while folded away a one-line
/// `summary` of what is set in it; `body` under it while open. `changed`
/// says something in it differs from how it comes, which offers Reset.
pub fn section(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    summary: &str,
    changed: bool,
    body: impl FnOnce(&mut Ui),
) -> SectionClicks {
    let tip = format!("Put {title} back as it comes");
    section_with_reset(ui, theme, title, summary, changed, &tip, body)
}

/// [`section`] for values that can change over time: while `animated`,
/// Reset also takes their keyframes away, and says so.
pub fn animated_section(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    summary: &str,
    changed: bool,
    animated: bool,
    body: impl FnOnce(&mut Ui),
) -> SectionClicks {
    let tip = match animated {
        true => format!("Put {title} back as it comes, and remove its keyframes"),
        false => format!("Put {title} back as it comes"),
    };
    section_with_reset(ui, theme, title, summary, changed || animated, &tip, body)
}

fn section_with_reset(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    summary: &str,
    changed: bool,
    reset_tip: &str,
    body: impl FnOnce(&mut Ui),
) -> SectionClicks {
    let mut clicks = SectionClicks::default();
    let id = ui.make_persistent_id(("inspector-section", title));
    let mut open = ui.data_mut(|data| *data.get_persisted_mut_or(id, true));
    ui.add_space(2.0);
    let header = ui
        .horizontal(|ui| {
            ui.set_min_height(26.0);
            let arrow = if open { "▾" } else { "▸" };
            ui.label(RichText::new(arrow).color(theme.muted));
            ui.label(
                RichText::new(title)
                    .family(crate::fonts::semibold())
                    .color(theme.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if changed
                    && ui
                        .add(
                            egui::Button::new(RichText::new("Reset").color(theme.text_secondary).size(12.0))
                                .frame(false),
                        )
                        .on_hover_text(reset_tip)
                        .clicked()
                {
                    clicks.reset = true;
                }
                if !open {
                    ui.add(egui::Label::new(RichText::new(summary).color(theme.muted).size(12.0)).truncate());
                }
            });
        })
        .response;
    // The whole header folds and unfolds, apart from its Reset.
    if !clicks.reset && header.interact(Sense::click()).clicked() {
        open = !open;
        ui.data_mut(|data| data.insert_persisted(id, open));
    }
    if open {
        ui.add_space(2.0);
        body(ui);
        ui.add_space(6.0);
    }
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, theme.line));
    ui.add_space(1.0);
    clicks
}

/// How a number is shown and changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Number {
    /// The least and the most it can be.
    pub range: (f64, f64),
    /// How much a drag of one point changes it.
    pub per_point: f64,
    pub decimals: usize,
    /// Written after it: " %", "°", " px".
    pub suffix: &'static str,
    /// What a double-click puts it back to.
    pub usual: f64,
}

/// What happened to a value this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Touched {
    /// It has another value now.
    pub changed: bool,
    /// It is being dragged or typed into: more changes may follow, so this
    /// is not the moment to make an undo step.
    pub busy: bool,
}

impl Touched {
    pub fn and(self, other: Touched) -> Touched {
        Touched {
            changed: self.changed || other.changed,
            busy: self.busy || other.busy,
        }
    }
}

/// Width of the label column.
const LABEL_WIDTH: f32 = 86.0;
/// Width of the number at the end of a slider's row.
const VALUE_WIDTH: f32 = 58.0;

/// A row: `label`, then a number to drag sideways or type. Double-click
/// puts it back to its usual value.
pub fn number(ui: &mut Ui, theme: &Theme, label: &str, value: &mut f64, how: Number) -> Touched {
    row(ui, theme, label, |ui| drag(ui, value, how))
}

/// A row with two numbers side by side, each with a short name before it
/// ("X", "Y").
pub fn pair(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    first: (&str, &mut f64, Number),
    second: (&str, &mut f64, Number),
) -> Touched {
    row(ui, theme, label, |ui| {
        ui.label(RichText::new(first.0).color(theme.muted).size(12.0));
        let a = drag(ui, first.1, first.2);
        ui.label(RichText::new(second.0).color(theme.muted).size(12.0));
        a.and(drag(ui, second.1, second.2))
    })
}

/// A row: `label`, then a slider across the rest of the row with its
/// number after it. Double-click puts it back to its usual value.
pub fn slider(ui: &mut Ui, theme: &Theme, label: &str, value: &mut f64, how: Number) -> Touched {
    slider_row(ui, theme, label, value, how, None)
}

/// What happened in a row with a keyframe control: to its value, and to
/// its keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keyed {
    pub touched: Touched,
    pub clicks: KeyClicks,
}

/// [`number`], with the keyframe control at the end of the row
/// ([`keyframe::control`]).
pub fn keyed_number(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f64,
    how: Number,
    keys: &Keys,
) -> Keyed {
    keyed_row(ui, theme, label, keys, |ui| drag(ui, value, how))
}

/// [`pair`], with one keyframe control for both numbers: they change over
/// time together.
pub fn keyed_pair(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    first: (&str, &mut f64, Number),
    second: (&str, &mut f64, Number),
    keys: &Keys,
) -> Keyed {
    keyed_row(ui, theme, label, keys, |ui| {
        // Each number gets half of what is left, so a long one never
        // pushes the keyframe control out of its place.
        let names = 2.0 * (12.0 + 2.0 * ui.spacing().item_spacing.x);
        let width = ((ui.available_width() - names) / 2.0).clamp(36.0, 90.0);
        ui.label(RichText::new(first.0).color(theme.muted).size(12.0));
        let a = sized_drag(ui, first.1, first.2, width);
        ui.label(RichText::new(second.0).color(theme.muted).size(12.0));
        a.and(sized_drag(ui, second.1, second.2, width))
    })
}

/// [`slider`], with the keyframe control at the end of the row.
pub fn keyed_slider(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f64,
    how: Number,
    keys: &Keys,
) -> Keyed {
    keyed_row(ui, theme, label, keys, |ui| slider_values(ui, value, how, None))
}

/// A row whose keyframe control sits at its right end; the values take
/// what is left between the label and it.
fn keyed_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    keys: &Keys,
    values: impl FnOnce(&mut Ui) -> Touched,
) -> Keyed {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        label_cell(ui, theme, label, LABEL_WIDTH);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let clicks = keyframe::control(ui, theme, label, keys);
            let touched = ui
                .with_layout(Layout::left_to_right(Align::Center), |ui| values(ui))
                .inner;
            Keyed { touched, clicks }
        })
        .inner
    })
    .inner
}

/// How a number is written in words of its own ("L 20", "Off",
/// "0.50 s · 30 frames") and how typed text is read back. Text that
/// `read` can't make sense of leaves the number as it was.
#[derive(Clone, Copy)]
pub struct Words<'a> {
    pub write: &'a dyn Fn(f64) -> String,
    pub read: &'a dyn Fn(&str) -> Option<f64>,
}

impl std::fmt::Debug for Words<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Words")
    }
}

/// [`slider`], with the number written in `words`.
pub fn slider_in_words(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f64,
    how: Number,
    words: Words,
) -> Touched {
    slider_row(ui, theme, label, value, how, Some(words))
}

/// [`number`], with the number written in `words`.
pub fn number_in_words(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f64,
    how: Number,
    words: Words,
) -> Touched {
    row(ui, theme, label, |ui| {
        let before = *value;
        let response = ui.add(worded(value_drag(value, how), Some(words)));
        touched(&response, value, before, how)
    })
}

fn slider_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f64,
    how: Number,
    words: Option<Words>,
) -> Touched {
    row(ui, theme, label, |ui| slider_values(ui, value, how, words))
}

/// A slider across the room left, with its number after it.
fn slider_values(ui: &mut Ui, value: &mut f64, how: Number, words: Option<Words>) -> Touched {
    // The number has a fixed place at the end of the row; the slider
    // takes what is left, so rows line up whatever the numbers say.
    let room = ui.available_width() - VALUE_WIDTH - ui.spacing().item_spacing.x;
    ui.spacing_mut().slider_width = room.max(40.0);
    let before = *value;
    let slider = ui.add(
        egui::Slider::new(value, how.range.0..=how.range.1)
            .show_value(false)
            .clamping(egui::SliderClamping::Always),
    );
    // Shift held while dragging: a tenth as fast, from where it was.
    if slider.dragged() && ui.input(|i| i.modifiers.shift) {
        *value = fine_step(
            before,
            ui.input(|i| i.pointer.delta().x),
            room.max(40.0),
            how.range,
        );
    }
    let on_slider = touched(&slider, value, before, how);
    let typed = ui.add_sized([VALUE_WIDTH, 20.0], worded(value_drag(value, how), words));
    on_slider.and(touched(&typed, value, before, how))
}

/// How much slower a slider moves with Shift held.
pub const FINE: f64 = 0.1;

/// A slider `width` points wide over `range`, dragged `dx` points with
/// Shift held: the value moves a tenth as far as the pointer, from
/// `before`, within the range.
pub fn fine_step(before: f64, dx: f32, width: f32, range: (f64, f64)) -> f64 {
    let per_point = (range.1 - range.0) / f64::from(width.max(1.0));
    (before + f64::from(dx) * per_point * FINE).clamp(range.0, range.1)
}

/// The number to drag sideways or type, as `how` says.
fn value_drag(value: &mut f64, how: Number) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(how.range.0..=how.range.1)
        .speed(how.per_point)
        .fixed_decimals(how.decimals)
        .suffix(how.suffix)
}

fn worded<'a>(drag: egui::DragValue<'a>, words: Option<Words<'a>>) -> egui::DragValue<'a> {
    match words {
        Some(words) => drag
            .custom_formatter(move |value, _| (words.write)(value))
            .custom_parser(move |text| (words.read)(text)),
        None => drag,
    }
}

fn row(ui: &mut Ui, theme: &Theme, label: &str, values: impl FnOnce(&mut Ui) -> Touched) -> Touched {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        label_cell(ui, theme, label, LABEL_WIDTH);
        values(ui)
    })
    .inner
}

/// [`drag`] in a box `width` wide.
fn sized_drag(ui: &mut Ui, value: &mut f64, how: Number, width: f32) -> Touched {
    let before = *value;
    let response = ui.add_sized([width, 20.0], value_drag(value, how));
    touched(&response, value, before, how)
}

fn drag(ui: &mut Ui, value: &mut f64, how: Number) -> Touched {
    let before = *value;
    let response = ui.add(value_drag(value, how));
    touched(&response, value, before, how)
}

fn touched(response: &egui::Response, value: &mut f64, before: f64, how: Number) -> Touched {
    if response.double_clicked() {
        *value = how.usual;
    }
    Touched {
        changed: *value != before,
        busy: response.dragged() || response.has_focus(),
    }
}

/// A label in a cell of a fixed width, so what follows lines up from row
/// to row whatever the labels say.
pub fn label_cell(ui: &mut Ui, theme: &Theme, label: &str, width: f32) {
    let (cell, _) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::hover());
    ui.painter().text(
        cell.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        theme.text_secondary,
    );
}

/// A line of quiet text: what a panel says when it has nothing to show.
pub fn note(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).color(theme.muted));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_moves_a_slider_a_tenth_as_far() {
        // 0 to 100 over 50 points: 2 per point, 0.2 with Shift.
        assert!((fine_step(10.0, 5.0, 50.0, (0.0, 100.0)) - 11.0).abs() < 1e-9);
        assert_eq!(
            fine_step(99.9, 50.0, 50.0, (0.0, 100.0)),
            100.0,
            "within the range"
        );
    }

    #[test]
    fn touches_add_up() {
        let changed = Touched {
            changed: true,
            busy: false,
        };
        let busy = Touched {
            changed: false,
            busy: true,
        };
        assert_eq!(
            changed.and(busy),
            Touched {
                changed: true,
                busy: true
            }
        );
        assert_eq!(Touched::default().and(Touched::default()), Touched::default());
    }
}
