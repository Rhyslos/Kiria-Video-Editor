//! The column of track names: its right-click menu (add, rename, hide and
//! remove tracks, show an audio track's level lines), the eye that hides a video track, and typing a track's
//! name in place.

use super::{Half, TimelineData, TimelineEvent, TimelineTrack, tracks_of};
use crate::Theme;
use std::collections::BTreeSet;

use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// Where a header's name starts: right of the column of its target tab
/// and sync lock ([`patch_rects`]) and its role's mark.
pub(crate) const NAME_LEFT: f32 = 22.0;

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

/// A header shorter than this has no buttons (a row partly scrolled out
/// of view); compact rows are shorter, so less is enough there.
pub(super) fn least_for_buttons(compact: bool) -> f32 {
    if compact { 14.0 } else { 20.0 }
}

/// The place of the `i`-th small button of a header, from the left: two fit
/// side by side at its right edge (mute and solo, or the eye in the second
/// place). In a short header (compact tracks) they are shorter, in its middle.
pub(super) fn button_rect(header: Rect, i: f32) -> Rect {
    let x = header.right() - 50.0 + i * 24.0;
    if header.height() < 28.0 {
        let height = (header.height() - 4.0).clamp(10.0, 16.0);
        Rect::from_min_size(pos2(x, header.center().y - height / 2.0), vec2(20.0, height))
    } else {
        Rect::from_min_size(pos2(x, header.top() + 6.0), vec2(20.0, 16.0))
    }
}

/// What the menu of the track names keeps or reads beside the timeline's
/// data: the field a name is typed in, the tracks showing level lines and
/// those pinned beside the master, and whether effects were copied.
pub(super) struct MenuState<'m> {
    pub rename: &'m mut Option<Rename>,
    pub level_lines: &'m mut BTreeSet<u64>,
    /// None where tracks are not pinned (a page without that column).
    pub pins: Option<&'m mut super::pins::Pins>,
    pub effects_copied: bool,
}

/// The right-click menu of the track names, for the spot that was clicked.
/// Call it every frame for the column's response, wherever the pointer is:
/// a menu that is not shown for a frame closes.
pub(super) fn menu(
    r: &egui::Response,
    spot: Option<MenuSpot>,
    data: &TimelineData,
    state: MenuState,
    events: &mut Vec<TimelineEvent>,
) {
    let MenuState {
        rename,
        level_lines,
        pins,
        effects_copied,
    } = state;
    r.context_menu(|ui| {
        let Some((half, track)) = spot else { return };
        let tracks = tracks_of(half, data);
        if let Some((index, track)) = track.and_then(|id| tracks.iter().enumerate().find(|(_, t)| t.id == id))
        {
            let id = track.id;
            if ui.button("Rename…").clicked() {
                *rename = Some(Rename::of(track));
                ui.close();
            }
            let lock = if track.locked { "Unlock" } else { "Lock" };
            item(
                events,
                ui,
                lock,
                TimelineEvent::LockTrack {
                    track: id,
                    locked: !track.locked,
                },
            );
            ui.menu_button("Colour", |ui| {
                if ui.radio(track.colour.is_none(), "Usual").clicked() {
                    events.push(TimelineEvent::ColourTrack {
                        track: id,
                        colour: None,
                    });
                    ui.close();
                }
                for (i, (name, colour)) in crate::theme::labels::ALL.iter().enumerate() {
                    let text = egui::RichText::new(format!("● {name}")).color(*colour);
                    if ui.radio(track.colour == Some(*colour), text).clicked() {
                        events.push(TimelineEvent::ColourTrack {
                            track: id,
                            colour: Some(i),
                        });
                        ui.close();
                    }
                }
            });
            if half == Half::Video {
                let text = if track.hidden {
                    "Show this track"
                } else {
                    "Hide this track"
                };
                item(
                    events,
                    ui,
                    text,
                    TimelineEvent::HideTrack {
                        track: id,
                        hidden: !track.hidden,
                    },
                );
            } else {
                let on = level_lines.contains(&id);
                let text = if on {
                    "Hide the level lines"
                } else {
                    "Show the level lines"
                };
                if ui.button(text).clicked() {
                    if on {
                        level_lines.remove(&id);
                    } else {
                        level_lines.insert(id);
                    }
                    ui.close();
                }
                if !data.roles.is_empty() {
                    ui.menu_button("Role", |ui| {
                        for (i, (name, colour)) in data.roles.iter().enumerate() {
                            let text = egui::RichText::new(format!("● {name}"));
                            let picked = track.role == Some(i);
                            if ui.radio(picked, text.color(*colour)).clicked() {
                                events.push(TimelineEvent::TrackRole { track: id, role: i });
                                ui.close();
                            }
                        }
                    });
                }
                if let Some(pins) = pins {
                    let is_pinned = pins.has(id);
                    let text = if is_pinned {
                        "Unpin from beside the master"
                    } else {
                        "Pin beside the master"
                    };
                    if ui
                        .button(text)
                        .on_hover_text(
                            "Its own tall fader and meter beside the master column, as many as you like",
                        )
                        .clicked()
                    {
                        pins.set(id, !is_pinned);
                        ui.close();
                    }
                }
            }
            ui.separator();
            item(
                events,
                ui,
                "Select every clip on it",
                TimelineEvent::SelectTrackClips {
                    track: id,
                    from: None,
                },
            );
            item(
                events,
                ui,
                "Select its clips after the playhead",
                TimelineEvent::SelectTrackClips {
                    track: id,
                    from: Some(data.playhead),
                },
            );
            if half == Half::Audio {
                ui.separator();
                item(
                    events,
                    ui,
                    "Record a voice-over here",
                    TimelineEvent::RecordOnTrack { track: id },
                );
                item(
                    events,
                    ui,
                    "Solo only this track",
                    TimelineEvent::SoloOnly { track: id },
                );
                item(
                    events,
                    ui,
                    "Reset volume and pan",
                    TimelineEvent::ResetTrackMix { track: id },
                );
                item(
                    events,
                    ui,
                    "Copy its effects",
                    TimelineEvent::CopyTrackEffects { track: id },
                );
                let paste = egui::Button::new("Paste effects");
                let r = ui.add_enabled(effects_copied, paste);
                if r.clicked() {
                    events.push(TimelineEvent::PasteTrackEffects { track: id });
                    ui.close();
                }
                r.on_disabled_hover_text("Copy the effects of a track first.");
            }
            ui.separator();
            item(
                events,
                ui,
                "Duplicate",
                TimelineEvent::DuplicateTrack { track: id },
            );
            // Video tracks are drawn V1 at the bottom, audio tracks A1 at
            // the top: "up" is a higher number for video, a lower one for audio.
            let audio = half == Half::Audio;
            let (up, down) = match audio {
                false => (index + 1, index.checked_sub(1)),
                true => (index.wrapping_sub(1), Some(index + 1)),
            };
            let up = (up < tracks.len()).then_some(up);
            let down = down.filter(|d| *d < tracks.len());
            for (text, to) in [("Move up", up), ("Move down", down)] {
                let r = ui.add_enabled(to.is_some(), egui::Button::new(text));
                if r.clicked()
                    && let Some(index) = to
                {
                    events.push(TimelineEvent::MoveTrack { track: id, index });
                    ui.close();
                }
            }
            let (above, below) = match audio {
                false => (index + 1, index),
                true => (index, index + 1),
            };
            item(
                events,
                ui,
                "Insert a track above",
                TimelineEvent::InsertTrack { audio, index: above },
            );
            item(
                events,
                ui,
                "Insert a track below",
                TimelineEvent::InsertTrack { audio, index: below },
            );
            let remove = egui::Button::new(format!("Remove {}", track.name));
            match why_it_stays(track, tracks.len()) {
                Some(why) => {
                    ui.add_enabled(false, remove).on_disabled_hover_text(why);
                }
                None => {
                    if ui.add(remove).clicked() {
                        events.push(TimelineEvent::RemoveTrack { track: id });
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
        let empty = data.video.iter().chain(data.audio).any(|t| t.clips.is_empty());
        let r = ui.add_enabled(empty, egui::Button::new("Remove the empty tracks"));
        if r.clicked() {
            events.push(TimelineEvent::RemoveEmptyTracks);
            ui.close();
        }
        r.on_disabled_hover_text("Every track has clips.");
    });
}

/// A line of the menu that sends `event` when clicked.
fn item(events: &mut Vec<TimelineEvent>, ui: &mut Ui, text: &str, event: TimelineEvent) {
    if ui.button(text).clicked() {
        events.push(event);
        ui.close();
    }
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
    compact: bool,
    events: &mut Vec<TimelineEvent>,
) {
    if header.height() < least_for_buttons(compact) {
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

/// The column at a header's left edge: the target tab on top and the sync
/// lock under it. A short header (compact tracks) has only the tab, as
/// tall as the header; one too short for buttons has neither.
pub(super) fn patch_rects(header: Rect, compact: bool) -> (Option<Rect>, Option<Rect>) {
    let x = header.left() + 3.0;
    if header.height() < least_for_buttons(compact) {
        return (None, None);
    }
    if compact || header.height() < 44.0 {
        let tab = Rect::from_min_max(pos2(x, header.top() + 3.0), pos2(x + 12.0, header.bottom() - 3.0));
        return (Some(tab), None);
    }
    let tab = Rect::from_min_size(pos2(x, header.top() + 6.0), vec2(12.0, 16.0));
    let sync = Rect::from_min_size(pos2(x, header.top() + 26.0), vec2(12.0, 16.0));
    (Some(tab), Some(sync))
}

/// The target tab: lit on the track a file placed from the media drawer
/// or the Library lands on (its picture on the video target, its first
/// sound stream on the audio target). A click makes this track the target.
/// True when clicked.
pub(super) fn target_tab(ui: &mut Ui, theme: &Theme, track: &TimelineTrack, rect: Rect, on: bool) -> bool {
    let r = ui
        .interact(rect, ui.scope_id().with(("track target", track.id)), Sense::click())
        .on_hover_text(if on {
            "The target: files placed from the media drawer or the Library land on this track (insert, overwrite, add at the end)"
        } else {
            "Make this track the target: files placed from the media drawer or the Library land on it"
        });
    let p = ui.painter();
    let fill = match on {
        true => theme.accent,
        false if r.hovered() => theme.line_strong,
        false => theme.line,
    };
    p.rect_filled(rect, 2.0, fill);
    // A small arrow pointing into the track.
    let c = rect.center();
    let colour = if on { theme.bg } else { theme.muted };
    p.add(Shape::convex_polygon(
        vec![c + vec2(-2.5, -3.5), c + vec2(3.0, 0.0), c + vec2(-2.5, 3.5)],
        colour,
        Stroke::NONE,
    ));
    r.clicked()
}

/// The sync lock: on (a quiet chain), a ripple edit elsewhere moves this
/// track's clips too, to stay in step; off (a broken chain, in the
/// warning colour), it stays where it is unless the edit is on it.
pub(super) fn sync_button(
    ui: &mut Ui,
    theme: &Theme,
    track: &TimelineTrack,
    rect: Rect,
    events: &mut Vec<TimelineEvent>,
) {
    let r = ui
        .interact(rect, ui.scope_id().with(("track sync", track.id)), Sense::click())
        .on_hover_text(if track.unsynced {
            "Sync lock off: closing a gap, a ripple trim or an insert on other tracks leaves this track where it is. Click to lock it in sync again"
        } else {
            "Sync lock on: closing a gap, a ripple trim or an insert on other tracks moves this track's clips too, so it stays in step. Click to turn it off"
        });
    let p = ui.painter();
    let (fill, colour) = match track.unsynced {
        true => (crate::status::WARNING, theme.bg),
        false if r.hovered() => (theme.line_strong, theme.text),
        false => (theme.line, theme.muted),
    };
    p.rect_filled(rect, 2.0, fill);
    draw_chain(p, rect.center(), colour, track.unsynced);
    if r.clicked() {
        events.push(TimelineEvent::SyncTrack {
            track: track.id,
            synced: track.unsynced,
        });
    }
}

/// Two links of a chain, one above the other, around `c`; pulled apart
/// when `broken`.
fn draw_chain(p: &egui::Painter, c: Pos2, colour: egui::Color32, broken: bool) {
    let apart = if broken { 1.2 } else { 0.0 };
    let stroke = Stroke::new(1.2, colour);
    for dy in [-2.4 - apart, 2.4 + apart] {
        let link = Rect::from_center_size(c + vec2(0.0, dy), vec2(4.5, 6.0));
        p.rect_stroke(link, 2.2, stroke, egui::StrokeKind::Middle);
    }
    if broken {
        // A small break across the middle.
        p.line_segment([c + vec2(-4.0, -1.0), c + vec2(4.0, 1.0)], stroke);
    }
}

/// The padlock in a track's header, at `rect`: shut and lit while the
/// track is locked. Click to lock or unlock it.
pub(super) fn lock_button(
    ui: &mut Ui,
    theme: &Theme,
    track: &TimelineTrack,
    rect: Rect,
    events: &mut Vec<TimelineEvent>,
) {
    let r = ui
        .interact(rect, ui.scope_id().with(("track lock", track.id)), Sense::click())
        .on_hover_text(if track.locked {
            "Locked: nothing on this track can be moved, trimmed, cut or changed. Click to unlock it"
        } else {
            "Lock this track: nothing on it can be moved, trimmed, cut or changed"
        });
    let fill = match track.locked {
        true => crate::status::WARNING,
        false if r.hovered() => theme.line_strong,
        false => theme.line,
    };
    let p = ui.painter();
    p.rect_filled(rect, 3.0, fill);
    draw_padlock(
        p,
        rect.center(),
        if track.locked { theme.bg } else { theme.text },
        track.locked,
    );
    if r.clicked() {
        events.push(TimelineEvent::LockTrack {
            track: track.id,
            locked: !track.locked,
        });
    }
}

/// A padlock about 10 points tall around `c`; open (its shackle lifted to
/// one side) unless `shut`.
pub(super) fn draw_padlock(p: &egui::Painter, c: Pos2, colour: egui::Color32, shut: bool) {
    let body = Rect::from_center_size(c + vec2(0.0, 2.0), vec2(9.0, 6.5));
    p.rect_filled(body, 1.2, colour);
    let lift = if shut { 0.0 } else { 2.0 };
    let shackle: Vec<Pos2> = (0..=10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 10.0;
            c + vec2(-3.0 * a.cos(), -1.5 - lift - 3.5 * a.sin())
        })
        .collect();
    let left_foot = [c + vec2(-3.0, -1.5 - lift), c + vec2(-3.0, body.top() - c.y)];
    let right_foot = [c + vec2(3.0, -1.5 - lift), c + vec2(3.0, body.top() - c.y - lift)];
    let stroke = Stroke::new(1.4, colour);
    p.add(Shape::line(shackle, stroke));
    if shut {
        p.line_segment(left_foot, stroke);
    }
    p.line_segment(right_foot, stroke);
}

/// Thin diagonal lines over a locked track's row, so it is seen at a glance.
pub(super) fn draw_locked(p: &egui::Painter, theme: &Theme, band: Rect) {
    let stroke = Stroke::new(1.0, theme.text.gamma_multiply(0.07));
    let step = 12.0;
    let mut x = band.left() - band.height();
    while x < band.right() {
        p.line_segment(
            [pos2(x, band.bottom()), pos2(x + band.height(), band.top())],
            stroke,
        );
        x += step;
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
        pos2(header.left() + NAME_LEFT - 6.0, centre_y - 11.0),
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
