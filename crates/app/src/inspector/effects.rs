//! The inspector's Effects section: a picture's effects in the order they
//! are applied, each with its settings, a switch, arrows to move it and a
//! button to take it away; "Add effect" puts a new one at the end.
//!
//! A setting being dragged is kept by the inspector (shown in the viewer at
//! once) and becomes one undo step when let go (`SetEffects`); a click on a
//! switch, an arrow, a remove button or Add is one step at once.
//!
//! Every setting has the keyframe control the look's rows have: the
//! diamond adds a keyframe at the playhead (or removes the one there), the
//! arrows go to the one before or after, a right-click on a filled diamond
//! picks its ease. The values shown are the effect's at the playhead.

use project::effects::Spot;
use project::keyframes::Ease;
use project::look::ONE;
use project::{Clip, Command, EffectKind, Effects, Rate, SequenceId, Time};
use ui_kit::Theme;
use ui_kit::inspector::{self, Number, Touched};
use ui_kit::keyframe::{KeyState, Keys};

/// What the section asked for.
#[derive(Debug, Default)]
pub struct Asked {
    /// Finished changes: each one undo step.
    pub commands: Vec<Command>,
    /// The effects as a setting being dragged leaves them, not yet a step.
    pub dragging: Option<Effects>,
    /// Whether a setting is still held: no step yet.
    pub busy: bool,
    /// Where the playhead was asked to go (an arrow beside a keyframe).
    pub seek: Option<Time>,
}

/// Where the playhead is, for the settings shown and the keyframe controls.
#[derive(Debug, Clone, Copy)]
pub struct At<'a> {
    pub clip: &'a Clip,
    pub playhead: Time,
    pub rate: Rate,
}

/// How a setting's whole millionths are shown.
#[derive(Debug, Clone, Copy)]
enum Show {
    /// 0 to 100 %.
    Percent,
    /// As a size: 0 is 100 %, `ONE` is this many %.
    Size(f64),
}

/// Each setting of an effect, in [`EffectKind::numbers`] order: its label
/// and how it is shown.
fn knobs(kind: &EffectKind) -> &'static [(&'static str, Show)] {
    use Show::{Percent, Size};
    const BOX: [(&str, Show); 4] = [
        ("Left", Percent),
        ("Top", Percent),
        ("Width", Percent),
        ("Height", Percent),
    ];
    match kind {
        EffectKind::Vignette(_) => &[("Amount", Percent), ("Size", Percent), ("Softness", Percent)],
        EffectKind::Grain(_) => &[("Amount", Percent), ("Size", Percent)],
        EffectKind::Sharpen(_) => &[("Amount", Percent), ("Size", Percent)],
        EffectKind::Glow(_) => &[("Amount", Percent), ("Threshold", Percent), ("Spread", Percent)],
        EffectKind::Blur(_) => &[("Amount", Percent), BOX[0], BOX[1], BOX[2], BOX[3]],
        EffectKind::Pixelate(_) => &[("Block size", Percent), BOX[0], BOX[1], BOX[2], BOX[3]],
        EffectKind::Zoom(_) => &[
            ("Zoom at end", Size(300.0)),
            ("Point across", Percent),
            ("Point down", Percent),
        ],
        EffectKind::Shake(_) => &[("Amount", Percent), ("Speed", Percent)],
        EffectKind::Pan(_) => &[
            ("Zoom", Size(200.0)),
            ("From across", Percent),
            ("From down", Percent),
            ("To across", Percent),
            ("To down", Percent),
        ],
    }
}

impl Show {
    fn shown(self, millionths: i32) -> f64 {
        let share = f64::from(millionths) / f64::from(ONE);
        match self {
            Show::Percent => share * 100.0,
            Show::Size(most) => 100.0 + (most - 100.0) * share,
        }
    }

    fn millionths(self, shown: f64) -> i32 {
        let share = match self {
            Show::Percent => shown / 100.0,
            Show::Size(most) => (shown - 100.0) / (most - 100.0),
        };
        (share * f64::from(ONE)).round().clamp(0.0, f64::from(ONE)) as i32
    }

    fn number(self, usual: i32) -> Number {
        let (range, per_point) = match self {
            Show::Percent => ((0.0, 100.0), 0.5),
            Show::Size(most) => ((100.0, most), (most - 100.0) / 200.0),
        };
        Number {
            range,
            per_point,
            decimals: 0,
            suffix: " %",
            usual: self.shown(usual),
        }
    }
}

/// The eases a keyframe can have, as the control offers them.
const EASES: [&str; 3] = [Ease::ALL[0].label(), Ease::ALL[1].label(), Ease::ALL[2].label()];

/// The section for `at.clip`'s effects as `shown` (with a drag in progress).
pub fn section(ui: &mut egui::Ui, theme: &Theme, sequence: SequenceId, at: At<'_>, shown: &Effects) -> Asked {
    let clip = at.clip.id;
    let mut asked = Asked::default();
    let mut changed = shown.clone();
    let mut touched = Touched::default();
    let mut clicked: Option<Effects> = None;
    let mut add: Option<EffectKind> = None;
    let mut seek = None;
    let clicks = inspector::section(ui, theme, "Effects", &summary(shown), !shown.is_empty(), |ui| {
        let count = shown.0.len();
        for i in 0..count {
            let effect = &shown.0[i];
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
                let row = settings(ui, theme, at, effect);
                match row {
                    Row::Clicked(next) => {
                        let mut all = shown.clone();
                        all.0[i] = next;
                        clicked = Some(all);
                    }
                    Row::Moved(next, t) => {
                        changed.0[i] = next;
                        touched = touched.and(t);
                    }
                    Row::Seek(to) => seek = Some(to),
                    Row::Still => {}
                }
            });
            ui.add_space(4.0);
        }
        if count == 0 {
            inspector::note(
                ui,
                theme,
                "None yet. Effects change this clip's picture, in the order listed. Drag one from the media drawer's Effects, or:",
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
    asked.seek = seek;
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

/// What happened in one effect's settings.
enum Row {
    Still,
    /// A setting is being dragged (or typed): the effect as it leaves it.
    Moved(project::Effect, Touched),
    /// A keyframe was added, removed or eased, or the whole picture ticked:
    /// one step at once.
    Clicked(project::Effect),
    /// An arrow beside a keyframe: the playhead goes there.
    Seek(Time),
}

/// The settings of one effect at the playhead, each with its keyframe
/// control. A change to a setting that has keyframes is a keyframe at the
/// playhead; one without changes it for the whole clip.
fn settings(ui: &mut egui::Ui, theme: &Theme, at: At<'_>, effect: &project::Effect) -> Row {
    let clip = at.clip;
    let source = clip.source_at(at.playhead);
    let near = clip.frame_in_file(at.playhead, at.rate);
    let here = at.rate.frame_at(at.playhead);
    let on_clip = clip.start <= at.playhead && at.playhead < clip.start + clip.duration;
    let now = effect.kind_at(source).numbers();
    let usual = EffectKind::ALL
        .iter()
        .find(|k| k.name() == effect.kind.name())
        .map_or_else(|| effect.kind.numbers(), EffectKind::numbers);
    let mut next = effect.clone();
    let mut touched = Touched::default();
    let mut result = Row::Still;
    // A box: the whole picture, or its four settings.
    let spot = effect.kind.spot_numbers();
    let whole = spot.clone().is_some_and(|range| {
        let whole = Spot::WHOLE;
        now[range] == [whole.left, whole.top, whole.width, whole.height]
    });
    for (index, (label, show)) in knobs(&effect.kind).iter().enumerate() {
        if let Some(range) = spot.clone()
            && index == range.start
        {
            let mut ticked = whole;
            let box_tip = "Untick to apply it to a box only: to hide a name or the chat";
            if ui
                .checkbox(&mut ticked, "Whole picture")
                .on_hover_text(box_tip)
                .changed()
            {
                let corner = if ticked { Spot::WHOLE } else { Spot::MIDDLE };
                let mut numbers = next.kind.numbers();
                numbers[range.clone()].copy_from_slice(&[
                    corner.left,
                    corner.top,
                    corner.width,
                    corner.height,
                ]);
                next.kind = next.kind.with_numbers(&numbers);
                next.keys.clear(range.clone());
                return Row::Clicked(next);
            }
        }
        if whole && spot.as_ref().is_some_and(|r| r.contains(&index)) {
            continue;
        }
        let keys = next.keys.of(index);
        let frames: Vec<i64> = keys
            .iter()
            .map(|k| at.rate.frame_at(clip.timeline_at(k.at)))
            .collect();
        let key_here = keys.iter().find(|k| near.0 <= k.at && k.at < near.1);
        let state = match (keys.is_empty(), key_here.is_some()) {
            (true, _) => KeyState::Still,
            (false, true) => KeyState::At,
            (false, false) => KeyState::Between,
        };
        let before = frames.iter().rev().find(|f| **f < here).copied();
        let after = frames.iter().find(|f| **f > here).copied();
        let control = Keys {
            state,
            before: before.is_some(),
            after: after.is_some(),
            can: on_clip,
            eases: &EASES,
            ease: key_here.and_then(|k| Ease::ALL.iter().position(|e| *e == k.ease)),
        };
        let mut value = show.shown(now[index]);
        let keyed =
            inspector::keyed_slider(ui, theme, label, &mut value, show.number(usual[index]), &control);
        if keyed.touched.changed {
            let value = show.millionths(value);
            if keys.is_empty() {
                let mut numbers = next.kind.numbers();
                numbers[index] = value;
                next.kind = next.kind.with_numbers(&numbers);
            } else if on_clip {
                next.keys.set(index, source, near, value);
            }
        }
        touched = touched.and(keyed.touched);
        let clicks = keyed.clicks;
        if clicks.toggle && on_clip {
            let mut keyed = effect.clone();
            match state {
                KeyState::At => keyed.keys.remove(index, near),
                _ => keyed.keys.set(index, source, near, now[index]),
            }
            result = Row::Clicked(keyed);
        } else if let Some(ease) = clicks.ease.and_then(|i| Ease::ALL.get(i)) {
            let mut eased = effect.clone();
            eased.keys.set_ease(index, near, *ease);
            result = Row::Clicked(eased);
        } else if let Some(frame) = match (clicks.previous, clicks.next) {
            (true, _) => before,
            (_, true) => after,
            _ => None,
        } {
            result = Row::Seek(at.rate.frame_start(frame));
        }
    }
    match result {
        Row::Still if touched.changed || touched.busy => Row::Moved(next, touched),
        other => other,
    }
}

/// The section's line while folded: the effects that are on, in order.
pub fn summary(effects: &Effects) -> String {
    let on: Vec<&str> = effects.applied().map(|e| e.kind.name()).collect();
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
    fn every_setting_has_a_row_and_comes_back_as_it_went() {
        for kind in EffectKind::ALL {
            let numbers = kind.numbers();
            assert_eq!(knobs(&kind).len(), numbers.len(), "{}", kind.name());
            for ((_, show), value) in knobs(&kind).iter().zip(numbers) {
                let back = show.millionths(show.shown(value));
                assert!(
                    (back - value).abs() <= 1,
                    "{}: {value} came back as {back}",
                    kind.name()
                );
            }
        }
        assert_eq!(Show::Size(300.0).shown(ONE), 300.0);
        assert_eq!(Show::Percent.millionths(150.0), ONE, "kept in range");
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
