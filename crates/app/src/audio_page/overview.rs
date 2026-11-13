//! The full loudness overview (the Meters tab): momentary, short-term and
//! integrated loudness, loudness range and peak for the whole mix and the
//! track the tools are on, large, against the loudness aimed for (the same
//! setting as the master column's and Settings'), with how integrated
//! loudness, range and peaks are counted chosen here.

use super::counting::{Counted, Counting};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use ui_kit::Theme;
use ui_kit::master::{AgainstTarget, LoudnessTarget, TRUE_PEAK_CEILING, against, db_text};

/// What the overview shows of one meter now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Now {
    pub momentary: f32,
    pub short_term: f32,
    pub integrated: f32,
    pub range: f32,
    pub peak: f32,
}

impl Now {
    pub const SILENT: Now = Now {
        momentary: f32::NEG_INFINITY,
        short_term: f32::NEG_INFINITY,
        integrated: f32::NEG_INFINITY,
        range: f32::NEG_INFINITY,
        peak: f32::NEG_INFINITY,
    };
}

/// What the overview asks of the page.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Asks {
    pub counting: Option<Counting>,
    pub target: Option<LoudnessTarget>,
    pub reset: bool,
}

/// The overview in the tool's room. `track` is the track's name and what
/// its meter says (its peak is a sample peak: tracks have no true-peak
/// meter); `mix` the whole mix's.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut Ui,
    theme: &Theme,
    mix: Now,
    track: Option<(&str, Now)>,
    counted: &Counted,
    counting: Counting,
    target: LoudnessTarget,
) -> Asks {
    let mut asks = Asks::default();
    let full = ui.available_rect_before_wrap();
    let controls_h = 26.0;
    let body = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - controls_h - 22.0));
    let gap = 12.0;
    let half = (body.width() - gap) / 2.0;
    let left = Rect::from_min_size(body.min, vec2(half, body.height()));
    let right = Rect::from_min_size(pos2(left.right() + gap, body.top()), vec2(half, body.height()));
    column(ui, theme, left, "Whole mix", mix, target, "True peak", true);
    match track {
        Some((name, now)) => column(ui, theme, right, name, now, target, "Peak", false),
        None => {
            ui.painter_at(right).text(
                right.center(),
                Align2::CENTER_CENTER,
                "No audio track",
                FontId::proportional(12.0),
                theme.muted,
            );
        }
    }
    // Counting and the aim.
    let row = Rect::from_min_size(
        pos2(full.left(), body.bottom() + 6.0),
        vec2(full.width(), controls_h),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui2 = &mut child;
    ui2.label(egui::RichText::new("Count").color(theme.text_secondary));
    for c in Counting::ALL {
        if ui2
            .selectable_label(c == counting, c.label())
            .on_hover_text(c.explained())
            .clicked()
            && c != counting
        {
            asks.counting = Some(c);
        }
    }
    if ui2
        .add_enabled(counted.mix.readings() > 0, egui::Button::new("Reset"))
        .on_hover_text("Start integrated loudness, range and peaks again")
        .clicked()
    {
        asks.reset = true;
    }
    ui2.add_space(8.0);
    ui2.label(egui::RichText::new("Aim").color(theme.text_secondary));
    egui::ComboBox::from_id_salt("overview-aim")
        .selected_text(target.label())
        .width(150.0)
        .show_ui(ui2, |ui| {
            for t in LoudnessTarget::ALL {
                if ui.selectable_label(t == target, t.label()).clicked() && t != target {
                    asks.target = Some(t);
                }
            }
        })
        .response
        .on_hover_text("The same loudness the master column and Settings › Audio aim for.");
    let seconds = counted.seconds();
    let said = if seconds > 0.0 {
        format!(
            "Integrated, range and peaks: {} of sound counted ({}).",
            ui_kit::time_text::duration(seconds.round() as u64),
            counting.label().to_lowercase()
        )
    } else {
        "Integrated, range and peaks: nothing counted yet, play the timeline.".to_owned()
    };
    ui.painter_at(full).text(
        pos2(full.left(), row.bottom() + 4.0),
        Align2::LEFT_TOP,
        said,
        FontId::proportional(11.5),
        theme.muted,
    );
    ui.allocate_rect(full, Sense::hover());
    asks
}

/// The colour an integrated loudness is shown in against the aim.
pub fn integrated_colour(theme: &Theme, integrated: f32, target: LoudnessTarget) -> Color32 {
    match against(integrated, target) {
        AgainstTarget::OnIt => ui_kit::status::GOOD,
        AgainstTarget::Louder => ui_kit::status::WARNING,
        AgainstTarget::Quieter | AgainstTarget::Unknown => theme.text,
    }
}

#[allow(clippy::too_many_arguments)]
fn column(
    ui: &Ui,
    theme: &Theme,
    r: Rect,
    title: &str,
    now: Now,
    target: LoudnessTarget,
    peak_name: &str,
    true_peak: bool,
) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 6.0, theme.bg);
    p.text(
        r.left_top() + vec2(10.0, 8.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(12.5),
        theme.text_secondary,
    );
    // Integrated, large, with the aim beside it.
    let big = (r.height() * 0.24).clamp(18.0, 40.0);
    let i_top = r.top() + 28.0;
    let integrated = db_text(now.integrated);
    p.text(
        pos2(r.left() + 10.0, i_top),
        Align2::LEFT_TOP,
        &integrated,
        ui_kit::fonts::mono(big),
        integrated_colour(theme, now.integrated, target),
    );
    p.text(
        pos2(r.left() + 10.0, i_top + big + 2.0),
        Align2::LEFT_TOP,
        "LUFS integrated",
        FontId::proportional(10.5),
        theme.muted,
    );
    if let Some(aim) = target.lufs() {
        p.text(
            pos2(r.right() - 10.0, i_top + 4.0),
            Align2::RIGHT_TOP,
            format!("aim {aim:.0}"),
            FontId::proportional(11.0),
            theme.text_secondary,
        );
        // How far off: a short scale ±6 LU around the aim.
        let scale = Rect::from_min_size(pos2(r.right() - 90.0, i_top + 22.0), vec2(80.0, 6.0));
        p.rect_filled(scale, 3.0, theme.panel);
        let mid = scale.center().x;
        p.line_segment(
            [pos2(mid, scale.top() - 3.0), pos2(mid, scale.bottom() + 3.0)],
            Stroke::new(1.0, ui_kit::status::GOOD),
        );
        if now.integrated.is_finite() {
            let off = ((now.integrated - aim) / 6.0).clamp(-1.0, 1.0);
            p.circle_filled(
                pos2(mid + off * scale.width() / 2.0, scale.center().y),
                4.0,
                integrated_colour(theme, now.integrated, target),
            );
        }
    }
    // The rest two by two: the number, its unit beside it, its name under it.
    let peak_colour = if now.peak > TRUE_PEAK_CEILING {
        ui_kit::status::ERROR
    } else {
        theme.text
    };
    let cells = [
        ("Momentary", db_text(now.momentary), "LUFS", theme.text),
        ("Short-term", db_text(now.short_term), "LUFS", theme.text),
        ("Range", db_text(now.range), "LU", theme.text),
        (
            peak_name,
            db_text(now.peak),
            if true_peak { "dBTP" } else { "dBFS" },
            peak_colour,
        ),
    ];
    let grid_top = i_top + big + 20.0;
    let row_h = ((r.bottom() - grid_top - 6.0) / 2.0).clamp(26.0, 44.0);
    let cell_w = (r.width() - 20.0) / 2.0;
    let mid = (row_h * 0.5).clamp(13.0, 20.0);
    for (i, (name, value, unit, colour)) in cells.into_iter().enumerate() {
        let x = r.left() + 10.0 + (i % 2) as f32 * cell_w;
        let y = grid_top + (i / 2) as f32 * row_h;
        let number = p.layout_no_wrap(value, ui_kit::fonts::mono(mid), colour);
        let w = number.size().x;
        p.galley(pos2(x, y), number, colour);
        p.text(
            pos2(x + w + 4.0, y + mid * 0.9),
            Align2::LEFT_BOTTOM,
            unit,
            FontId::proportional(10.0),
            theme.muted,
        );
        p.text(
            pos2(x, y + mid + 1.0),
            Align2::LEFT_TOP,
            name,
            FontId::proportional(10.0),
            theme.muted,
        );
    }
}
