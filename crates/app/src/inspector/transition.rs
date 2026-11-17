//! The inspector's Transition in section, on a picture with a clip ending
//! where it starts on its track: which transition plays across that cut
//! (or none, a plain cut) and how long it lasts. Each change is one undo
//! step; a length being dragged becomes one when let go.

use project::{Clip, Command, SequenceId, Time, Track, Transition, TransitionKind};
use ui_kit::{Theme, inspector};

/// Lengths offered with one click, in seconds.
const QUICK: [f64; 4] = [0.25, 0.5, 1.0, 2.0];

/// Draws the section for `clip` on `track` when a transition can go into
/// it, and returns the change asked for.
pub fn section(
    ui: &mut egui::Ui,
    theme: &Theme,
    sequence: SequenceId,
    track: &Track,
    clip: &Clip,
) -> Option<Command> {
    let previous = project::transitions::before(track, clip).filter(|c| c.is_file() && clip.is_file())?;
    let longest = previous.duration.min(clip.duration).as_secs_f64();
    let now = clip.transition;
    let summary = match now {
        Some(t) => format!("{} · {}", t.kind.name(), seconds_text(t.length.as_secs_f64())),
        None => "A plain cut".to_owned(),
    };
    let mut asked: Option<Option<Transition>> = None;
    let clicks = inspector::section(ui, theme, "Transition in", &summary, now.is_some(), |ui| {
        ui.horizontal(|ui| {
            inspector::label_cell(ui, theme, "Kind", inspector::LABEL_WIDTH);
            let shown = now.map_or("None (a cut)", |t| t.kind.name());
            egui::ComboBox::from_id_salt(("transition kind", clip.id.0))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(now.is_none(), "None (a cut)").clicked() && now.is_some() {
                        asked = Some(None);
                    }
                    for kind in TransitionKind::ALL {
                        let picked = now.is_some_and(|t| t.kind == kind);
                        if ui.selectable_label(picked, kind.name()).clicked() && !picked {
                            let length = now.map_or_else(|| Transition::new(kind).length, |t| t.length);
                            asked = Some(Some(Transition { kind, length }));
                        }
                    }
                })
                .response
                .on_hover_text("How the picture before this cut gives way to this one");
        });
        let Some(transition) = now else {
            ui.label(
                egui::RichText::new("Plays across the cut from the clip before: half before it, half after.")
                    .color(theme.muted)
                    .size(12.0),
            );
            return;
        };
        ui.horizontal(|ui| {
            inspector::label_cell(ui, theme, "Length", inspector::LABEL_WIDTH);
            let id = ui.scope_id().with(("transition length", clip.id.0));
            let stored = transition.length.as_secs_f64();
            let mut value = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(stored);
            let response = ui
                .add(
                    egui::DragValue::new(&mut value)
                        .range(0.04..=longest.max(0.04))
                        .speed(0.01)
                        .max_decimals(2)
                        .suffix(" s"),
                )
                .on_hover_text("How long it takes, centred on the cut; no longer than either clip");
            if response.dragged() {
                ui.data_mut(|d| d.insert_temp(id, value));
            } else {
                ui.data_mut(|d| d.remove::<f64>(id));
                if (value - stored).abs() > 1e-9 {
                    asked = Some(Some(Transition {
                        length: length_of(value),
                        ..transition
                    }));
                }
            }
            for quick in QUICK.into_iter().filter(|q| *q <= longest) {
                if ui
                    .small_button(seconds_text(quick))
                    .on_hover_text("Use this length")
                    .clicked()
                {
                    asked = Some(Some(Transition {
                        length: length_of(quick),
                        ..transition
                    }));
                }
            }
        });
    });
    if clicks.reset {
        asked = Some(None);
    }
    let transition = asked?;
    (transition != now).then_some(Command::SetTransition {
        sequence,
        clip: clip.id,
        transition,
    })
}

/// A length in seconds as a time, to the millisecond.
fn length_of(seconds: f64) -> Time {
    Time::from_ratio((seconds * 1000.0).round() as i64, 1000).unwrap_or(Time::ZERO)
}

/// "0.5 s".
fn seconds_text(seconds: f64) -> String {
    let text = format!("{seconds:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text} s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_kept_to_the_millisecond_and_written_short() {
        assert_eq!(length_of(0.5), Time::from_ratio(1, 2).unwrap());
        assert_eq!(length_of(1.234), Time::from_ratio(1234, 1000).unwrap());
        assert_eq!(seconds_text(0.5), "0.5 s");
        assert_eq!(seconds_text(2.0), "2 s");
        assert_eq!(seconds_text(0.25), "0.25 s");
    }
}
