//! The media drawer's Effects side: every picture effect in groups, a find
//! box, and the ones used last on top. A row is dragged onto a clip on the
//! timeline (`ui_kit::DraggedEffect`), double-clicked (or its + clicked) to
//! go onto the selected clip, and previewed on the selected clip while the
//! pointer rests on it, with no undo step until it is added. Transitions
//! are in a group of their own: dragged onto a cut
//! (`ui_kit::DraggedTransition`), or double-clicked to go into the
//! selected clip from the one before it.

mod thumbs;

use std::collections::HashSet;

use project::{EffectKind, TitleStyle, TransitionKind};
use ui_kit::{DraggedEffect, DraggedTitle, DraggedTransition, Theme};

/// What a title layout is for, in a line.
fn title_line(style: TitleStyle) -> &'static str {
    match style {
        TitleStyle::LowerThird => "A name low on the left, on a dark band",
        TitleStyle::Centred => "Big words in the middle of the picture",
        TitleStyle::EndCard => "Words on a dark card filling the picture",
    }
}

/// What a transition does, in a line.
fn transition_line(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::Dissolve => "The next shot fades in over the last",
        TransitionKind::DipToBlack => "Out to black, then in from it",
        TransitionKind::Wipe => "The next shot is uncovered from the left",
        TransitionKind::Push => "The next shot slides in and pushes the last out",
        TransitionKind::Zoom => "The last grows and fades as the next settles in",
    }
}

/// One row of the drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Effect(EffectKind),
    Transition(TransitionKind),
    Title(TitleStyle),
}

impl Item {
    fn name(self) -> &'static str {
        match self {
            Item::Effect(kind) => kind.name(),
            Item::Transition(kind) => kind.name(),
            Item::Title(style) => style.name(),
        }
    }

    fn line(self) -> &'static str {
        match self {
            Item::Effect(kind) => about(&kind).1,
            Item::Transition(kind) => transition_line(kind),
            Item::Title(style) => title_line(style),
        }
    }
}

/// How many effects "Recently used" keeps.
pub const RECENT: usize = 5;

/// The groups, in the order shown.
const GROUPS: [&str; 5] = ["Motion", "Frame", "Keying", "Blur and censor", "Stylise"];

/// Which group an effect is in, and a line on what it does.
fn about(kind: &EffectKind) -> (&'static str, &'static str) {
    match kind {
        EffectKind::Zoom(_) => ("Motion", "Moves in on a point over the clip"),
        EffectKind::Shake(_) => ("Motion", "A smooth camera shake"),
        EffectKind::Pan(_) => ("Motion", "Slides across the picture over the clip"),
        EffectKind::Stabilise(_) => ("Motion", "Steadies a shaky camera, after analysing the clip"),
        EffectKind::Blur(_) => ("Blur and censor", "Softens the picture, or a box in it"),
        EffectKind::Pixelate(_) => ("Blur and censor", "Big blocks over the picture, or a box in it"),
        EffectKind::Vignette(_) => ("Stylise", "Darker towards the edges"),
        EffectKind::Grain(_) => ("Stylise", "Film grain that moves every frame"),
        EffectKind::Sharpen(_) => ("Stylise", "Crisper detail"),
        EffectKind::Glow(_) => ("Stylise", "Bright parts spill a soft light"),
        EffectKind::Mirror(_) => ("Frame", "Flips the picture, left for right or upside down"),
        EffectKind::Corners(_) => ("Frame", "Rounds the corners off, or makes a circle for a facecam"),
        EffectKind::Outline(_) => ("Frame", "A coloured line round its edge, as a facecam border"),
        EffectKind::Shadow(_) => ("Frame", "A soft shadow behind it, lifting it off what is below"),
        EffectKind::Letterbox(_) => ("Frame", "Black bars for a wider shape, as in the cinema"),
        EffectKind::Key(_) => ("Keying", "Takes a green or blue screen away, and its spill"),
    }
}

/// What was asked for on the Effects side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Put this effect on the selected picture.
    Add(EffectKind),
    /// Put this transition into the selected picture.
    AddTransition(TransitionKind),
    /// Add a title of this layout at the playhead.
    AddTitle(TitleStyle),
    /// Show them as tiles (true) or rows: a setting.
    Tiles(bool),
}

/// The Effects side's own state.
#[derive(Debug, Default)]
pub struct EffectsDrawer {
    /// Words to find effects by.
    pub find: String,
    /// The effect the pointer rests on, previewed on the selected picture.
    pub previewing: Option<EffectKind>,
    /// Shown as a grid of tiles rather than rows (a setting, set by the page).
    pub tiles: bool,
    /// The groups folded away, by title, for the session.
    folded: HashSet<String>,
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

/// A transition by its place in [`TransitionKind::ALL`], as a drag carries it.
pub fn transition_by_index(index: u32) -> Option<TransitionKind> {
    TransitionKind::ALL.get(index as usize).copied()
}

/// A title layout by its place in [`TitleStyle::ALL`], as a drag carries it.
pub fn title_by_index(index: u32) -> Option<TitleStyle> {
    TitleStyle::ALL.get(index as usize).copied()
}

/// Whether a title layout matches the words of `find`.
fn title_matches(style: TitleStyle, find: &str) -> bool {
    let text = format!("{} titles text words {}", style.name(), title_line(style)).to_lowercase();
    find.split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// Whether a transition matches the words of `find` (its name, its
/// group's or its line).
fn transition_matches(kind: TransitionKind, find: &str) -> bool {
    let text = format!("{} transitions {}", kind.name(), transition_line(kind)).to_lowercase();
    find.split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
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
        let mut asked = None;
        ui.horizontal(|ui| {
            for (tiles, label, tip) in [
                (false, "☰", "Show them as rows"),
                (true, "▦", "Show them as tiles"),
            ] {
                if ui
                    .selectable_label(self.tiles == tiles, label)
                    .on_hover_text(tip)
                    .clicked()
                    && self.tiles != tiles
                {
                    asked = Some(Asked::Tiles(tiles));
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.find)
                    .hint_text("Find an effect")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add_space(4.0);
        let hint = if selected {
            "Drag one onto a clip (a transition onto a cut), or double-click to add it to the selected clip. Rest on an effect to preview it."
        } else {
            "Drag one onto a clip on the timeline (a transition onto a cut between two), or select a clip to add and preview them here."
        };
        ui.label(egui::RichText::new(hint).small().color(theme.muted));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let finding = !self.find.trim().is_empty();
                let recent: Vec<Item> = recent
                    .iter()
                    .filter_map(|n| by_name(n))
                    .map(Item::Effect)
                    .collect();
                if !finding && !recent.is_empty() {
                    self.group(ui, theme, "Recently used", &recent, selected, &mut asked);
                }
                let mut any = false;
                let transitions: Vec<Item> = TransitionKind::ALL
                    .into_iter()
                    .filter(|k| transition_matches(*k, &self.find))
                    .map(Item::Transition)
                    .collect();
                if !transitions.is_empty() {
                    any = true;
                    self.group(ui, theme, "Transitions", &transitions, selected, &mut asked);
                }
                let titles: Vec<Item> = TitleStyle::ALL
                    .into_iter()
                    .filter(|s| title_matches(*s, &self.find))
                    .map(Item::Title)
                    .collect();
                if !titles.is_empty() {
                    any = true;
                    self.group(ui, theme, "Titles", &titles, true, &mut asked);
                }
                for group in GROUPS {
                    let kinds: Vec<Item> = EffectKind::ALL
                        .into_iter()
                        .filter(|k| about(k).0 == group && matches(k, &self.find))
                        .map(Item::Effect)
                        .collect();
                    if !kinds.is_empty() {
                        any = true;
                        self.group(ui, theme, group, &kinds, selected, &mut asked);
                    }
                }
                if !any {
                    ui.label(egui::RichText::new("Nothing here has those words.").color(theme.muted));
                }
            });
        asked
    }

    fn group(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        title: &str,
        items: &[Item],
        selected: bool,
        asked: &mut Option<Asked>,
    ) {
        let folded = self.folded.contains(title);
        let arrow = if folded { "▸" } else { "▾" };
        let heading = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("{arrow} {title}"))
                        .small()
                        .strong()
                        .color(theme.muted),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text(if folded {
                "Show this group"
            } else {
                "Fold this group away"
            });
        if heading.clicked() {
            if folded {
                self.folded.remove(title);
            } else {
                self.folded.insert(title.to_owned());
            }
        }
        if folded {
            ui.add_space(4.0);
            return;
        }
        if self.tiles {
            let width = ((ui.available_width() - TILE_GAP) / 2.0).floor().max(60.0);
            for pair in items.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    for item in pair {
                        if let Some(a) = self.row(ui, theme, *item, selected, egui::vec2(width, TILE_HEIGHT))
                        {
                            *asked = Some(a);
                        }
                    }
                });
            }
        } else {
            for item in items {
                let size = egui::vec2(ui.available_width(), ROW_HEIGHT);
                if let Some(a) = self.row(ui, theme, *item, selected, size) {
                    *asked = Some(a);
                }
            }
        }
        ui.add_space(8.0);
    }

    /// One effect, transition or title: its picture, name and (as a row)
    /// its line; dragged, double-clicked or + to add.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        item: Item,
        selected: bool,
        size: egui::Vec2,
    ) -> Option<Asked> {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        match item {
            Item::Effect(kind) => {
                let index = EffectKind::ALL.iter().position(|k| *k == kind).unwrap_or(0) as u32;
                response.dnd_set_drag_payload(DraggedEffect(index));
            }
            Item::Transition(kind) => {
                let index = TransitionKind::ALL.iter().position(|k| *k == kind).unwrap_or(0) as u32;
                response.dnd_set_drag_payload(DraggedTransition(index));
            }
            Item::Title(style) => {
                let index = TitleStyle::ALL.iter().position(|s| *s == style).unwrap_or(0) as u32;
                response.dnd_set_drag_payload(DraggedTitle(index));
            }
        }
        let lit = response.hovered() || response.dragged();
        if lit {
            ui.painter().rect_filled(rect, 6.0, theme.line);
        }
        if let Item::Effect(kind) = item
            && response.hovered()
            && selected
            && !response.dragged()
        {
            self.previewing = Some(kind);
        }
        let painter = ui.painter_at(rect);
        let tile = size.y >= TILE_HEIGHT;
        if tile {
            let picture = egui::Rect::from_min_size(
                rect.min + egui::vec2(4.0, 4.0),
                egui::vec2(rect.width() - 8.0, TILE_HEIGHT - 26.0),
            );
            thumbs::draw(&painter, picture, item);
            painter.text(
                egui::pos2(rect.left() + 6.0, rect.bottom() - 6.0),
                egui::Align2::LEFT_BOTTOM,
                item.name(),
                egui::FontId::proportional(12.5),
                theme.text,
            );
        } else {
            let picture = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 6.0, rect.center().y - THUMB.y / 2.0),
                THUMB,
            );
            thumbs::draw(&painter, picture, item);
            let text = rect.shrink2(egui::vec2(10.0, 6.0));
            let left = picture.right() + 10.0;
            painter.text(
                egui::pos2(left, text.top()),
                egui::Align2::LEFT_TOP,
                item.name(),
                egui::FontId::proportional(14.0),
                theme.text,
            );
            painter.text(
                egui::pos2(left, text.bottom()),
                egui::Align2::LEFT_BOTTOM,
                item.line(),
                egui::FontId::proportional(11.5),
                theme.muted,
            );
        }
        let response = response.on_hover_text(item.line());
        let mut asked = None;
        if selected {
            let plus = match tile {
                true => egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 14.0, rect.bottom() - 12.0),
                    egui::vec2(22.0, 22.0),
                ),
                false => egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 18.0, rect.center().y),
                    egui::vec2(24.0, 24.0),
                ),
            };
            let tip = match item {
                Item::Effect(_) => "Add to the selected clip",
                Item::Transition(_) => "Put it into the selected clip, from the clip before it",
                Item::Title(_) => "Add it at the playhead",
            };
            let add = ui
                .interact(plus, response.id.with("add"), egui::Sense::click())
                .on_hover_text(tip);
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
                asked = Some(match item {
                    Item::Effect(kind) => Asked::Add(kind),
                    Item::Transition(kind) => Asked::AddTransition(kind),
                    Item::Title(style) => Asked::AddTitle(style),
                });
            }
        }
        response.on_hover_cursor(egui::CursorIcon::Grab);
        asked
    }
}

/// Height of one effect's row.
const ROW_HEIGHT: f32 = 48.0;

/// A row's picture.
const THUMB: egui::Vec2 = egui::vec2(60.0, 36.0);

/// A tile's height, and the gap between two tiles.
const TILE_HEIGHT: f32 = 92.0;
const TILE_GAP: f32 = 8.0;

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
        let facecam: Vec<&str> = EffectKind::ALL
            .iter()
            .filter(|k| matches(k, "facecam"))
            .map(|k| k.name())
            .collect();
        assert_eq!(facecam, ["Rounded corners", "Outline"]);
        assert!(matches(&EffectKind::ALL[14], "green screen"));
        assert!(matches(&EffectKind::ALL[10], "flip"));
        assert!(!matches(&EffectKind::ALL[0], "nothing like it"));
    }

    #[test]
    fn transitions_travel_by_place_and_are_found_by_their_words() {
        for (i, kind) in TransitionKind::ALL.iter().enumerate() {
            assert_eq!(transition_by_index(i as u32), Some(*kind));
            assert!(transition_matches(*kind, "transition"));
        }
        assert!(transition_matches(TransitionKind::DipToBlack, "black"));
        assert!(!transition_matches(TransitionKind::Wipe, "black"));
    }

    #[test]
    fn titles_travel_by_place_and_are_found_by_their_words() {
        for (i, style) in TitleStyle::ALL.iter().enumerate() {
            assert_eq!(title_by_index(i as u32), Some(*style));
            assert!(title_matches(*style, "title"));
            assert!(title_matches(*style, "text"));
        }
        assert!(title_matches(TitleStyle::LowerThird, "name"));
        assert!(!title_matches(TitleStyle::Centred, "band"));
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
