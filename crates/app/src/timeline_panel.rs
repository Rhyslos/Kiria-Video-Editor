//! Turns the open timeline into what the shared timeline widget draws, and
//! keeps the selection.

use crate::import;
use project::{ClipId, LabelColor, Project, Sequence};
use ui_kit::{TimelineClip, TimelineMarker, TimelineTrack};

/// What the widget needs for one frame.
pub struct Drawn {
    pub video: Vec<TimelineTrack>,
    pub audio: Vec<TimelineTrack>,
    pub markers: Vec<TimelineMarker>,
}

/// Builds the widget's data. Clips in `selected`, and clips linked to them,
/// show as selected.
pub fn drawn(project: &Project, seq: &Sequence, selected: &[ClipId]) -> Drawn {
    let links: Vec<_> = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| selected.contains(&c.id))
        .filter_map(|c| c.link)
        .collect();
    let track = |t: &project::Track| TimelineTrack {
        id: t.id.0,
        name: t.name.clone(),
        muted: t.muted,
        solo: t.solo,
        clips: t
            .clips
            .iter()
            .map(|c| {
                let media = project.media.get(&c.media);
                let name = media
                    .and_then(|m| m.path.file_name())
                    .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned());
                TimelineClip {
                    id: c.id.0,
                    start: c.start.as_secs_f64(),
                    duration: c.duration.as_secs_f64(),
                    info: media.map_or_else(String::new, import::summary),
                    name,
                    selected: selected.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)),
                    missing: media.is_none(),
                    link: c.link.map(|l| l.0),
                }
            })
            .collect(),
    };
    Drawn {
        video: seq.video_tracks.iter().map(track).collect(),
        audio: seq.audio_tracks.iter().map(track).collect(),
        markers: seq
            .labels
            .iter()
            .map(|l| TimelineMarker {
                at: l.at.as_secs_f64(),
                name: l.name.clone(),
                colour: label_colour(l.color),
            })
            .collect(),
    }
}

/// The colour each timeline-label colour is drawn in.
pub fn label_colour(c: LabelColor) -> egui::Color32 {
    use egui::Color32 as C;
    match c {
        LabelColor::Blue => C::from_rgb(0x4F, 0x8C, 0xE0),
        LabelColor::Green => C::from_rgb(0x5D, 0xBB, 0x7A),
        LabelColor::Pink => C::from_rgb(0xE0, 0x6F, 0xB4),
        LabelColor::Yellow => C::from_rgb(0xE6, 0xC3, 0x5C),
        LabelColor::Orange => C::from_rgb(0xE8, 0x8A, 0x3C),
        LabelColor::Purple => C::from_rgb(0x9A, 0x6C, 0xD8),
        LabelColor::Red => C::from_rgb(0xE5, 0x48, 0x4D),
        LabelColor::Grey => C::from_rgb(0x8A, 0x8F, 0x96),
    }
}

/// Clicking a clip: alone, or added to (or taken out of) the selection.
pub fn select(selected: &mut Vec<ClipId>, clip: ClipId, add: bool) {
    if !add {
        selected.clear();
        selected.push(clip);
    } else if let Some(i) = selected.iter().position(|c| *c == clip) {
        selected.remove(i);
    } else {
        selected.push(clip);
    }
}

/// Drops clips that no longer exist (after a delete or an undo).
pub fn keep_existing(selected: &mut Vec<ClipId>, seq: &Sequence) {
    selected.retain(|id| seq.tracks().any(|t| t.clips.iter().any(|c| c.id == *id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_alone_adding_and_taking_out() {
        let mut s = Vec::new();
        select(&mut s, ClipId(1), false);
        select(&mut s, ClipId(2), true);
        assert_eq!(s, [ClipId(1), ClipId(2)]);
        select(&mut s, ClipId(1), true);
        assert_eq!(s, [ClipId(2)]);
        select(&mut s, ClipId(3), false);
        assert_eq!(s, [ClipId(3)]);
    }
}
