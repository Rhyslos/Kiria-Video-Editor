//! Clips copied from a timeline and pasted into the same timeline or
//! another one of the project.
//!
//! A copy holds the chosen clips and every clip linked to them, each on
//! the track of its kind and number it came from, with its start measured
//! from the earliest copied clip's. Everything about a clip comes along:
//! its look, keyframes, speed, ramp, sound and lines; a nested clip still
//! plays its timeline and an adjustment layer is still one.
//!
//! Pasting follows the rule keyframes have ("Paste places what fits and
//! replaces clashes"): the earliest copied clip starts at the paste point
//! and the others as far after it as they were, each on the track of the
//! same kind and number (tracks that are missing are made, as a template
//! makes them). What a pasted clip lands on is cut away on its track, as a
//! file dropped on a track replaces what it covers; nothing else moves. A
//! nested clip that would make the timeline play itself, or a clip whose
//! file or timeline is no longer in the project, is left out
//! ([`CopiedClips::left_out`] says how many) and the rest are pasted; a
//! paste where nothing fits is refused with the reason. Copied clips that
//! were linked are linked to each other again, never to the originals.

use crate::command::{insert_clip, missing, sequence_mut, sequence_ref, settle_automatic, with_linked};
use crate::ids::{ClipId, LinkId, SequenceId};
use crate::model::{Clip, ClipKind, MediaKind, Project, Track, TrackKind};
use crate::time::Time;
use diag::AppError;
use serde::{Deserialize, Serialize};

/// Clips copied from a timeline, ready to paste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopiedClips {
    /// The video tracks from the first up to the highest one copied from,
    /// each holding only the clips copied from it, their starts counted
    /// from the earliest copied clip's start.
    pub video_tracks: Vec<Track>,
    /// The same for audio tracks.
    pub audio_tracks: Vec<Track>,
}

impl CopiedClips {
    /// `clips` of `sequence` and every clip linked to them. Refused when
    /// none is given or one no longer exists.
    pub fn copy(project: &Project, sequence: SequenceId, clips: &[ClipId]) -> diag::Result<CopiedClips> {
        let seq = sequence_ref(project, sequence)?;
        let mut wanted = Vec::new();
        for clip in clips {
            wanted.extend(with_linked(seq, *clip)?);
        }
        let first = seq
            .tracks()
            .flat_map(|t| t.clips.iter())
            .filter(|c| wanted.contains(&c.id))
            .map(|c| c.start)
            .min();
        let Some(first) = first else {
            return Err(AppError::new("No clips to copy")
                .why("No clip is selected.")
                .fix("Click a clip on the timeline first (Shift-click more)."));
        };
        let keep = |tracks: &imbl::Vector<Track>| -> Vec<Track> {
            let mut kept: Vec<Track> = tracks
                .iter()
                .map(|t| Track {
                    clips: t
                        .clips
                        .iter()
                        .filter(|c| wanted.contains(&c.id))
                        .map(|c| Clip {
                            start: c.start - first,
                            ..c.clone()
                        })
                        .collect(),
                    ..t.clone()
                })
                .collect();
            while kept.last().is_some_and(|t| t.clips.is_empty()) {
                kept.pop();
            }
            kept
        };
        Ok(CopiedClips {
            video_tracks: keep(&seq.video_tracks),
            audio_tracks: keep(&seq.audio_tracks),
        })
    }

    /// Every copied clip.
    pub fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.video_tracks
            .iter()
            .chain(self.audio_tracks.iter())
            .flat_map(|t| t.clips.iter())
    }

    /// How many clips it holds.
    pub fn len(&self) -> usize {
        self.clips().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many of its clips can't go into `sequence` of `project`: a
    /// nested clip that would make it play itself, or a clip whose file or
    /// timeline is no longer in the project.
    pub fn left_out(&self, project: &Project, sequence: SequenceId) -> usize {
        self.clips().filter(|c| !fits(project, sequence, c)).count()
    }

    /// "3 clips", "1 clip".
    pub fn says(&self) -> String {
        match self.len() {
            1 => "1 clip".to_owned(),
            n => format!("{n} clips"),
        }
    }
}

/// Whether `clip` can be pasted into `sequence`.
fn fits(project: &Project, sequence: SequenceId, clip: &Clip) -> bool {
    match clip.kind {
        ClipKind::File => project.media.contains_key(&clip.media),
        ClipKind::Timeline(inner) => project.sequence(inner).is_some() && !project.plays(inner, sequence),
        ClipKind::Adjustment => true,
    }
}

/// See [`crate::Command::PasteClips`].
pub(crate) fn paste(
    p: &mut Project,
    sequence: SequenceId,
    at: Time,
    copied: &CopiedClips,
) -> diag::Result<()> {
    if copied.is_empty() {
        return Err(AppError::new("Nothing to paste")
            .why("No clips were copied.")
            .fix("Select clips on a timeline and copy them first."));
    }
    if at.is_negative() {
        return Err(
            AppError::new("The clips can't be pasted there").why("That is before the timeline starts.")
        );
    }
    let name = sequence_ref(p, sequence)?.name.clone();
    if copied.left_out(p, sequence) == copied.len() {
        return Err(AppError::new(format!("Nothing copied can go into {name}"))
            .why("Each copied clip plays this timeline (it can't play inside itself), or a file or timeline no longer in the project.")
            .fix("Paste into another timeline."));
    }
    // A timeline still waiting for its first video takes its settings from
    // the first pasted one's file.
    let first_video = copied
        .video_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_file())
        .filter_map(|c| p.media.get(&c.media))
        .find(|m| m.kind == MediaKind::Video && m.info.video.is_some())
        .cloned();
    if let Some(video) = first_video {
        settle_automatic(p, sequence, &video)?;
    }
    let rate = sequence_ref(p, sequence)?.settings.frame_rate;
    let at = rate.snap(at);
    // Where each goes: the kind and number of its track, and its new self.
    let mut links: Vec<(LinkId, LinkId)> = Vec::new();
    let mut placing: Vec<(TrackKind, usize, Clip)> = Vec::new();
    for (kind, tracks) in [
        (TrackKind::Video, &copied.video_tracks),
        (TrackKind::Audio, &copied.audio_tracks),
    ] {
        for (index, track) in tracks.iter().enumerate() {
            let fitting: Vec<&Clip> = track.clips.iter().filter(|c| fits(p, sequence, c)).collect();
            for clip in fitting {
                let link = clip.link.map(|old| match links.iter().find(|(o, _)| *o == old) {
                    Some((_, new)) => *new,
                    None => {
                        let new: LinkId = p.allocate_id();
                        links.push((old, new));
                        new
                    }
                });
                let placed = Clip {
                    id: p.allocate_id(),
                    start: at + rate.snap(clip.start),
                    link,
                    ..clip.clone()
                };
                placing.push((kind, index, placed));
            }
        }
    }
    // Every track a clip lands on is there, then what each covers goes.
    for (kind, tracks) in [
        (TrackKind::Video, &copied.video_tracks),
        (TrackKind::Audio, &copied.audio_tracks),
    ] {
        let wanted = placing
            .iter()
            .filter(|(k, ..)| *k == kind)
            .map(|(_, i, _)| *i + 1)
            .max();
        if let Some(count) = wanted {
            crate::templates::make_tracks(p, sequence, kind, &tracks[..count])?;
        }
    }
    let track_id = |p: &Project, kind: TrackKind, index: usize| -> diag::Result<crate::ids::TrackId> {
        let seq = sequence_ref(p, sequence)?;
        let tracks = match kind {
            TrackKind::Video => &seq.video_tracks,
            TrackKind::Audio => &seq.audio_tracks,
        };
        tracks.get(index).map(|t| t.id).ok_or_else(|| missing("track"))
    };
    for (kind, index, clip) in &placing {
        let track = track_id(p, *kind, *index)?;
        crate::edits::clear_space_on(p, sequence, clip.start, clip.duration, &[track])?;
    }
    for (kind, index, clip) in placing {
        let track = track_id(p, kind, index)?;
        let seq = sequence_mut(p, sequence)?;
        let target = crate::command::tracks_mut(seq)
            .find(|t| t.id == track)
            .ok_or_else(|| missing("track"))?;
        insert_clip(target, clip)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
