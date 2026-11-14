//! The inspector's Effects section: a picture's effects in the order they
//! are applied, each with its settings, a switch, arrows to move it and a
//! button to take it away; "Add effect" puts a new one at the end.
//!
//! A setting being dragged is kept by the inspector (shown in the viewer at
//! once) and becomes one undo step when let go (`SetEffects`); a click on a
//! switch, an arrow, a remove button or Add is one step at once.

use project::effects::{Blur, Glow, Grain, Pan, Pixelate, Shake, Sharpen, Spot, Vignette, Zoom};
use project::look::ONE;
use project::{ClipId, Command, EffectKind, Effects, SequenceId};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number, Touched};

/// What the section asked for.
#[derive(Debug, Default)]
pub struct Asked {
    /// Finished changes: each one undo step.
    pub commands: Vec<Command>,
    /// The effects as a setting being dragged leaves them, not yet a step.
    pub dragging: Option<Effects>,
    /// Whether a setting is still held: no step yet.
    pub busy: bool,
}

/// Percent, 0 to 100, in steps of one.
const PERCENT: Number = Number {
    range: (0.0, 100.0),
    per_point: 0.5,
    decimals: 0,
    suffix: " %",
    usual: 0.0,
};

/// The section for `clip`'s effects as `shown` (with a drag in progress).
pub fn section(
    ui: &mut egui::Ui,
    theme: &Theme,
    sequence: SequenceId,
    clip: ClipId,
    shown: &Effects,
) -> Asked {
    let mut asked = Asked::default();
    let mut changed = shown.clone();
    let mut touched = Touched::default();
    let mut clicked: Option<Effects> = None;
    let mut add: Option<EffectKind> = None;
    let clicks = inspector::section(ui, theme, "Effects", &summary(shown), !shown.is_empty(), |ui| {
        let count = shown.0.len();
        for i in 0..count {
            let effect = changed.0[i];
            ui.horizontal(|ui| {
                let mut on = !effect.off;
                if ui
                    .checkbox(&mut on, egui::RichText::new(effect.kind.name()).strong())
                    .on_hover_text(if on {
                        "Switch it off (kept, with its settings)"
                    } else {
                        "Switch it on"
                    })
                    .changed()
                {
                    let mut next = shown.clone();
                    next.0[i].off = !on;
                    clicked = Some(next);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Take it away").clicked() {
                        let mut next = shown.clone();
                        next.0.remove(i);
                        clicked = Some(next);
                    }
                    let later = ui.add_enabled(i + 1 < count, egui::Button::new("↓").small());
                    if later
                        .on_hover_text("Apply it later (after the one below)")
                        .clicked()
                    {
                        let mut next = shown.clone();
                        next.0.swap(i, i + 1);
                        clicked = Some(next);
                    }
                    let sooner = ui.add_enabled(i > 0, egui::Button::new("↑").small());
                    if sooner
                        .on_hover_text("Apply it sooner (before the one above)")
                        .clicked()
                    {
                        let mut next = shown.clone();
                        next.0.swap(i, i - 1);
                        clicked = Some(next);
                    }
                });
            });
            ui.add_enabled_ui(!effect.off, |ui| {
                let (kind, t) = settings(ui, theme, effect.kind);
                changed.0[i].kind = kind;
                touched = touched.and(t);
            });
            ui.add_space(4.0);
        }
        if count == 0 {
            inspector::note(
                ui,
                theme,
                "None yet. Effects change this clip's picture, in the order listed.",
            );
        }
        ui.menu_button("Add effect", |ui| {
            for kind in EffectKind::ALL {
                if ui.button(kind.name()).clicked() {
                    add = Some(kind);
                    ui.close();
                }
            }
        });
    });
    let set = |effects: Effects| Command::SetEffects {
        sequence,
        clip,
        effects: Box::new(effects),
    };
    if clicks.reset {
        asked.commands.push(set(Effects::default()));
    } else if let Some(next) = clicked {
        asked.commands.push(set(next));
    } else if let Some(kind) = add {
        asked.commands.push(Command::AddEffect { sequence, clip, kind });
    } else if touched.changed || touched.busy {
        asked.dragging = Some(changed);
        asked.busy = touched.busy;
    }
    asked
}

/// The settings of one effect, as sliders; returns them as changed.
fn settings(ui: &mut egui::Ui, theme: &Theme, kind: EffectKind) -> (EffectKind, Touched) {
    // Shared by the sliders' and the box's closures.
    let touched = std::cell::Cell::new(Touched::default());
    let slider = |ui: &mut egui::Ui, label: &str, value: i32, usual: i32| {
        let mut shown = percent(value);
        let moved = inspector::slider(ui, theme, label, &mut shown, PERCENT.usual_at(usual));
        touched.set(touched.get().and(moved));
        millionths(shown)
    };
    // A share shown as a size: 0 is 100 %, `ONE` is `most` %.
    let size = |ui: &mut egui::Ui, label: &str, value: i32, usual: i32, most: f64| {
        let as_size = |share: i32| 100.0 + (most - 100.0) * f64::from(share) / f64::from(ONE);
        let mut shown = as_size(value);
        let how = Number {
            range: (100.0, most),
            per_point: (most - 100.0) / 200.0,
            decimals: 0,
            suffix: " %",
            usual: as_size(usual),
        };
        let moved = inspector::slider(ui, theme, label, &mut shown, how);
        touched.set(touched.get().and(moved));
        ((shown - 100.0) / (most - 100.0) * f64::from(ONE))
            .round()
            .clamp(0.0, f64::from(ONE)) as i32
    };
    let spot = |ui: &mut egui::Ui, spot: Spot| {
        let mut whole = spot.is_whole();
        if ui
            .checkbox(&mut whole, "Whole picture")
            .on_hover_text("Untick to apply it to a box only: to hide a name or the chat")
            .changed()
        {
            touched.set(touched.get().and(Touched {
                changed: true,
                busy: false,
            }));
            return if whole { Spot::WHOLE } else { Spot::MIDDLE };
        }
        if whole {
            return spot;
        }
        let mut left = percent(spot.left);
        let mut top = percent(spot.top);
        let mut width = percent(spot.width);
        let mut height = percent(spot.height);
        let middle = Spot::MIDDLE;
        let moved = inspector::slider(ui, theme, "Left", &mut left, PERCENT.usual_at(middle.left))
            .and(inspector::slider(
                ui,
                theme,
                "Top",
                &mut top,
                PERCENT.usual_at(middle.top),
            ))
            .and(inspector::slider(
                ui,
                theme,
                "Width",
                &mut width,
                PERCENT.usual_at(middle.width),
            ))
            .and(inspector::slider(
                ui,
                theme,
                "Height",
                &mut height,
                PERCENT.usual_at(middle.height),
            ));
        touched.set(touched.get().and(moved));
        fitted_spot(
            millionths(left),
            millionths(top),
            millionths(width),
            millionths(height),
        )
    };
    let changed = match kind {
        EffectKind::Vignette(v) => {
            let usual = Vignette::USUAL;
            EffectKind::Vignette(Vignette {
                amount: slider(ui, "Amount", v.amount, usual.amount),
                size: slider(ui, "Size", v.size, usual.size),
                softness: slider(ui, "Softness", v.softness, usual.softness),
            })
        }
        EffectKind::Grain(g) => {
            let usual = Grain::USUAL;
            EffectKind::Grain(Grain {
                amount: slider(ui, "Amount", g.amount, usual.amount),
                size: slider(ui, "Size", g.size, usual.size),
            })
        }
        EffectKind::Sharpen(s) => {
            let usual = Sharpen::USUAL;
            EffectKind::Sharpen(Sharpen {
                amount: slider(ui, "Amount", s.amount, usual.amount),
                radius: slider(ui, "Size", s.radius, usual.radius),
            })
        }
        EffectKind::Glow(g) => {
            let usual = Glow::USUAL;
            EffectKind::Glow(Glow {
                amount: slider(ui, "Amount", g.amount, usual.amount),
                threshold: slider(ui, "Threshold", g.threshold, usual.threshold),
                radius: slider(ui, "Spread", g.radius, usual.radius),
            })
        }
        EffectKind::Blur(b) => {
            let radius = slider(ui, "Amount", b.radius, Blur::USUAL.radius);
            EffectKind::Blur(Blur {
                radius,
                spot: spot(ui, b.spot),
            })
        }
        EffectKind::Pixelate(p) => {
            let blocks = slider(ui, "Block size", p.size, Pixelate::USUAL.size);
            EffectKind::Pixelate(Pixelate {
                size: blocks,
                spot: spot(ui, p.spot),
            })
        }
        EffectKind::Zoom(z) => {
            let usual = Zoom::USUAL;
            EffectKind::Zoom(Zoom {
                amount: size(ui, "Zoom at end", z.amount, usual.amount, 300.0),
                x: slider(ui, "Point across", z.x, usual.x),
                y: slider(ui, "Point down", z.y, usual.y),
            })
        }
        EffectKind::Shake(k) => {
            let usual = Shake::USUAL;
            EffectKind::Shake(Shake {
                amount: slider(ui, "Amount", k.amount, usual.amount),
                speed: slider(ui, "Speed", k.speed, usual.speed),
            })
        }
        EffectKind::Pan(p) => {
            let usual = Pan::USUAL;
            EffectKind::Pan(Pan {
                zoom: size(ui, "Zoom", p.zoom, usual.zoom, 200.0),
                from_x: slider(ui, "From across", p.from_x, usual.from_x),
                from_y: slider(ui, "From down", p.from_y, usual.from_y),
                to_x: slider(ui, "To across", p.to_x, usual.to_x),
                to_y: slider(ui, "To down", p.to_y, usual.to_y),
            })
        }
    };
    (changed, touched.get())
}

/// A box from its sliders, kept inside the picture: a box pushed past an
/// edge is moved back rather than refused, and is never less than 1 % across.
fn fitted_spot(left: i32, top: i32, width: i32, height: i32) -> Spot {
    let least = ONE / 100;
    let width = width.clamp(least, ONE);
    let height = height.clamp(least, ONE);
    Spot {
        left: left.clamp(0, ONE - width),
        top: top.clamp(0, ONE - height),
        width,
        height,
    }
}

trait UsualAt {
    fn usual_at(self, millionths: i32) -> Number;
}

impl UsualAt for Number {
    fn usual_at(self, millionths: i32) -> Number {
        Number {
            usual: percent(millionths),
            ..self
        }
    }
}

fn percent(millionths: i32) -> f64 {
    f64::from(millionths) * 100.0 / f64::from(ONE)
}

fn millionths(percent: f64) -> i32 {
    (percent.clamp(0.0, 100.0) * f64::from(ONE) / 100.0).round() as i32
}

/// The section's line while folded: the effects that are on, in order.
pub fn summary(effects: &Effects) -> String {
    let on: Vec<&str> = effects.applied().map(EffectKind::name).collect();
    match (on.is_empty(), effects.is_empty()) {
        (_, true) => "None".to_owned(),
        (true, false) => "All switched off".to_owned(),
        (false, _) => on.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percents_go_to_millionths_and_back() {
        for millionths_in in [0, 1, 333_333, 500_000, ONE] {
            let back = millionths(percent(millionths_in));
            assert!(
                (back - millionths_in).abs() <= 1,
                "{millionths_in} came back as {back}"
            );
        }
        assert_eq!(millionths(150.0), ONE, "kept in range");
    }

    #[test]
    fn a_box_stays_inside_the_picture() {
        let spot = fitted_spot(900_000, 0, 300_000, 0);
        assert_eq!((spot.left, spot.width), (700_000, 300_000), "moved back in");
        assert_eq!(spot.height, ONE / 100, "never nothing");
        assert_eq!(fitted_spot(0, 0, ONE, ONE), Spot::WHOLE);
    }

    #[test]
    fn the_summary_names_what_is_on() {
        assert_eq!(summary(&Effects::default()), "None");
        let (mut effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        assert_eq!(summary(&effects), "Vignette");
        effects.0[0].off = true;
        assert_eq!(summary(&effects), "All switched off");
    }
}
