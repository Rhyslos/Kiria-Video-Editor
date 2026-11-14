//! The inspector's Effects section: a picture's effects in the order they
//! are applied, each with its settings, a switch, arrows to move it and a
//! button to take it away; "Add effect" puts a new one at the end.
//!
//! A setting being dragged is kept by the inspector (shown in the viewer at
//! once) and becomes one undo step when let go (`SetEffects`); a click on a
//! switch, an arrow, a remove button or Add is one step at once.

use project::effects::Vignette;
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
    match kind {
        EffectKind::Vignette(v) => {
            let mut amount = percent(v.amount);
            let mut size = percent(v.size);
            let mut softness = percent(v.softness);
            let usual = Vignette::USUAL;
            let touched = inspector::slider(ui, theme, "Amount", &mut amount, PERCENT.usual_at(usual.amount))
                .and(inspector::slider(
                    ui,
                    theme,
                    "Size",
                    &mut size,
                    PERCENT.usual_at(usual.size),
                ))
                .and(inspector::slider(
                    ui,
                    theme,
                    "Softness",
                    &mut softness,
                    PERCENT.usual_at(usual.softness),
                ));
            let changed = Vignette {
                amount: millionths(amount),
                size: millionths(size),
                softness: millionths(softness),
            };
            (EffectKind::Vignette(changed), touched)
        }
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
    fn the_summary_names_what_is_on() {
        assert_eq!(summary(&Effects::default()), "None");
        let (mut effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        assert_eq!(summary(&effects), "Vignette");
        effects.0[0].off = true;
        assert_eq!(summary(&effects), "All switched off");
    }
}
