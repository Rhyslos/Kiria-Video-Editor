//! The inspector's Title section, on a title: its words, its layout, the
//! size and colour of its letters and the band behind them. Words being
//! typed and a size being dragged are shown in the viewer as they change
//! and become one undo step when the field is left or the slider let go;
//! a layout, colour or band change is one at once.

use project::{ClipId, Command, SequenceId, Title, TitleStyle};
use ui_kit::{Theme, inspector};

/// Colours offered with one click, red, green and blue.
const SWATCHES: [([u8; 3], &str); 6] = [
    ([255, 255, 255], "White"),
    ([20, 20, 22], "Black"),
    ([255, 214, 10], "Yellow"),
    ([255, 92, 72], "Red"),
    ([80, 200, 255], "Blue"),
    ([120, 230, 120], "Green"),
];

/// A row with a few colours to click and a button for any other: true
/// when `rgb` was changed (a click: one step at once).
pub(super) fn colour_row(ui: &mut egui::Ui, theme: &Theme, label: &str, rgb: &mut [u8; 3]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        inspector::label_cell(ui, theme, label, inspector::LABEL_WIDTH);
        ui.spacing_mut().item_spacing.x = 5.0;
        for (colour, name) in SWATCHES {
            let fill = egui::Color32::from_rgb(colour[0], colour[1], colour[2]);
            let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 4.0, fill);
            let ring = if *rgb == colour { theme.text } else { theme.line };
            ui.painter()
                .rect_stroke(rect, 4.0, egui::Stroke::new(1.5, ring), egui::StrokeKind::Outside);
            if response.on_hover_text(name).clicked() && *rgb != colour {
                *rgb = colour;
                changed = true;
            }
        }
        let mut any = *rgb;
        if ui
            .color_edit_button_srgb(&mut any)
            .on_hover_text("Any colour")
            .changed()
        {
            *rgb = any;
            changed = true;
        }
    });
    changed
}

/// What the section asked for.
#[derive(Debug, Default)]
pub struct Asked {
    /// The change to make, one undo step.
    pub command: Option<Command>,
    /// The title as it is being changed, not yet an undo step.
    pub changing: Option<Title>,
}

/// Draws the section for the title `clip` showing `shown` (its words with
/// any change being made), `stored` being what the project holds.
pub fn section(
    ui: &mut egui::Ui,
    theme: &Theme,
    sequence: SequenceId,
    clip: ClipId,
    stored: &Title,
    shown: &Title,
    focus: bool,
) -> Asked {
    let mut title = shown.clone();
    // Whether a hand is still on something: the words' field has the
    // keyboard, or the size is being dragged.
    let mut busy = false;
    let mut at_once = false;
    let summary = format!("{} · {}", title.style.name(), title.first_line());
    inspector::section(ui, theme, "Title", &summary, false, |ui| {
        let words = ui.add(
            egui::TextEdit::multiline(&mut title.text)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("What it says"),
        );
        if focus {
            // A title just added: its words chosen, ready to type over.
            words.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), words.id) {
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(title.text.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), words.id);
            }
        }
        busy |= words.has_focus();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            inspector::label_cell(ui, theme, "Layout", inspector::LABEL_WIDTH);
            egui::ComboBox::from_id_salt(("title layout", clip.0))
                .selected_text(title.style.name())
                .show_ui(ui, |ui| {
                    for style in TitleStyle::ALL {
                        if ui.selectable_label(title.style == style, style.name()).clicked()
                            && title.style != style
                        {
                            title.style = style;
                            at_once = true;
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            inspector::label_cell(ui, theme, "Size", inspector::LABEL_WIDTH);
            let (least, most) = Title::SIZES;
            let mut percent = f64::from(title.size) / 10_000.0;
            let slider = ui.add(
                egui::Slider::new(
                    &mut percent,
                    f64::from(least) / 10_000.0..=f64::from(most) / 10_000.0,
                )
                .suffix(" %")
                .max_decimals(1),
            );
            let slider = slider.on_hover_text("How tall its letters are, against the frame's height");
            title.size = (percent * 10_000.0).round() as i32;
            busy |= slider.dragged() || slider.has_focus();
        });
        let mut rgb = title.colour;
        if colour_row(ui, theme, "Colour", &mut rgb) {
            title.colour = rgb;
            at_once = true;
        }
        ui.horizontal(|ui| {
            inspector::label_cell(ui, theme, "Band", inspector::LABEL_WIDTH);
            let what = match title.style {
                TitleStyle::EndCard => "A dark card behind it",
                _ => "A dark band behind it",
            };
            if ui.checkbox(&mut title.backing, what).changed() {
                at_once = true;
            }
        });
    });
    let changed = title != *stored;
    if changed && title.check().is_err() {
        // Too long: kept as it was.
        return Asked::default();
    }
    match (changed, busy && !at_once) {
        (false, _) => Asked::default(),
        (true, true) => Asked {
            command: None,
            changing: Some(title),
        },
        (true, false) => Asked {
            command: Some(Command::SetTitle {
                sequence,
                clip,
                title: Box::new(title),
            }),
            changing: None,
        },
    }
}
