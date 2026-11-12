//! The column of track names: its right-click menu (add, rename, hide and
//! remove tracks, show an audio track's level lines), the eye that hides a video track, and typing a track's
//! name in place.

use super::{Half, TimelineData, TimelineEvent, TimelineTrack, tracks_of};
use crate::Theme;
use std::collections::BTreeSet;

use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// The longest name that can be typed for a track, in characters.
pub(super) const LONGEST_NAME: usize = 40;

/// Where the menu of the track names was opened: the area, and the track
/// under the pointer (none on the empty part of the column).
pub(super) type MenuSpot = (Half, Option<u64>);

/// A track's name while it is being typed in its header.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rename {
    pub track: u64,
    text: String,
    /// The field has been given the keyboard (on its first frame).
    focused: bool,
}

impl Rename {
    pub fn of(track: &TimelineTrack) -> Rename {
        Rename {
            track: track.id,
            text: track.name.clone(),
            focused: false,
        }
    }
}

/// What typing a name came to this frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Typed {
    /// Still typing.
    Going,
    /// Enter, or a click elsewhere: the name to give.
    Done(String),
    /// Escape, or nothing typed: the name stays.
    Dropped,
}

/// The place of the `i`-th small button of a header, from the left: two fit
/// side by side at its right edge (mute and solo, or the eye in the second place).
pub(super) fn button_rect(header: Rect, i: f32) -> Rect {
    Rect::from_min_size(
        pos2(header.right() - 50.0 + i * 24.0, header.top() + 6.0),
        vec2(20.0, 16.0),
    )
}

/// The right-click menu of the track names, for the spot that was clicked.
/// Call it every frame for the column's response, wherever the pointer is:
/// a menu that is not shown for a frame closes.
pub(super) fn menu(
    r: &egui::Response,
    spot: Option<MenuSpot>,
    data: &TimelineData,
    rename: &mut Option<Rename>,
    level_lines: &mut BTreeSet<u64>,
    events: &mut Vec<TimelineEvent>,
) {
    r.context_menu(|ui| {
        let Some((half, track)) = spot else { return };
        let tracks = tracks_of(half, data);
        if let Some(track) = track.and_then(|id| tracks.iter().find(|t| t.id == id)) {
            if ui.button("Rename…").clicked() {
                *rename = Some(Rename::of(track));
                ui.close();
            }
            if half == Half::Video {
                let text = if track.hidden {
                    "Show this track"
                } else {
                    "Hide this track"
                };
                if ui.button(text).clicked() {
                    events.push(TimelineEvent::HideTrack {
                        track: track.id,
                        hidden: !track.hidden,
                    });
                    ui.close();
                }
            } else {
                let on = level_lines.contains(&track.id);
                let text = if on {
                    "Hide the level lines"
                } else {
                    "Show the level lines"
                };
                if ui.button(text).clicked() {
                    if on {
                        level_lines.remove(&track.id);
                    } else {
                        level_lines.insert(track.id);
                    }
                    ui.close();
                }
            }
            let remove = egui::Button::new(format!("Remove {}", track.name));
            match why_it_stays(track, tracks.len()) {
                Some(why) => {
                    ui.add_enabled(false, remove).on_disabled_hover_text(why);
                }
                None => {
                    if ui.add(remove).clicked() {
                        events.push(TimelineEvent::RemoveTrack { track: track.id });
                        ui.close();
                    }
                }
            }
            ui.separator();
        }
        // The kind of the area that was clicked first.
        let add = [
            (
                "Add a video track on top",
                TimelineEvent::AddTrack { audio: false },
            ),
            (
                "Add an audio track at the bottom",
                TimelineEvent::AddTrack { audio: true },
            ),
        ];
        let order = if half == Half::Video { [0, 1] } else { [1, 0] };
        for i in order {
            let (text, event) = &add[i];
            if ui.button(*text).clicked() {
                events.push(event.clone());
                ui.close();
            }
        }
    });
}

/// Why `track` can't be removed, when it can't: it has clips, or it is the
/// last of the `count` tracks of its kind.
pub(super) fn why_it_stays(track: &TimelineTrack, count: usize) -> Option<&'static str> {
    if !track.clips.is_empty() {
        Some("It still has clips: move them to another track or delete them first.")
    } else if count <= 1 {
        Some("A timeline keeps at least one track of each kind.")
    } else {
        None
    }
}

/// The eye in a video track's header: open while the track is seen in the
/// picture, struck through while it is hidden. Click to change.
pub(super) fn eye(
    ui: &mut Ui,
    theme: &Theme,
    track: &TimelineTrack,
    header: Rect,
    events: &mut Vec<TimelineEvent>,
) {
    if header.height() < 20.0 {
        return;
    }
    let rect = button_rect(header, 1.0);
    let r = ui
        .interact(rect, ui.scope_id().with(("track eye", track.id)), Sense::click())
        .on_hover_text(if track.hidden {
            "Hidden: not seen in the picture. Click to show this track"
        } else {
            "Hide this track: the picture is put together without it"
        });
    let fill = if r.hovered() {
        theme.line_strong
    } else {
        theme.line
    };
    let p = ui.painter();
    p.rect_filled(rect, 3.0, fill);
    draw_eye(
        p,
        rect.center(),
        if track.hidden { theme.muted } else { theme.text },
        track.hidden,
    );
    if r.clicked() {
        events.push(TimelineEvent::HideTrack {
            track: track.id,
            hidden: !track.hidden,
        });
    }
}

/// An eye about 14 points wide around `c`; `shut` strikes it through.
fn draw_eye(p: &egui::Painter, c: Pos2, colour: egui::Color32, shut: bool) {
    let stroke = Stroke::new(1.3, colour);
    let (half_width, lift) = (6.5, 4.0);
    // The two lids: arcs from corner to corner, bulging up and down.
    for side in [-1.0, 1.0] {
        let lid: Vec<Pos2> = (0..=8)
            .map(|i| {
                let t = i as f32 / 8.0;
                let x = -half_width + 2.0 * half_width * t;
                c + vec2(x, side * lift * (1.0 - (2.0 * t - 1.0).powi(2)))
            })
            .collect();
        p.add(Shape::line(lid, stroke));
    }
    p.circle_filled(c, 1.9, colour);
    if shut {
        p.line_segment(
            [c + vec2(-6.0, 5.0), c + vec2(6.0, -5.0)],
            Stroke::new(1.6, colour),
        );
    }
}

/// Where a track's name is typed: over the name, left of the buttons.
fn field_rect(header: Rect, centre_y: f32) -> Rect {
    Rect::from_min_max(
        pos2(header.left() + 6.0, centre_y - 11.0),
        pos2(header.right() - 54.0, centre_y + 11.0),
    )
}

/// The field a track's name is typed in, drawn over the name. Enter or a
/// click elsewhere gives the name; Escape leaves it as it was.
pub(super) fn name_field(ui: &mut Ui, rename: &mut Rename, header: Rect, centre_y: f32) -> Typed {
    let rect = field_rect(header, centre_y);
    let id = ui.scope_id().with(("track name", rename.track));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let field = child.add_sized(
        rect.size(),
        egui::TextEdit::singleline(&mut rename.text)
            .id(id)
            .font(FontId::proportional(12.5))
            .char_limit(LONGEST_NAME)
            .margin(egui::Margin::symmetric(5, 2)),
    );
    if !rename.focused {
        rename.focused = true;
        field.request_focus();
        // The whole name is selected, so typing replaces it.
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let all = egui::text::CCursorRange::select_all_str(&rename.text);
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
        return Typed::Going;
    }
    if !field.lost_focus() {
        return Typed::Going;
    }
    let name = rename.text.trim();
    if name.is_empty() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        Typed::Dropped
    } else {
        Typed::Done(name.to_owned())
    }
}

/// A track's name in its header, fainter when the track is muted or hidden.
pub(super) fn draw_name(p: &egui::Painter, theme: &Theme, track: &TimelineTrack, at: Pos2) {
    p.text(
        at,
        Align2::LEFT_CENTER,
        &track.name,
        FontId::proportional(12.5),
        if track.muted || track.hidden {
            theme.faint
        } else {
            theme.text
        },
    );
}
