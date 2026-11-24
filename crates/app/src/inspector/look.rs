//! A look's numbers as the inspector shows them, and the arithmetic behind
//! its buttons and headers: [`Values`] (pixels, percent and degrees, read
//! back without changing what wasn't touched), the ranges each number
//! moves in, Fit and Fill, the corner and half layouts, and each picture
//! section's one-line summary. Nothing here draws.

use project::Look;
use project::look::ONE;
use ui_kit::inspector::Number;

/// A look's numbers as the inspector shows them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Values {
    /// The picture's centre from the frame's, in frame pixels.
    pub(super) x: f64,
    pub(super) y: f64,
    /// Percent of the size that fits the frame.
    pub(super) scale: f64,
    /// Degrees clockwise.
    pub(super) turn: f64,
    /// Percent cut off the left, top, right and bottom.
    pub(super) crop: [f64; 4],
    /// Percent.
    pub(super) opacity: f64,
    /// Stops.
    pub(super) exposure: f64,
    /// -100 to 100.
    pub(super) contrast: f64,
    pub(super) saturation: f64,
    pub(super) temperature: f64,
    pub(super) tint: f64,
}

const ONE_F: f64 = ONE as f64;

pub(super) fn pixels(side: u32) -> Number {
    let reach = f64::from(Look::AWAY) / ONE_F * f64::from(side);
    Number {
        range: (-reach, reach),
        per_point: 1.0,
        decimals: 0,
        suffix: " px",
        usual: 0.0,
    }
}

pub(super) const SIZE: Number = Number {
    range: (1.0, 10_000.0),
    per_point: 0.5,
    decimals: 1,
    suffix: " %",
    usual: 100.0,
};
pub(super) const TURN: Number = Number {
    range: (-360.0, 360.0),
    per_point: 0.5,
    decimals: 1,
    suffix: "°",
    usual: 0.0,
};
pub(super) const PERCENT_FULL: Number = Number {
    range: (0.0, 100.0),
    per_point: 1.0,
    decimals: 0,
    suffix: " %",
    usual: 100.0,
};
pub(super) const CROP: Number = Number {
    range: (0.0, 99.0),
    per_point: 0.2,
    decimals: 1,
    suffix: " %",
    usual: 0.0,
};
pub(super) const EXPOSURE: Number = Number {
    range: (-5.0, 5.0),
    per_point: 0.02,
    decimals: 2,
    suffix: "",
    usual: 0.0,
};
pub(super) const EITHER_WAY: Number = Number {
    range: (-100.0, 100.0),
    per_point: 1.0,
    decimals: 0,
    suffix: "",
    usual: 0.0,
};

impl Values {
    pub(super) fn of(look: &Look, frame: (u32, u32)) -> Values {
        let part = |value: i32| f64::from(value) / ONE_F;
        Values {
            x: part(look.x) * f64::from(frame.0),
            y: part(look.y) * f64::from(frame.1),
            scale: part(look.scale) * 100.0,
            turn: part(look.turn),
            crop: [look.crop_left, look.crop_top, look.crop_right, look.crop_bottom]
                .map(|side| part(side) * 100.0),
            opacity: part(look.opacity) * 100.0,
            exposure: part(look.exposure),
            contrast: part(look.contrast) * 100.0,
            saturation: part(look.saturation) * 100.0,
            temperature: part(look.temperature) * 100.0,
            tint: part(look.tint) * 100.0,
        }
    }

    /// The look these numbers mean, within its limits. A number that was
    /// not touched keeps `before`'s exact value, so showing a look and
    /// reading it back never changes it.
    pub(super) fn look(&self, frame: (u32, u32), before: &Look) -> Look {
        let was = Values::of(before, frame);
        let whole = |now: f64, then: f64, stored: i32, one: f64| {
            if now == then {
                stored
            } else {
                (now / one * ONE_F).round() as i32
            }
        };
        Look {
            x: whole(self.x, was.x, before.x, f64::from(frame.0.max(1))),
            y: whole(self.y, was.y, before.y, f64::from(frame.1.max(1))),
            scale: whole(self.scale, was.scale, before.scale, 100.0),
            turn: whole(self.turn, was.turn, before.turn, 1.0),
            crop_left: whole(self.crop[0], was.crop[0], before.crop_left, 100.0),
            crop_top: whole(self.crop[1], was.crop[1], before.crop_top, 100.0),
            crop_right: whole(self.crop[2], was.crop[2], before.crop_right, 100.0),
            crop_bottom: whole(self.crop[3], was.crop[3], before.crop_bottom, 100.0),
            opacity: whole(self.opacity, was.opacity, before.opacity, 100.0),
            exposure: whole(self.exposure, was.exposure, before.exposure, 1.0),
            contrast: whole(self.contrast, was.contrast, before.contrast, 100.0),
            saturation: whole(self.saturation, was.saturation, before.saturation, 100.0),
            temperature: whole(self.temperature, was.temperature, before.temperature, 100.0),
            tint: whole(self.tint, was.tint, before.tint, 100.0),
        }
        .within_limits()
    }
}

/// `look` with the picture as large as fits the frame, centred: what the
/// Fit button and a double-click on the picture in the viewer mean. Its
/// turn, crop, opacity and colour stay.
pub fn fitted(look: &Look) -> Look {
    Look {
        x: 0,
        y: 0,
        scale: ONE,
        ..*look
    }
}

/// The size, in percent of the fitted size, at which a picture of
/// `picture`'s shape just fills a frame of `frame`'s.
pub(super) fn fill_percent(picture: (u32, u32), frame: (u32, u32)) -> f64 {
    let across = f64::from(frame.0) / f64::from(picture.0.max(1));
    let down = f64::from(frame.1) / f64::from(picture.1.max(1));
    across.max(down) / across.min(down) * 100.0
}

/// How large a picture in a corner is, as a share of its fitted size, and
/// how far it stays from the frame's edges, as a share of the frame's width.
const CORNER_SIZE: f64 = 0.3;
const CORNER_MARGIN: f64 = 0.03;

/// The size a picture of `picture`'s shape has when fitted into `frame`, in
/// frame pixels.
fn fitted_size(picture: (u32, u32), frame: (u32, u32)) -> (f64, f64) {
    let (pw, ph) = (f64::from(picture.0.max(1)), f64::from(picture.1.max(1)));
    let scale = (f64::from(frame.0) / pw).min(f64::from(frame.1) / ph);
    (pw * scale, ph * scale)
}

fn millionths(share: f64) -> i32 {
    (share * ONE_F).round() as i32
}

/// `look` with the picture small in a corner of the frame: a third of its
/// fitted size, a little in from the edges, upright and uncut. Its opacity
/// and quick colour stay.
pub(super) fn in_corner(
    look: &Look,
    picture: (u32, u32),
    frame: (u32, u32),
    right: bool,
    bottom: bool,
) -> Look {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (w, h) = fitted_size(picture, frame);
    let (w, h) = (w * CORNER_SIZE, h * CORNER_SIZE);
    let margin = CORNER_MARGIN * fw;
    // The centre's distance from the frame's centre, as a share of the frame.
    let x = (fw / 2.0 - margin - w / 2.0) / fw;
    let y = (fh / 2.0 - margin - h / 2.0) / fh;
    Look {
        x: millionths(if right { x } else { -x }),
        y: millionths(if bottom { y } else { -y }),
        scale: millionths(CORNER_SIZE),
        turn: 0,
        crop_left: 0,
        crop_top: 0,
        crop_right: 0,
        crop_bottom: 0,
        ..*look
    }
    .within_limits()
}

/// `look` with the picture filling the top or bottom half of the frame:
/// large enough to cover it, centred in it, and what sticks out cut off.
pub(super) fn in_half(look: &Look, picture: (u32, u32), frame: (u32, u32), bottom: bool) -> Look {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (w, h) = fitted_size(picture, frame);
    let (half_w, half_h) = (fw, fh / 2.0);
    // Covers the half: the larger of the two scales.
    let scale = (half_w / w).max(half_h / h);
    let (w, h) = (w * scale, h * scale);
    let cut_x = ((w - half_w) / 2.0 / w).max(0.0);
    let cut_y = ((h - half_h) / 2.0 / h).max(0.0);
    Look {
        x: 0,
        y: millionths(if bottom { 0.25 } else { -0.25 }),
        scale: millionths(scale),
        turn: 0,
        crop_left: millionths(cut_x),
        crop_right: millionths(cut_x),
        crop_top: millionths(cut_y),
        crop_bottom: millionths(cut_y),
        ..*look
    }
    .within_limits()
}

/// `look` with `crop` (shares of the picture's width and height cut off
/// the left, top, right and bottom) and the part left as large as fits
/// the frame, centred, upright: the bars of a recording cut away, or its
/// shape changed. Its opacity and quick colour stay.
pub fn cropped_to_fit(look: &Look, picture: (u32, u32), crop: [f64; 4], frame: (u32, u32)) -> Look {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (w, h) = fitted_size(picture, frame);
    let [left, top, right, bottom] = crop.map(|c| c.clamp(0.0, 0.49));
    let (kept_w, kept_h) = (w * (1.0 - left - right), h * (1.0 - top - bottom));
    let scale = (fw / kept_w).min(fh / kept_h);
    // The part's middle on the frame's: the picture moves by how far the
    // part's middle is from its own.
    let x = (right - left) / 2.0 * w * scale / fw;
    let y = (bottom - top) / 2.0 * h * scale / fh;
    Look {
        x: millionths(x),
        y: millionths(y),
        scale: millionths(scale),
        turn: 0,
        crop_left: millionths(left),
        crop_top: millionths(top),
        crop_right: millionths(right),
        crop_bottom: millionths(bottom),
        ..*look
    }
    .within_limits()
}

/// The crop that gives a picture of `picture`'s size the shape `wide` to
/// `high`, cutting the same off both sides of the long way.
pub(crate) fn crop_to_shape(picture: (u32, u32), (wide, high): (u32, u32)) -> [f64; 4] {
    let (w, h) = (f64::from(picture.0.max(1)), f64::from(picture.1.max(1)));
    let shape = f64::from(wide.max(1)) / f64::from(high.max(1));
    if w / h > shape {
        let cut = (1.0 - shape * h / w) / 2.0;
        [cut, 0.0, cut, 0.0]
    } else {
        let cut = (1.0 - w / shape / h) / 2.0;
        [0.0, cut, 0.0, cut]
    }
}

/// The size in pixels of what `look`'s crop leaves of a picture of
/// `picture`'s size, both sides even.
pub(crate) fn cropped_size(look: &Look, picture: (u32, u32)) -> (u32, u32) {
    let share = |a: i32, b: i32| 1.0 - f64::from(a + b) / ONE_F;
    let even = |side: u32, kept: f64| (((f64::from(side) * kept).round() as u32).max(2) + 1) & !1;
    (
        even(picture.0, share(look.crop_left, look.crop_right)),
        even(picture.1, share(look.crop_top, look.crop_bottom)),
    )
}

/// A number without a trailing ".0".
pub(super) fn short(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text,
    }
}

/// With its sign, also when positive.
fn signed(value: f64, decimals: usize) -> String {
    let text = short(value.abs(), decimals);
    if value < 0.0 {
        format!("−{text}")
    } else {
        format!("+{text}")
    }
}

pub(super) fn transform_summary(look: &Look, frame: (u32, u32)) -> String {
    let values = Values::of(look, frame);
    let mut parts = Vec::new();
    if look.scale != ONE {
        parts.push(format!("{} %", short(values.scale, 1)));
    }
    if (look.x, look.y) != (0, 0) {
        parts.push(format!("at {}, {}", signed(values.x, 0), signed(values.y, 0)));
    }
    if look.turn != 0 {
        parts.push(format!("turned {}°", short(values.turn, 1)));
    }
    if look.opacity != ONE {
        parts.push(format!("{} % opaque", short(values.opacity, 0)));
    }
    if parts.is_empty() {
        "Fitted, centred".to_owned()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn crop_summary(look: &Look) -> String {
    let values = Values::of(look, (1, 1));
    let parts: Vec<String> = ["Left", "Top", "Right", "Bottom"]
        .iter()
        .zip(values.crop)
        .filter(|(_, cut)| *cut > 0.0)
        .map(|(side, cut)| format!("{side} {} %", short(cut, 1)))
        .collect();
    if parts.is_empty() {
        "Nothing cut off".to_owned()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn colour_summary(look: &Look) -> String {
    let values = Values::of(look, (1, 1));
    let mut parts = Vec::new();
    if look.exposure != 0 {
        parts.push(format!("Exposure {}", signed(values.exposure, 2)));
    }
    for (name, value) in [
        ("Contrast", values.contrast),
        ("Saturation", values.saturation),
        ("Temperature", values.temperature),
        ("Tint", values.tint),
    ] {
        if value != 0.0 {
            parts.push(format!("{name} {}", signed(value, 0)));
        }
    }
    if parts.is_empty() {
        "As it is".to_owned()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_cropped_out_of_a_wide_recording_fills_its_own_timeline() {
        // 2560 in the middle of 3440, on a 3440×1440 timeline.
        let picture = (3440, 1440);
        let crop = [440.0 / 3440.0, 0.0, 440.0 / 3440.0, 0.0];
        let look = cropped_to_fit(&Look::PLAIN, picture, crop, (3440, 1440));
        assert_eq!(cropped_size(&look, picture), (2560, 1440));
        // As large as fits: as high as the frame, so the scale is the full
        // height's (the whole picture already fitted that), centred.
        assert_eq!((look.x, look.y), (0, 0));
        assert_eq!(look.scale, ONE);
        // On a 2560×1440 timeline it fills the frame: the whole picture is
        // 3440/2560 of its fitted size.
        let filled = cropped_to_fit(&Look::PLAIN, picture, crop, (2560, 1440));
        assert!((f64::from(filled.scale) / ONE_F - 3440.0 / 2560.0).abs() < 1e-5);
        assert_eq!((filled.x, filled.y), (0, 0));
    }

    #[test]
    fn bars_on_one_side_move_the_picture_so_what_is_left_is_centred() {
        // Black only on the left 20 %, on a frame of the part's shape.
        let picture = (1000, 500);
        let look = cropped_to_fit(&Look::PLAIN, picture, [0.2, 0.0, 0.0, 0.0], (800, 500));
        // The part fills the frame; the picture moves left by half the bar.
        assert!((f64::from(look.scale) / ONE_F - 1.25).abs() < 1e-5);
        let x = f64::from(look.x) / ONE_F * 800.0;
        assert!((x - -100.0).abs() < 0.5, "{x}");
    }

    #[test]
    fn a_shape_is_cut_from_the_long_way() {
        let wide = crop_to_shape((3440, 1440), (16, 9));
        assert!((wide[0] - 440.0 / 3440.0).abs() < 1e-9 && wide[1] == 0.0);
        let tall = crop_to_shape((1920, 1080), (9, 16));
        assert!((1920.0 * (1.0 - 2.0 * tall[0]) - 607.5).abs() < 0.01);
        let high = crop_to_shape((1080, 1920), (1, 1));
        assert_eq!((high[0], high[2]), (0.0, 0.0));
        assert!((1920.0 * (1.0 - 2.0 * high[1]) - 1080.0).abs() < 0.01);
    }

    const FRAME: (u32, u32) = (1920, 1080);

    #[test]
    fn showing_a_look_and_reading_it_back_changes_nothing() {
        let look = Look {
            x: 333_333,
            y: -123_457,
            scale: 314_159,
            turn: -2_500_001,
            crop_left: 120_001,
            crop_bottom: 49_999,
            opacity: 899_999,
            exposure: 250_001,
            temperature: 150_003,
            ..Look::PLAIN
        };
        assert_eq!(Values::of(&look, FRAME).look(FRAME, &look), look);
        assert_eq!(
            Values::of(&Look::PLAIN, FRAME).look(FRAME, &Look::PLAIN),
            Look::PLAIN
        );
    }

    #[test]
    fn numbers_are_pixels_percent_and_degrees() {
        let look = Look {
            x: ONE / 4,
            y: -ONE / 10,
            scale: ONE * 3 / 10,
            turn: 90 * ONE,
            crop_left: ONE / 8,
            opacity: ONE / 2,
            ..Look::PLAIN
        };
        let values = Values::of(&look, FRAME);
        assert_eq!((values.x, values.y), (480.0, -108.0));
        assert_eq!((values.scale, values.turn, values.opacity), (30.0, 90.0, 50.0));
        assert_eq!(values.crop, [12.5, 0.0, 0.0, 0.0]);

        // A changed number is stored to the nearest millionth, and kept within limits.
        let mut changed = values;
        changed.x = 96.0;
        changed.scale = 0.2;
        let stored = changed.look(FRAME, &look);
        assert_eq!(stored.x, 50_000);
        assert_eq!(stored.scale, Look::SCALE.0, "never smaller than a hundredth");
        assert_eq!(stored.y, look.y, "what wasn't touched is exactly as before");
    }

    #[test]
    fn fill_is_how_much_larger_than_fitted_covers_the_frame() {
        assert_eq!(fill_percent((1920, 1080), (1920, 1080)), 100.0);
        // A wide recording on a tall timeline: fitted it is 1080 wide and
        // 607.5 high; to cover 1920 of height it must be 3.16 times that.
        let percent = fill_percent((1920, 1080), (1080, 1920));
        assert!((percent - 316.05).abs() < 0.01, "{percent}");
    }

    #[test]
    fn headers_say_what_is_set_in_a_line() {
        assert_eq!(transform_summary(&Look::PLAIN, FRAME), "Fitted, centred");
        let corner = Look {
            scale: 300_000,
            x: 320_000,
            y: 280_000,
            turn: -2_500_000,
            opacity: 900_000,
            ..Look::PLAIN
        };
        assert_eq!(
            transform_summary(&corner, FRAME),
            "30 % · at +614, +302 · turned -2.5° · 90 % opaque"
        );
        assert_eq!(crop_summary(&Look::PLAIN), "Nothing cut off");
        let cut = Look {
            crop_left: 120_000,
            crop_bottom: 50_000,
            ..Look::PLAIN
        };
        assert_eq!(crop_summary(&cut), "Left 12 % · Bottom 5 %");
        assert_eq!(colour_summary(&Look::PLAIN), "As it is");
        let warm = Look {
            exposure: 250_000,
            temperature: 150_000,
            saturation: -100_000,
            ..Look::PLAIN
        };
        assert_eq!(
            colour_summary(&warm),
            "Exposure +0.25 · Saturation −10 · Temperature +15"
        );
    }

    #[test]
    fn a_corner_layout_is_a_third_of_the_size_a_little_in_from_the_edges() {
        let tinted = Look {
            opacity: ONE / 2,
            temperature: 100_000,
            crop_left: 200_000,
            turn: 5 * ONE,
            ..Look::PLAIN
        };
        let corner = in_corner(&tinted, (1920, 1080), FRAME, true, true);
        assert_eq!(corner.scale, 300_000);
        // 576 x 324, 57.6 px in from the right and the bottom edge.
        let values = Values::of(&corner, FRAME);
        assert!((values.x - (960.0 - 57.6 - 288.0)).abs() < 0.01, "{}", values.x);
        assert!((values.y - (540.0 - 57.6 - 162.0)).abs() < 0.01, "{}", values.y);
        assert_eq!((corner.turn, corner.crop_left), (0, 0), "upright and uncut");
        assert_eq!(
            (corner.opacity, corner.temperature),
            (ONE / 2, 100_000),
            "the rest stays"
        );
        let top_left = in_corner(&tinted, (1920, 1080), FRAME, false, false);
        assert_eq!((top_left.x, top_left.y), (-corner.x, -corner.y));
    }

    #[test]
    fn a_half_layout_covers_its_half_and_cuts_what_sticks_out() {
        // A wide recording on a tall timeline: fitted it is 1080 x 607.5.
        let tall = (1080, 1920);
        let top = in_half(&Look::PLAIN, (1920, 1080), tall, false);
        // To cover 1080 x 960 it is scaled by 960 / 607.5.
        assert_eq!(top.scale, 1_580_247);
        assert_eq!(top.y, -250_000);
        // 1706.7 wide then: 313.3 cut off each side, nothing above or below.
        assert!(
            (f64::from(top.crop_left) / ONE_F - 313.33 / 1706.67).abs() < 1e-4,
            "{}",
            top.crop_left
        );
        assert_eq!(top.crop_left, top.crop_right);
        assert_eq!((top.crop_top, top.crop_bottom), (0, 0));
        assert_eq!(in_half(&Look::PLAIN, (1920, 1080), tall, true).y, 250_000);
        // On a wide frame the half is wider than high: top and bottom are cut.
        let wide = in_half(&Look::PLAIN, (1920, 1080), FRAME, false);
        assert_eq!(wide.scale, ONE);
        assert_eq!(
            (wide.crop_left, wide.crop_top, wide.crop_bottom),
            (0, 250_000, 250_000)
        );
    }
}
