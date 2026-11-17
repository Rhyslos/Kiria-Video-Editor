//! The media drawer's Effects side: every picture effect in groups, a find
//! box, and the ones used last on top. A row is dragged onto a clip on the
//! timeline (`ui_kit::DraggedEffect`), double-clicked (or its + clicked) to
//! go onto the selected clip, and previewed on the selected clip while the
//! pointer rests on it, with no undo step until it is added.

use project::EffectKind;
use ui_kit::{DraggedEffect, Theme};

/// How many effects "Recently used" keeps.
pub const RECENT: usize = 5;

/// The groups, in the order shown.
const GROUPS: [&str; 3] = ["Motion", "Blur and censor", "Stylise"];

/// Which group an effect is in, and a line on what it does.
fn about(kind: &EffectKind) -> (&'static str, &'static str) {
    match kind {
        EffectKind::Zoom(_) => ("Motion", "Moves in on a point over the clip"),
        EffectKind::Shake(_) => ("Motion", "A smooth camera shake"),
        EffectKind::Pan(_) => ("Motion", "Slides across the picture over the clip"),
        EffectKind::Blur(_) => ("Blur and censor", "Softens the picture, or a box in it"),
        EffectKind::Pixelate(_) => ("Blur and censor", "Big blocks over the picture, or a box in it"),
        EffectKind::Vignette(_) => ("Stylise", "Darker towards the edges"),
        EffectKind::Grain(_) => ("Stylise", "Film grain that moves every frame"),
        EffectKind::Sharpen(_) => ("Stylise", "Crisper detail"),
        EffectKind::Glow(_) => ("Stylise", "Bright parts spill a soft light"),
    }
}

/// What was asked for on the Effects side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Put this effect on the selected picture.
    Add(EffectKind),
}

/// The Effects side's own state.
#[derive(Debug, Default)]
pub struct EffectsDrawer {
    /// Words to find effects by.
    pub find: String,
    /// The effect the pointer rests on, previewed on the selected picture.
    pub previewing: Option<EffectKind>,
}

/// An effect by its place in [`EffectKind::ALL`], as a drag carries it.
pub fn by_index(index: u32) -> Option<EffectKind> {
    EffectKind::ALL.get(index as usize).copied()
}

/// An effect by its name, as "Recently used" keeps it.
pub fn by_name(name: &str) -> Option<EffectKind> {
    EffectKind::ALL.into_iter().find(|kind| kind.name() == name)
}

/// `recent` with `kind` put first, at most [`RECENT`] long, each once.
pub fn used(recent: &[String], kind: &EffectKind) -> Vec<String> {
    let name = kind.name().to_owned();
    std::iter::once(name.clone())
        .chain(recent.iter().filter(|n| **n != name).cloned())
        .take(RECENT)
        .collect()
}

/// Whether `kind` matches the words of `find` (its name, group or line).
fn matches(kind: &EffectKind, find: &str) -> bool {
    let (group, line) = about(kind);
    let text = format!("{} {group} {line}", kind.name()).to_lowercase();
    find.split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

impl EffectsDrawer {
    /// Draws the Effects side. `selected` says whether a picture is
    /// selected (the + buttons and double-clicks need one); `recent` is
    /// the effects used last, by name.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        selected: bool,
        recent: &[String],
    ) -> Option<Asked> {
        self.previewing = None;
        ui.add(
            egui::TextEdit::singleline(&mut self.find)
                .hint_text("Find an effect")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let hint = if selected {
            "Drag one onto a clip, or double-click to add it to the selected clip. Rest on one to preview it."
        } else {
            "Drag one onto a clip on the timeline, or select a clip to add and preview them here."
        };
        ui.label(egui::RichText::new(hint).small().color(theme.muted));
        ui.add_space(6.0);
        let mut asked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let finding = !self.find.trim().is_empty();
                let recent: Vec<EffectKind> = recent.iter().filter_map(|n| by_name(n)).collect();
                if !finding && !recent.is_empty() {
                    self.group(ui, theme, "Recently used", &recent, selected, &mut asked);
                }
                let mut any = false;
                for group in GROUPS {
                    let kinds: Vec<EffectKind> = EffectKind::ALL
                        .into_iter()
                        .filter(|k| about(k).0 == group && matches(k, &self.find))
                        .collect();
                    if !kinds.is_empty() {
                        any = true;
                        self.group(ui, theme, group, &kinds, selected, &mut asked);
                    }
                }
                if !any {
                    ui.label(egui::RichText::new("No effect has those words.").color(theme.muted));
                }
            });
        asked
    }

    fn group(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        title: &str,
        kinds: &[EffectKind],
        selected: bool,
        asked: &mut Option<Asked>,
    ) {
        ui.label(egui::RichText::new(title).small().strong().color(theme.muted));
        for kind in kinds {
            if let Some(a) = self.row(ui, theme, kind, selected) {
                *asked = Some(a);
            }
        }
        ui.add_space(8.0);
    }

    /// One effect: its name and line; dragged, double-clicked or + to add.
    fn row(&mut self, ui: &mut egui::Ui, theme: &Theme, kind: &EffectKind, selected: bool) -> Option<Asked> {
        let index = EffectKind::ALL.iter().position(|k| k == kind).unwrap_or(0) as u32;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_HEIGHT),
            egui::Sense::click_and_drag(),
        );
        response.dnd_set_drag_payload(DraggedEffect(index));
        let lit = response.hovered() || response.dragged();
        if lit {
            ui.painter().rect_filled(rect, 6.0, theme.line);
        }
        if response.hovered() && selected && !response.dragged() {
            self.previewing = Some(*kind);
        }
        let painter = ui.painter_at(rect);
        let text = rect.shrink2(egui::vec2(10.0, 6.0));
        painter.text(
            text.left_top(),
            egui::Align2::LEFT_TOP,
            kind.name(),
            egui::FontId::proportional(14.0),
            theme.text,
        );
        painter.text(
            text.left_bottom(),
            egui::Align2::LEFT_BOTTOM,
            about(kind).1,
            egui::FontId::proportional(11.5),
            theme.muted,
        );
        let mut asked = None;
        if selected {
            let plus = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 18.0, rect.center().y),
                egui::vec2(24.0, 24.0),
            );
            let add = ui
                .interact(plus, response.id.with("add"), egui::Sense::click())
                .on_hover_text("Add to the selected clip");
            if lit || add.hovered() {
                let colour = if add.hovered() { theme.text } else { theme.muted };
                ui.painter().text(
                    plus.center(),
                    egui::Align2::CENTER_CENTER,
                    "+",
                    egui::FontId::proportional(18.0),
                    colour,
                );
            }
            if add.clicked() || response.double_clicked() {
                asked = Some(Asked::Add(*kind));
            }
        }
        response.on_hover_cursor(egui::CursorIcon::Grab);
        asked
    }
}

/// Height of one effect's row.
const ROW_HEIGHT: f32 = 44.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_is_in_a_group_and_found_by_its_words() {
        for kind in EffectKind::ALL {
            assert!(GROUPS.contains(&about(&kind).0), "{}", kind.name());
            assert!(matches(&kind, ""), "nothing to find shows everything");
            assert!(matches(&kind, &kind.name().to_uppercase()));
        }
        let censor: Vec<&str> = EffectKind::ALL
            .iter()
            .filter(|k| matches(k, "censor"))
            .map(|k| k.name())
            .collect();
        assert_eq!(censor, ["Blur", "Pixelate"]);
        assert!(!matches(&EffectKind::ALL[0], "nothing like it"));
    }

    #[test]
    fn effects_travel_by_place_and_are_kept_by_name() {
        for (i, kind) in EffectKind::ALL.iter().enumerate() {
            assert_eq!(by_index(i as u32), Some(*kind));
            assert_eq!(by_name(kind.name()), Some(*kind));
        }
        assert_eq!(by_index(99), None);
    }

    #[test]
    fn the_last_used_come_first_each_once() {
        let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let all = EffectKind::ALL;
        let recent = used(&[], &all[0]);
        assert_eq!(recent, names(&["Vignette"]));
        let recent = used(&recent, &all[3]);
        let recent = used(&recent, &all[0]);
        assert_eq!(recent, names(&["Vignette", "Glow"]), "moved up, not repeated");
        let many = all.iter().fold(Vec::new(), |r, k| used(&r, k));
        assert_eq!(many.len(), RECENT);
        assert_eq!(many[0], all[all.len() - 1].name());
    }
}
