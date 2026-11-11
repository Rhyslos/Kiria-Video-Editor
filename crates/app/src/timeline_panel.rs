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
/// show as selected; clips of `missing` media show as missing. `pictures`
/// gives a video stream's pictures when ready. `waveform` gives a sound stream's peaks when ready.
pub fn drawn(
    project: &Project,
    seq: &Sequence,
    selected: &[ClipId],
    missing: &[project::MediaId],
    mut waveform: impl FnMut(&project::MediaRef, u32) -> Option<cache::Peaks>,
    mut pictures: impl FnMut(&project::MediaRef, u32) -> Option<ui_kit::ClipPictures>,
) -> Drawn {
    let links: Vec<_> = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| selected.contains(&c.id))
        .filter_map(|c| c.link)
        .collect();
    let mut track = |t: &project::Track| TimelineTrack {
        id: t.id.0,
        name: t.name.clone(),
        muted: t.muted,
        solo: t.solo,
        gain_db: t.gain_db,
        clips: t
            .clips
            .iter()
            .map(|c| {
                let media = project.media.get(&c.media);
                let name = media
                    .and_then(|m| m.path.file_name())
                    .map_or_else(|| "missing".into(), |n| n.to_string_lossy().into_owned());
                let name = if c.speed == project::Speed::NORMAL {
                    name
                } else {
                    format!("{name} · {}", c.speed.percent())
                };
                TimelineClip {
                    id: c.id.0,
                    start: c.start.as_secs_f64(),
                    duration: c.duration.as_secs_f64(),
                    info: media.map_or_else(String::new, import::summary),
                    name,
                    selected: selected.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)),
                    missing: media.is_none_or(|m| missing.contains(&m.id)),
                    link: c.link.map(|l| l.0),
                    waveform: match (t.kind, media) {
                        (project::TrackKind::Audio, Some(m)) => {
                            waveform(m, c.stream).map(|p| ui_kit::ClipWaveform {
                                peaks: p.values,
                                per_second: p.per_second as f32,
                                source_in: c.source_in.as_secs_f64(),
                                speed: c.speed.as_f64(),
                            })
                        }
                        _ => None,
                    },
                    pictures: match (t.kind, media) {
                        (project::TrackKind::Video, Some(m)) if !missing.contains(&m.id) => {
                            pictures(m, c.stream).map(|p| ui_kit::ClipPictures {
                                source_in: c.source_in.as_secs_f64(),
                                speed: c.speed.as_f64(),
                                ..p
                            })
                        }
                        _ => None,
                    },
                }
            })
            .collect(),
    };
    Drawn {
        video: seq.video_tracks.iter().map(&mut track).collect(),
        audio: seq.audio_tracks.iter().map(&mut track).collect(),
        markers: seq
            .labels
            .iter()
            .map(|l| TimelineMarker {
                id: l.id.0,
                at: l.at.as_secs_f64(),
                name: l.name.clone(),
                colour: label_colour(l.color),
            })
            .collect(),
    }
}

/// The rough cut view: each video clip shows its sound inside it (the first
/// sound clip linked to it), and sound clips that belong to a video clip
/// leave the audio area. Tracks left empty there are not shown.
pub fn fold_sound(drawn: &mut Drawn) {
    let video_links: Vec<u64> = drawn
        .video
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| c.link)
        .collect();
    for clip in drawn.video.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        let Some(link) = clip.link else { continue };
        clip.waveform = drawn
            .audio
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|a| a.link == Some(link))
            .and_then(|a| a.waveform.clone());
    }
    for track in &mut drawn.audio {
        track
            .clips
            .retain(|c| c.link.is_none_or(|l| !video_links.contains(&l)));
    }
    drawn.audio.retain(|t| !t.clips.is_empty());
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

    fn clip(id: u64, link: Option<u64>, peaks: Option<f32>) -> ui_kit::TimelineClip {
        ui_kit::TimelineClip {
            id,
            start: 0.0,
            duration: 1.0,
            name: String::new(),
            info: String::new(),
            selected: false,
            missing: false,
            link,
            waveform: peaks.map(|p| ui_kit::ClipWaveform {
                peaks: vec![p].into(),
                per_second: 1.0,
                source_in: 0.0,
                speed: 1.0,
            }),
            pictures: None,
        }
    }

    fn track(id: u64, clips: Vec<ui_kit::TimelineClip>) -> TimelineTrack {
        TimelineTrack {
            id,
            name: String::new(),
            clips,
            muted: false,
            solo: false,
            gain_db: 0.0,
        }
    }

    #[test]
    fn the_rough_cut_folds_linked_sound_into_its_picture() {
        let mut d = Drawn {
            video: vec![track(1, vec![clip(10, Some(1), None)])],
            audio: vec![
                track(2, vec![clip(20, Some(1), Some(0.5))]),
                track(3, vec![clip(30, Some(1), Some(0.9)), clip(31, None, Some(0.2))]),
            ],
            markers: Vec::new(),
        };
        fold_sound(&mut d);
        let w = d.video[0].clips[0].waveform.as_ref().unwrap();
        assert_eq!(w.peaks[0], 0.5, "the first sound track's");
        assert_eq!(d.audio.len(), 1, "the emptied track is gone");
        assert_eq!(
            d.audio[0].clips.iter().map(|c| c.id).collect::<Vec<_>>(),
            [31],
            "unlinked sound stays"
        );
    }
}
