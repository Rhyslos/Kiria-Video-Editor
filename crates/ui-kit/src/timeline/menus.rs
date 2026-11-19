//! The timeline's right-click menus: an area's (clips, gaps, a track,
//! the region), the region's in the ruler, and keyframe marks'.

use super::{Gap, KeyframeMark, MarksChange, MenuSpot, TimelineEvent};
use egui::Ui;

/// The right-click menu of a timeline area, for the time and clip that were
/// clicked (`menu_at`). Call it every frame for the area's response, wherever
/// the pointer is: a menu that is not shown for a frame closes.
pub(super) fn clip_menu(r: &egui::Response, menu: Menu, events: &mut Vec<TimelineEvent>) {
    let Menu {
        at: menu_at,
        marks,
        eases,
        has_region,
        video_track,
        on_nested,
        ends_empty,
        gap,
        selection,
        several,
    } = menu;
    r.context_menu(|ui| {
        if let Some((clip, marks)) = marks {
            marks_menu(ui, clip, marks, eases, events);
            return;
        }
        let Some((at, clip)) = menu_at else { return };
        if let Some(gap) = gap.filter(|_| clip.is_none())
            && ui
                .button("Close the gap")
                .on_hover_text("Move everything after it back by its length, on every track")
                .clicked()
        {
            events.push(TimelineEvent::CloseGap(gap));
            ui.close();
        }
        if ui.button("Add a label here").clicked() {
            events.push(TimelineEvent::AddLabel(at));
            ui.close();
        }
        if let Some((id, start, end)) = clip {
            if on_nested && ui.button("Open its timeline").clicked() {
                events.push(TimelineEvent::OpenNested { clip: id });
                ui.close();
            }
            if ends_empty
                && ui
                    .button("Fit to the nested timeline")
                    .on_hover_text("Its timeline is shorter now: shorten the clip to end where it does.")
                    .clicked()
            {
                events.push(TimelineEvent::FitNested { clip: id });
                ui.close();
            }
            if ui.button("Split this clip here").clicked() {
                events.push(TimelineEvent::SplitAt { at, clip: Some(id) });
                ui.close();
            }
            ui.menu_button("Speed", |ui| {
                for (text, num, den) in [
                    ("25% (slow motion)", 1, 4),
                    ("50% (slow motion)", 1, 2),
                    ("100% (normal)", 1, 1),
                    ("200%", 2, 1),
                    ("400%", 4, 1),
                ] {
                    if ui.button(text).clicked() {
                        events.push(TimelineEvent::SetSpeed { clip: id, num, den });
                        ui.close();
                    }
                }
            });
            if ui.button("Set the region to this clip").clicked() {
                events.push(TimelineEvent::SetRegion(Some((start, end))));
                ui.close();
            }
        }
        if ui.button("Split every track here").clicked() {
            events.push(TimelineEvent::SplitAt { at, clip: None });
            ui.close();
        }
        if video_track.is_some() && ui.button("Add an adjustment layer here").clicked() {
            events.push(TimelineEvent::AddAdjustment {
                at,
                track: video_track,
            });
            ui.close();
        }
        if selection && ui.button("Nest the selected clips").clicked() {
            events.push(TimelineEvent::NestSelected);
            ui.close();
        }
        if several && ui.button("Sync by sound…").clicked() {
            events.push(TimelineEvent::SyncBySound);
            ui.close();
        }
        if has_region {
            ui.separator();
            region_menu(ui, events);
            if ui.button("Clear the region").clicked() {
                events.push(TimelineEvent::SetRegion(None));
                ui.close();
            }
        }
    });
}

/// What can be done with the region's stretch of every track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionCut {
    /// Take it out and close the gap (everything after moves back).
    Remove,
    /// Take it out and leave the gap.
    Lift,
    /// Keep only it: everything before and after goes.
    Keep,
}

/// The region's cuts as menu lines, each with the command it is in the
/// shortcut table (so search finds them too).
pub(super) fn region_menu(ui: &mut Ui, events: &mut Vec<TimelineEvent>) {
    for (cut, text, tip) in [
        (
            RegionCut::Remove,
            "Remove the region",
            "Cut it out of every track and close the gap",
        ),
        (
            RegionCut::Lift,
            "Remove the region, leave a gap",
            "Empty it on every track; nothing moves",
        ),
        (
            RegionCut::Keep,
            "Keep only the region",
            "Remove everything before and after it",
        ),
    ] {
        if ui.button(text).on_hover_text(tip).clicked() {
            events.push(TimelineEvent::RegionCut(cut));
            ui.close();
        }
    }
    if ui
        .button("Label the region")
        .on_hover_text("A label over the whole region, named and kept with the timeline")
        .clicked()
    {
        events.push(TimelineEvent::LabelRegion);
        ui.close();
    }
}

/// What the right-click menu of a timeline area is about.
#[derive(Clone, Copy)]
pub(super) struct Menu<'a> {
    pub(super) at: Option<MenuSpot>,
    /// Keyframe marks it was opened on (with their clip), and the marks
    /// shown there now: the menu is theirs.
    pub(super) marks: Option<(u64, &'a [&'a KeyframeMark])>,
    pub(super) eases: &'a [&'a str],
    pub(super) has_region: bool,
    /// The video track the menu was opened on (none over the audio area).
    pub(super) video_track: Option<u64>,
    /// The clip it was opened on plays another timeline.
    pub(super) on_nested: bool,
    /// That nested clip is longer than what its timeline has left to play.
    pub(super) ends_empty: bool,
    /// The gap it was opened on.
    pub(super) gap: Option<Gap>,
    /// Some clip is selected.
    pub(super) selection: bool,
    /// Two or more clips are selected.
    pub(super) several: bool,
}

/// The right-click menu of keyframe marks: go there, remove their
/// keyframes, or set how they go on to the next.
fn marks_menu(
    ui: &mut Ui,
    clip: u64,
    marks: &[&KeyframeMark],
    eases: &[&str],
    events: &mut Vec<TimelineEvent>,
) {
    let times: Vec<f64> = marks.iter().map(|m| m.at).collect();
    let mut keyed: Vec<&str> = Vec::new();
    for word in marks.iter().flat_map(|m| m.keyed.iter()) {
        if !keyed.contains(word) {
            keyed.push(word);
        }
    }
    let title = match (marks.len(), keyed.is_empty()) {
        (1, false) => format!("Keyframes: {}", keyed.join(", ")),
        (1, true) => "Keyframes".to_owned(),
        (n, _) => format!("{n} keyframe marks"),
    };
    ui.label(egui::RichText::new(title).weak().size(12.0));
    let change = |change| TimelineEvent::Keyframes {
        clip,
        marks: times.clone(),
        change,
    };
    if let [one] = marks
        && ui.button("Go here").clicked()
    {
        events.push(TimelineEvent::Seek(one.at));
        ui.close();
    }
    if ui.button("Remove these keyframes").clicked() {
        events.push(change(MarksChange::Remove));
        ui.close();
    }
    if eases.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("From here on").weak().size(12.0));
    // Ticked when every mark goes on that way.
    let shared = marks
        .first()
        .and_then(|m| m.ease)
        .filter(|e| marks.iter().all(|m| m.ease == Some(*e)));
    for (i, name) in eases.iter().enumerate() {
        if ui.selectable_label(shared == Some(i), *name).clicked() {
            events.push(change(MarksChange::Ease(i)));
            ui.close();
        }
    }
}
