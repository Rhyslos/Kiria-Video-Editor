//! The inspector's picture sections: Transform (with its Blend row, Fit,
//! Fill, "On the picture" and the layouts), Crop and Quick color, or for an
//! adjustment layer Opacity and Quick color. Each number has its keyframe
//! diamond. The numbers change in place as they are touched; what else was
//! asked for (a section's Reset, a keyframe control, another blend mode)
//! comes back as [`Asks`] for the inspector to answer.

use project::look::ONE;
use project::{Ease, Look, LookField};
use ui_kit::Theme;
use ui_kit::inspector::{self, Keyed, Touched};
use ui_kit::keyframe::KeyClicks;

use super::keyframes::{KeyTable, over_time};
use super::look::{
    CROP, EITHER_WAY, EXPOSURE, PERCENT_FULL, SIZE, TURN, Values, colour_summary, crop_summary, fill_percent,
    fitted, in_corner, in_half, pixels, short, transform_summary,
};

/// The numbers each picture section holds: what its Reset puts back, and
/// takes the keyframes of.
pub(super) const TRANSFORM: [LookField; 5] = [
    LookField::X,
    LookField::Y,
    LookField::Scale,
    LookField::Turn,
    LookField::Opacity,
];
pub(super) const CROP_FIELDS: [LookField; 4] = [
    LookField::CropLeft,
    LookField::CropTop,
    LookField::CropRight,
    LookField::CropBottom,
];
/// An adjustment layer's one number besides its colour.
const OPACITY: [LookField; 1] = [LookField::Opacity];
pub(super) const COLOUR: [LookField; 5] = [
    LookField::Exposure,
    LookField::Contrast,
    LookField::Saturation,
    LookField::Temperature,
    LookField::Tint,
];

/// What the picture sections asked for besides changed numbers.
#[derive(Debug, Default)]
pub(super) struct Asks {
    /// A section's Reset: these numbers back as they come, keyframes gone.
    pub(super) resets: Vec<&'static [LookField]>,
    /// A keyframe control was used, for this group.
    pub(super) keys: Vec<(&'static [LookField], KeyClicks)>,
    /// Another blend mode was picked.
    pub(super) mode: Option<project::BlendMode>,
}

impl Asks {
    /// Takes a keyed row's outcome for the group of `field`.
    fn row(&mut self, touched: &mut Touched, field: LookField, keyed: Keyed) {
        *touched = touched.and(keyed.touched);
        if keyed.clicks.any() {
            self.keys.push((field.group(), keyed.clicks));
        }
    }
}

/// Transform, Crop and Quick color: for clips with a picture. The numbers
/// in `values` change as they are touched; Reset and the keyframe controls
/// come back as [`Asks`].
#[allow(clippy::too_many_arguments)]
pub(super) fn picture_sections(
    ui: &mut egui::Ui,
    theme: &Theme,
    before: &Look,
    frame: (u32, u32),
    picture: (u32, u32),
    on_picture: (&mut bool, &str),
    adjustment: bool,
    mode: project::BlendMode,
    keys: &KeyTable,
    values: &mut Values,
    touched: &mut Touched,
) -> Asks {
    let before = *before;
    let mut asks = Asks::default();
    let eases = Ease::ALL.map(Ease::label);
    let k = |field| keys.keys(field, &eases);
    if adjustment {
        // An adjustment layer has no picture to place or cut: only how
        // strongly its colour shows.
        let clicks = inspector::animated_section(
            ui,
            theme,
            "Opacity",
            &format!(
                "{}{}",
                over_time(keys, &OPACITY),
                short(f64::from(before.opacity) / f64::from(ONE) * 100.0, 0) + " %"
            ),
            before.opacity != ONE,
            keys.animates(&OPACITY),
            |ui| {
                let keyed = inspector::keyed_slider(
                    ui,
                    theme,
                    "Opacity",
                    &mut values.opacity,
                    PERCENT_FULL,
                    &k(LookField::Opacity),
                );
                asks.row(touched, LookField::Opacity, keyed);
            },
        );
        if clicks.reset {
            asks.resets.push(&OPACITY);
        }
        colour_section(ui, theme, &before, keys, values, touched, &mut asks);
        return asks;
    }
    let clicks = inspector::animated_section(
        ui,
        theme,
        "Transform",
        &format!(
            "{}{}",
            over_time(keys, &TRANSFORM),
            transform_summary(&before, frame)
        ),
        before.is_moved() || before.opacity != ONE,
        keys.animates(&TRANSFORM),
        |ui| {
            let keyed = inspector::keyed_pair(
                ui,
                theme,
                "Position",
                ("X", &mut values.x, pixels(frame.0)),
                ("Y", &mut values.y, pixels(frame.1)),
                &k(LookField::X),
            );
            asks.row(touched, LookField::X, keyed);
            let keyed =
                inspector::keyed_number(ui, theme, "Size", &mut values.scale, SIZE, &k(LookField::Scale));
            asks.row(touched, LookField::Scale, keyed);
            let keyed =
                inspector::keyed_number(ui, theme, "Turn", &mut values.turn, TURN, &k(LookField::Turn));
            asks.row(touched, LookField::Turn, keyed);
            let keyed = inspector::keyed_slider(
                ui,
                theme,
                "Opacity",
                &mut values.opacity,
                PERCENT_FULL,
                &k(LookField::Opacity),
            );
            asks.row(touched, LookField::Opacity, keyed);
            asks.mode = blend_row(ui, theme, mode);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(90.0);
                if ui
                    .button("Fit")
                    .on_hover_text("As large as fits the frame, centred: nothing cut off")
                    .clicked()
                {
                    let fit = Values::of(&fitted(&before), frame);
                    (values.x, values.y, values.scale) = (fit.x, fit.y, fit.scale);
                    touched.changed = true;
                }
                if ui
                    .button("Fill")
                    .on_hover_text("Large enough to fill the frame, centred: no bars")
                    .clicked()
                {
                    (values.x, values.y, values.scale) = (0.0, 0.0, fill_percent(picture, frame));
                    touched.changed = true;
                }
                let (on, tip) = on_picture;
                ui.toggle_value(on, "On the picture").on_hover_text(tip);
            });
            // One click for the usual places: a small picture in a
            // corner (a facecam over a recording), or one half of the
            // frame (two pictures stacked on a vertical timeline).
            let now = values.look(frame, &before);
            let mut placed = None;
            ui.horizontal_wrapped(|ui| {
                inspector::label_cell(ui, theme, "Layout", 86.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (label, tip, right, bottom) in [
                    ("↖", "Small, in the top left corner", false, false),
                    ("↗", "Small, in the top right corner", true, false),
                    ("↙", "Small, in the bottom left corner", false, true),
                    ("↘", "Small, in the bottom right corner", true, true),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        placed = Some(in_corner(&now, picture, frame, right, bottom));
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(90.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (label, tip, bottom) in [
                    ("Top half", "Fills the top half of the frame, cut to fit", false),
                    (
                        "Bottom half",
                        "Fills the bottom half of the frame, cut to fit",
                        true,
                    ),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        placed = Some(in_half(&now, picture, frame, bottom));
                    }
                }
            });
            if let Some(look) = placed {
                *values = Values::of(&look, frame);
                touched.changed = true;
            }
        },
    );
    if clicks.reset {
        asks.resets.push(&TRANSFORM);
    }

    let clicks = inspector::animated_section(
        ui,
        theme,
        "Crop",
        &format!("{}{}", over_time(keys, &CROP_FIELDS), crop_summary(&before)),
        before.is_cropped(),
        keys.animates(&CROP_FIELDS),
        |ui| {
            for ((label, value), field) in ["Left", "Top", "Right", "Bottom"]
                .into_iter()
                .zip(&mut values.crop)
                .zip(CROP_FIELDS)
            {
                let keyed = inspector::keyed_slider(ui, theme, label, value, CROP, &k(field));
                asks.row(touched, field, keyed);
            }
        },
    );
    if clicks.reset {
        asks.resets.push(&CROP_FIELDS);
    }

    colour_section(ui, theme, &before, keys, values, touched, &mut asks);
    asks
}

/// The Blend row: how the picture mixes with what is below it. The mode
/// picked, if another one was.
fn blend_row(ui: &mut egui::Ui, theme: &Theme, mode: project::BlendMode) -> Option<project::BlendMode> {
    let mut picked = None;
    ui.horizontal(|ui| {
        inspector::label_cell(ui, theme, "Blend", inspector::LABEL_WIDTH);
        egui::ComboBox::from_id_salt("blend mode")
            .selected_text(mode.name())
            .show_ui(ui, |ui| {
                for other in project::BlendMode::ALL {
                    if ui.selectable_label(other == mode, other.name()).clicked() && other != mode {
                        picked = Some(other);
                    }
                }
            })
            .response
            .on_hover_text("How the picture mixes with what is below it: Add, Screen and Lighten brighten, Multiply darkens");
    });
    picked
}

/// The Quick color section.
fn colour_section(
    ui: &mut egui::Ui,
    theme: &Theme,
    before: &Look,
    keys: &KeyTable,
    values: &mut Values,
    touched: &mut Touched,
    asks: &mut Asks,
) {
    let eases = Ease::ALL.map(Ease::label);
    let k = |field| keys.keys(field, &eases);
    let clicks = inspector::animated_section(
        ui,
        theme,
        "Quick color",
        &format!("{}{}", over_time(keys, &COLOUR), colour_summary(before)),
        before.is_coloured(),
        keys.animates(&COLOUR),
        |ui| {
            let keyed = inspector::keyed_slider(
                ui,
                theme,
                "Exposure",
                &mut values.exposure,
                EXPOSURE,
                &k(LookField::Exposure),
            );
            asks.row(touched, LookField::Exposure, keyed);
            for (label, value, field) in [
                ("Contrast", &mut values.contrast, LookField::Contrast),
                ("Saturation", &mut values.saturation, LookField::Saturation),
                ("Temperature", &mut values.temperature, LookField::Temperature),
                ("Tint", &mut values.tint, LookField::Tint),
            ] {
                let keyed = inspector::keyed_slider(ui, theme, label, value, EITHER_WAY, &k(field));
                asks.row(touched, field, keyed);
            }
        },
    );
    if clicks.reset {
        asks.resets.push(&COLOUR);
    }
}
