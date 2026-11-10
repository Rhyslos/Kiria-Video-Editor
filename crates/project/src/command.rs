//! Every change to a project is a [`Command`].
//!
//! A command takes the current snapshot and returns a new one, or an
//! explained error and no change. Commands are plain data, so they can be
//! written to the autosave journal and replayed after a crash.

use crate::ids::{ClipId, LabelId, LinkId, MediaId, SequenceId, TrackId};
use crate::model::{
    Clip, MediaKind, MediaRef, Project, Region, Sequence, SequenceSettings, TimelineLabel, Track, TrackKind,
};
use crate::time::Time;
use diag::AppError;
use serde::{Deserialize, Serialize};

/// One change to a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    RenameProject {
        name: String,
    },
    AddMedia {
        media: MediaRef,
    },
    /// Adds a media file and places it at the end of the timeline as one step:
    /// its picture on the first video track and each sound stream on its own
    /// audio track (made when missing, named after the stream), all linked.
    /// The media's `id` is ignored and a new one is given.
    Import {
        sequence: SequenceId,
        media: MediaRef,
    },
    AddTrack {
        sequence: SequenceId,
        kind: TrackKind,
        name: String,
    },
    /// Places a clip; its `id` is ignored and a new one is given.
    PlaceClip {
        sequence: SequenceId,
        track: TrackId,
        clip: Clip,
    },
    /// Removes a clip and the clips linked to it. With `ripple`, later clips
    /// on the same tracks move back to close the gap.
    RemoveClip {
        sequence: SequenceId,
        clip: ClipId,
        ripple: bool,
    },
    /// Moves a clip to a new start time, and optionally to another track of the same kind.
    MoveClip {
        sequence: SequenceId,
        clip: ClipId,
        start: Time,
        track: Option<TrackId>,
    },
    /// Moves a clip and every clip linked to it by `by` (later is positive),
    /// keeping them in step. Refused if any would overlap or start before zero.
    MoveLinked {
        sequence: SequenceId,
        clip: ClipId,
        by: Time,
    },
    /// Moves one edge of a clip (and the same edge of its linked clips) to
    /// `to` on the timeline. The start edge also moves where in the file the
    /// clip begins, so the picture stays in place.
    Trim {
        sequence: SequenceId,
        clip: ClipId,
        edge: Edge,
        to: Time,
    },
    /// Splits at `at`: only `clip` and its linked clips, or every clip under `at` when `None`.
    Split {
        sequence: SequenceId,
        at: Time,
        clip: Option<ClipId>,
    },
    /// Adds a timeline label; its `id` is ignored and a new one is given.
    AddLabel {
        sequence: SequenceId,
        label: TimelineLabel,
    },
    RemoveLabel {
        sequence: SequenceId,
        label: LabelId,
    },
    SetRegion {
        sequence: SequenceId,
        region: Option<Region>,
    },
    SetEditNeedle {
        sequence: SequenceId,
        at: Time,
    },
    /// Fixes a timeline's picture size, frame rate and sound rate (it stops being automatic).
    SetSettings {
        sequence: SequenceId,
        settings: SequenceSettings,
    },
}

impl Command {
    /// The new snapshot after this command, or why it could not be done.
    pub fn apply(&self, before: &Project) -> diag::Result<Project> {
        let mut p = before.clone();
        match self {
            Command::RenameProject { name } => {
                if name.trim().is_empty() {
                    return Err(AppError::new("A project needs a name").fix("Type at least one letter."));
                }
                p.name = name.clone();
            }
            Command::AddMedia { media } => {
                let id: MediaId = p.allocate_id();
                p.media.insert(id, MediaRef { id, ..media.clone() });
            }
            Command::Import { sequence, media } => import(&mut p, *sequence, media)?,
            Command::AddTrack { sequence, kind, name } => {
                let track = p.new_track(*kind, name);
                let seq = sequence_mut(&mut p, *sequence)?;
                match kind {
                    TrackKind::Video => seq.video_tracks.push_back(track),
                    TrackKind::Audio => seq.audio_tracks.push_back(track),
                }
            }
            Command::PlaceClip {
                sequence,
                track,
                clip,
            } => {
                if !p.media.contains_key(&clip.media) {
                    return Err(missing("media file"));
                }
                if clip.duration <= Time::ZERO || clip.start.is_negative() || clip.source_in.is_negative() {
                    return Err(AppError::new("The clip can't be placed there")
                        .why("It would start before the timeline or have no length."));
                }
                let id: ClipId = p.allocate_id();
                let track = track_mut(sequence_mut(&mut p, *sequence)?, *track)?;
                insert_clip(track, Clip { id, ..clip.clone() })?;
            }
            Command::RemoveClip {
                sequence,
                clip,
                ripple,
            } => {
                let seq = sequence_mut(&mut p, *sequence)?;
                let targets = with_linked(seq, *clip)?;
                for track in tracks_mut(seq) {
                    let removed: Vec<Clip> = track
                        .clips
                        .iter()
                        .filter(|c| targets.contains(&c.id))
                        .cloned()
                        .collect();
                    track.clips.retain(|c| !targets.contains(&c.id));
                    if *ripple {
                        for gone in removed.iter().rev() {
                            close_gap(track, gone.start, gone.duration);
                        }
                    }
                }
            }
            Command::MoveClip {
                sequence,
                clip,
                start,
                track,
            } => {
                if start.is_negative() {
                    return Err(
                        AppError::new("The clip can't move there").why("That is before the timeline starts.")
                    );
                }
                let seq = sequence_mut(&mut p, *sequence)?;
                let (from, mut moved) = take_clip(seq, *clip)?;
                moved.start = *start;
                let to = track.unwrap_or(from);
                let from_kind = track_mut(seq, from)?.kind;
                let target = track_mut(seq, to)?;
                if target.kind != from_kind {
                    return Err(AppError::new("The clip can't move to that track")
                        .why("Video clips go on video tracks and audio clips on audio tracks."));
                }
                insert_clip(target, moved)?;
            }
            Command::MoveLinked { sequence, clip, by } => move_linked(&mut p, *sequence, *clip, *by)?,
            Command::Trim {
                sequence,
                clip,
                edge,
                to,
            } => trim(&mut p, *sequence, *clip, *edge, *to)?,
            Command::Split { sequence, at, clip } => split(&mut p, *sequence, *at, *clip)?,
            Command::AddLabel { sequence, label } => {
                if label.at.is_negative() {
                    return Err(
                        AppError::new("A label can't go there").why("That is before the timeline starts.")
                    );
                }
                let id: LabelId = p.allocate_id();
                let seq = sequence_mut(&mut p, *sequence)?;
                let index = seq
                    .labels
                    .iter()
                    .position(|l| l.at > label.at)
                    .unwrap_or(seq.labels.len());
                seq.labels.insert(index, TimelineLabel { id, ..label.clone() });
            }
            Command::RemoveLabel { sequence, label } => {
                let seq = sequence_mut(&mut p, *sequence)?;
                let before_len = seq.labels.len();
                seq.labels.retain(|l| l.id != *label);
                if seq.labels.len() == before_len {
                    return Err(missing("label"));
                }
            }
            Command::SetRegion { sequence, region } => {
                if region.is_some_and(|r| r.start >= r.end || r.start.is_negative()) {
                    return Err(AppError::new("That region can't be set")
                        .why("A region must start at or after zero and end after it starts."));
                }
                sequence_mut(&mut p, *sequence)?.region = *region;
            }
            Command::SetSettings { sequence, settings } => {
                if settings.width == 0 || settings.height == 0 || settings.sample_rate == 0 {
                    return Err(AppError::new("Those timeline settings can't be used")
                        .why("The picture size and sound rate must be above zero."));
                }
                let seq = sequence_mut(&mut p, *sequence)?;
                seq.settings = *settings;
                seq.automatic = false;
            }
            Command::SetEditNeedle { sequence, at } => {
                sequence_mut(&mut p, *sequence)?.edit_needle = (*at).max(Time::ZERO);
            }
        }
        p.validate()?;
        Ok(p)
    }
}

/// One end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Start,
    End,
}

fn move_linked(p: &mut Project, sequence: SequenceId, clip: ClipId, by: Time) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let group = with_linked(seq, clip)?;
    let mut moved = Vec::new();
    for track in tracks_mut(seq) {
        let (ours, rest): (Vec<Clip>, Vec<Clip>) =
            track.clips.iter().cloned().partition(|c| group.contains(&c.id));
        if ours.is_empty() {
            continue;
        }
        track.clips = rest.into_iter().collect();
        moved.push((track.id, ours));
    }
    for (track, clips) in moved {
        let target = track_mut(seq, track)?;
        for mut c in clips {
            c.start = c.start + by;
            if c.start.is_negative() {
                return Err(
                    AppError::new("The clip can't move there").why("That is before the timeline starts.")
                );
            }
            insert_clip(target, c)?;
        }
    }
    Ok(())
}

fn trim(p: &mut Project, sequence: SequenceId, clip: ClipId, edge: Edge, to: Time) -> diag::Result<()> {
    let media_lengths: Vec<(MediaId, Time)> = p.media.values().map(|m| (m.id, m.duration)).collect();
    let still = |id: MediaId| p.media.get(&id).is_some_and(|m| m.kind == MediaKind::Image);
    let stills: Vec<MediaId> = p.media.keys().copied().filter(|id| still(*id)).collect();
    let seq = sequence_mut(p, sequence)?;
    let reference = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .cloned()
        .ok_or_else(|| missing("clip"))?;
    let group = with_linked(seq, clip)?;
    let delta = match edge {
        Edge::Start => to - reference.start,
        Edge::End => to - reference.end(),
    };
    for track in tracks_mut(seq) {
        let mut changed = Vec::new();
        for c in track.clips.iter().filter(|c| group.contains(&c.id)) {
            let mut t = c.clone();
            match edge {
                Edge::Start => {
                    t.start = c.start + delta;
                    t.source_in = c.source_in + delta;
                    t.duration = c.duration - delta;
                }
                Edge::End => t.duration = c.duration + delta,
            }
            if t.duration <= Time::ZERO {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("It would have no length left.")
                    .fix("Delete the clip instead."));
            }
            if t.source_in.is_negative() || t.start.is_negative() {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("Its file has nothing earlier to show."));
            }
            let length = media_lengths
                .iter()
                .find(|(id, _)| *id == c.media)
                .map(|(_, l)| *l);
            if !stills.contains(&c.media)
                && let Some(length) = length
                && length > Time::ZERO
                && t.source_in + t.duration > length
            {
                return Err(
                    AppError::new("The clip can't be trimmed that far").why("Its file ends before that.")
                );
            }
            changed.push(t);
        }
        for t in changed {
            track.clips.retain(|c| c.id != t.id);
            insert_clip(track, t)?;
        }
    }
    Ok(())
}

/// How long a still image lasts when it is placed.
pub const STILL_LENGTH: Time = Time::seconds(5);

fn import(p: &mut Project, sequence: SequenceId, media: &MediaRef) -> diag::Result<()> {
    let length = match media.kind {
        MediaKind::Image => STILL_LENGTH,
        MediaKind::Video | MediaKind::Audio => media.duration,
    };
    if length <= Time::ZERO {
        return Err(AppError::new("This file can't be placed")
            .why("Its length could not be read, so the app doesn't know how long the clip is.")
            .fix("Try re-saving the file with a recording or conversion tool, then import it again."));
    }
    let video = media
        .info
        .video
        .as_ref()
        .filter(|_| media.kind != MediaKind::Audio);
    let audio = if media.kind == MediaKind::Image {
        &[][..]
    } else {
        &media.info.audio[..]
    };
    if video.is_none() && audio.is_empty() {
        return Err(
            AppError::new("This file has nothing to place").why("No picture or sound was found in it.")
        );
    }

    // An automatic timeline takes its settings from the first video imported.
    if let Some(v) = video
        && media.kind == MediaKind::Video
    {
        let seq = sequence_mut(p, sequence)?;
        if seq.automatic {
            seq.settings = SequenceSettings::matching(v, &media.info.audio);
            seq.automatic = false;
        }
    }

    let id: MediaId = p.allocate_id();
    p.media.insert(id, MediaRef { id, ..media.clone() });
    let link: Option<LinkId> = (usize::from(video.is_some()) + audio.len() > 1).then(|| p.allocate_id());
    let at = sequence_ref(p, sequence)?.length();
    let clip = |p: &mut Project, stream: u32| Clip {
        id: p.allocate_id(),
        media: id,
        stream,
        start: at,
        source_in: Time::ZERO,
        duration: length,
        link,
    };

    if let Some(v) = video {
        let c = clip(p, v.index);
        if sequence_ref(p, sequence)?.video_tracks.is_empty() {
            let track = p.new_track(TrackKind::Video, "V1");
            sequence_mut(p, sequence)?.video_tracks.push_back(track);
        }
        let seq = sequence_mut(p, sequence)?;
        insert_clip(&mut seq.video_tracks[0], c)?;
    }
    for (i, stream) in audio.iter().enumerate() {
        let c = clip(p, stream.index);
        if sequence_ref(p, sequence)?.audio_tracks.len() <= i {
            let track = p.new_track(TrackKind::Audio, &stream.name);
            sequence_mut(p, sequence)?.audio_tracks.push_back(track);
        }
        let track = &mut sequence_mut(p, sequence)?.audio_tracks[i];
        // A track nobody has used yet takes the stream's name.
        if track.clips.is_empty() && track.name == format!("A{}", i + 1) {
            track.name = stream.name.clone();
        }
        insert_clip(track, c)?;
    }
    Ok(())
}

fn split(p: &mut Project, sequence: SequenceId, at: Time, only: Option<ClipId>) -> diag::Result<()> {
    let targets = match only {
        Some(clip) => Some(with_linked(sequence_ref(p, sequence)?, clip)?),
        None => None,
    };
    // Clips under `at` to cut, with a new link per original link so the right halves stay linked together.
    let mut cuts: Vec<(ClipId, Option<LinkId>)> = Vec::new();
    for track in sequence_ref(p, sequence)?.tracks() {
        for clip in &track.clips {
            let chosen = targets.as_ref().is_none_or(|t| t.contains(&clip.id));
            if chosen && clip.start < at && at < clip.end() {
                cuts.push((clip.id, clip.link));
            }
        }
    }
    if cuts.is_empty() {
        return Err(AppError::new("Nothing to split here")
            .why("No clip crosses the edit needle at this point.")
            .fix("Move the edit needle over a clip, then split."));
    }
    let mut new_links: Vec<(LinkId, LinkId)> = Vec::new();
    let mut right_ids: Vec<(ClipId, ClipId, Option<LinkId>)> = Vec::new();
    for (id, link) in cuts {
        let new_link = link.map(|old| match new_links.iter().find(|(o, _)| *o == old) {
            Some((_, n)) => *n,
            None => {
                let n = p.allocate_id();
                new_links.push((old, n));
                n
            }
        });
        right_ids.push((id, p.allocate_id(), new_link));
    }
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        for (left_id, right_id, right_link) in &right_ids {
            if let Some(i) = track.clips.iter().position(|c| c.id == *left_id) {
                let left = track.clips[i].clone();
                let first = at - left.start;
                let right = Clip {
                    id: *right_id,
                    start: at,
                    source_in: left.source_in + first,
                    duration: left.duration - first,
                    link: *right_link,
                    ..left.clone()
                };
                track.clips[i] = Clip {
                    duration: first,
                    ..left
                };
                track.clips.insert(i + 1, right);
            }
        }
    }
    Ok(())
}

fn sequence_ref(p: &Project, id: SequenceId) -> diag::Result<&Sequence> {
    p.sequences
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| missing("timeline"))
}

fn sequence_mut(p: &mut Project, id: SequenceId) -> diag::Result<&mut Sequence> {
    p.sequences
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| missing("timeline"))
}

fn tracks_mut(seq: &mut Sequence) -> impl Iterator<Item = &mut Track> {
    seq.video_tracks.iter_mut().chain(seq.audio_tracks.iter_mut())
}

fn track_mut(seq: &mut Sequence, id: TrackId) -> diag::Result<&mut Track> {
    tracks_mut(seq)
        .find(|t| t.id == id)
        .ok_or_else(|| missing("track"))
}

/// `clip` and every clip linked to it, on any track.
fn with_linked(seq: &Sequence, clip: ClipId) -> diag::Result<Vec<ClipId>> {
    let link = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .ok_or_else(|| missing("clip"))?
        .link;
    Ok(seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.id == clip || (link.is_some() && c.link == link))
        .map(|c| c.id)
        .collect())
}

/// Removes a clip from whichever track holds it.
fn take_clip(seq: &mut Sequence, clip: ClipId) -> diag::Result<(TrackId, Clip)> {
    for track in tracks_mut(seq) {
        if let Some(i) = track.clips.iter().position(|c| c.id == clip) {
            return Ok((track.id, track.clips.remove(i)));
        }
    }
    Err(missing("clip"))
}

/// Inserts in start order; refuses an overlap.
fn insert_clip(track: &mut Track, clip: Clip) -> diag::Result<()> {
    if track
        .clips
        .iter()
        .any(|c| c.start < clip.end() && clip.start < c.end())
    {
        return Err(AppError::new("There is already a clip there")
            .why(format!("It would overlap another clip on {}.", track.name))
            .fix("Move it to an empty spot or another track."));
    }
    let index = track
        .clips
        .iter()
        .position(|c| c.start > clip.start)
        .unwrap_or(track.clips.len());
    track.clips.insert(index, clip);
    Ok(())
}

/// Moves every clip that starts at or after `from` back by `by`.
fn close_gap(track: &mut Track, from: Time, by: Time) {
    for clip in track.clips.iter_mut() {
        if clip.start >= from {
            clip.start = clip.start - by;
        }
    }
}

fn missing(thing: &str) -> AppError {
    AppError::new(format!("That {thing} no longer exists"))
        .why("It may have been removed by an earlier change.")
        .fix("Undo the last change if this was unexpected.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioStream, Fingerprint, MediaInfo, SequenceSettings, VideoStream};
    use crate::time::Rate;

    struct Fixture {
        p: Project,
        seq: SequenceId,
        v1: TrackId,
        a1: TrackId,
        media: MediaId,
    }

    fn fixture() -> Fixture {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let p = Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: "rec.mkv".into(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: "x".into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(100),
                info: MediaInfo::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let seq = p.active().unwrap();
        Fixture {
            seq: seq.id,
            v1: seq.video_tracks[0].id,
            a1: seq.audio_tracks[0].id,
            media: *p.media.keys().next().unwrap(),
            p,
        }
    }

    fn clip(f: &Fixture, start: i64, len: i64, link: Option<LinkId>) -> Clip {
        Clip {
            id: ClipId(0),
            media: f.media,
            stream: 0,
            start: Time::seconds(start),
            source_in: Time::ZERO,
            duration: Time::seconds(len),
            link,
        }
    }

    fn place(f: &Fixture, p: &Project, track: TrackId, c: Clip) -> diag::Result<Project> {
        Command::PlaceClip {
            sequence: f.seq,
            track,
            clip: c,
        }
        .apply(p)
    }

    fn clips(p: &Project, track: usize, audio: bool) -> Vec<(i64, i64)> {
        let seq = p.active().unwrap();
        let t = if audio {
            &seq.audio_tracks[track]
        } else {
            &seq.video_tracks[track]
        };
        t.clips
            .iter()
            .map(|c| (c.start.as_secs_f64() as i64, c.duration.as_secs_f64() as i64))
            .collect()
    }

    #[test]
    fn overlapping_placement_is_refused_and_nothing_changes() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let err = place(&f, &p, f.v1, clip(&f, 5, 10, None)).unwrap_err();
        assert!(err.what().contains("already a clip"));
        assert_eq!(clips(&p, 0, false), vec![(0, 10)]);
    }

    #[test]
    fn split_with_a_clip_selected_cuts_it_and_its_linked_audio_only() {
        let mut f = fixture();
        let link = Some(f.p.allocate_id());
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 0, 10, link)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 20, 10, None)).unwrap();
        let v = p.active().unwrap().video_tracks[0].clips[0].id;
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(4),
            clip: Some(v),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&p, 0, false), vec![(0, 4), (4, 6), (20, 10)]);
        assert_eq!(clips(&p, 0, true), vec![(0, 4), (4, 6)]);
        let seq = p.active().unwrap();
        let (rv, ra) = (&seq.video_tracks[0].clips[1], &seq.audio_tracks[0].clips[1]);
        assert_eq!(rv.link, ra.link, "the right halves stay linked to each other");
        assert_ne!(rv.link, link, "but not to the left halves");
        assert_eq!(rv.source_in, Time::seconds(4));
    }

    #[test]
    fn split_with_nothing_selected_cuts_every_track() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.a1, clip(&f, 2, 10, None)).unwrap();
        let p = Command::Split {
            sequence: f.seq,
            at: Time::seconds(5),
            clip: None,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&p, 0, false), vec![(0, 5), (5, 5)]);
        assert_eq!(clips(&p, 0, true), vec![(2, 3), (5, 7)]);
        assert!(
            Command::Split {
                sequence: f.seq,
                at: Time::seconds(50),
                clip: None
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn delete_leaves_a_gap_and_ripple_delete_closes_it() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 10, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 10, 5, None)).unwrap();
        let p = place(&f, &p, f.v1, clip(&f, 20, 5, None)).unwrap();
        let middle = p.active().unwrap().video_tracks[0].clips[1].id;
        let gap = Command::RemoveClip {
            sequence: f.seq,
            clip: middle,
            ripple: false,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&gap, 0, false), vec![(0, 10), (20, 5)]);
        let closed = Command::RemoveClip {
            sequence: f.seq,
            clip: middle,
            ripple: true,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&closed, 0, false), vec![(0, 10), (15, 5)]);
    }

    #[test]
    fn moving_keeps_order_and_refuses_the_wrong_kind_of_track() {
        let f = fixture();
        let p = place(&f, &f.p, f.v1, clip(&f, 0, 5, None)).unwrap();
        let id = p.active().unwrap().video_tracks[0].clips[0].id;
        let moved = Command::MoveClip {
            sequence: f.seq,
            clip: id,
            start: Time::seconds(30),
            track: None,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(clips(&moved, 0, false), vec![(30, 5)]);
        let wrong = Command::MoveClip {
            sequence: f.seq,
            clip: id,
            start: Time::ZERO,
            track: Some(f.a1),
        }
        .apply(&p);
        assert!(wrong.is_err());
    }

    #[test]
    fn labels_stay_in_time_order() {
        let f = fixture();
        let label = |at| TimelineLabel {
            id: LabelId(0),
            name: "x".into(),
            at: Time::seconds(at),
            color: crate::model::LabelColor::Blue,
        };
        let p = Command::AddLabel {
            sequence: f.seq,
            label: label(30),
        }
        .apply(&f.p)
        .unwrap();
        let p = Command::AddLabel {
            sequence: f.seq,
            label: label(10),
        }
        .apply(&p)
        .unwrap();
        let times: Vec<_> = p.active().unwrap().labels.iter().map(|l| l.at).collect();
        assert_eq!(times, vec![Time::seconds(10), Time::seconds(30)]);
    }

    fn recording(audio: &[&str], seconds: i64) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: "match.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: "y".into(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(seconds),
            info: MediaInfo {
                container: "matroska,webm".into(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "hevc".into(),
                    width: 2560,
                    height: 1440,
                    frame_rate: Rate::FPS_60,
                    rotation: 0,
                    variable_frame_rate: false,
                }),
                audio: audio
                    .iter()
                    .enumerate()
                    .map(|(i, name)| AudioStream {
                        index: i as u32 + 1,
                        codec: "aac".into(),
                        channels: 2,
                        sample_rate: 48_000,
                        name: (*name).into(),
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn importing_puts_each_sound_stream_on_its_own_named_track() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game", "Mic", "Voice chat"], 30),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game", "Mic", "Voice chat"]);
        assert_eq!(s.video_tracks[0].clips.len(), 1);
        let all: Vec<&Clip> = s.tracks().flat_map(|t| t.clips.iter()).collect();
        assert_eq!(all.len(), 4);
        assert!(
            all.iter().all(|c| c.link.is_some() && c.link == all[0].link),
            "all linked"
        );
        let streams: Vec<u32> = s.audio_tracks.iter().map(|t| t.clips[0].stream).collect();
        assert_eq!(streams, [1, 2, 3]);
        assert_eq!(s.length(), Time::seconds(30));
    }

    #[test]
    fn a_second_import_goes_after_the_first_and_keeps_track_names() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 30),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Other", "Mic"], 10),
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        let names: Vec<&str> = s.audio_tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game", "Mic"], "a used track keeps its name");
        assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(30));
        assert_eq!(s.length(), Time::seconds(40));
        assert_eq!(p.media.len(), 2);
    }

    #[test]
    fn images_get_a_fixed_length_and_files_without_length_are_refused() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let mut image = recording(&[], 0);
        image.kind = MediaKind::Image;
        let q = Command::Import {
            sequence: seq,
            media: image,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(q.active().unwrap().length(), STILL_LENGTH);
        assert!(q.active().unwrap().video_tracks[0].clips[0].link.is_none());

        let err = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 0),
        }
        .apply(&p)
        .unwrap_err();
        assert!(err.what().contains("can't be placed"));
    }

    #[test]
    fn an_automatic_timeline_takes_the_first_videos_settings() {
        let p = Project::new_automatic("Test");
        let seq = p.active_sequence;
        assert!(p.active().unwrap().automatic);

        // Sound alone doesn't settle it.
        let mut voice = recording(&["Voice"], 5);
        voice.kind = MediaKind::Audio;
        voice.info.video = None;
        let p = Command::Import {
            sequence: seq,
            media: voice,
        }
        .apply(&p)
        .unwrap();
        assert!(p.active().unwrap().automatic);

        let mut phone = recording(&["Audio 1"], 10);
        if let Some(v) = phone.info.video.as_mut() {
            (v.width, v.height, v.rotation, v.frame_rate) = (1920, 1080, 90, Rate::NTSC_30);
        }
        phone.info.audio[0].sample_rate = 44_100;
        let p = Command::Import {
            sequence: seq,
            media: phone,
        }
        .apply(&p)
        .unwrap();
        let s = p.active().unwrap();
        assert!(!s.automatic);
        assert_eq!(
            s.settings,
            SequenceSettings {
                width: 1080,
                height: 1920,
                frame_rate: Rate::NTSC_30,
                sample_rate: 44_100
            }
        );

        // Later videos don't change it.
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.active().unwrap().settings.width, 1080);
    }

    #[test]
    fn a_project_with_fixed_settings_keeps_them() {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(p.active().unwrap().settings, SequenceSettings::GAME_CLIP);
    }

    #[test]
    fn settings_can_be_set_by_hand_and_must_make_sense() {
        let p = Project::new_automatic("Test");
        let seq = p.active_sequence;
        let q = Command::SetSettings {
            sequence: seq,
            settings: SequenceSettings::VERTICAL_SHORT,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(q.active().unwrap().settings, SequenceSettings::VERTICAL_SHORT);
        assert!(!q.active().unwrap().automatic);
        let bad = SequenceSettings {
            width: 0,
            ..SequenceSettings::GAME_CLIP
        };
        assert!(
            Command::SetSettings {
                sequence: seq,
                settings: bad
            }
            .apply(&p)
            .is_err()
        );
    }

    fn imported() -> (Project, SequenceId) {
        let p = Project::new("Test", SequenceSettings::GAME_CLIP);
        let seq = p.active_sequence;
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game", "Mic"], 30),
        }
        .apply(&p)
        .unwrap();
        let p = Command::Import {
            sequence: seq,
            media: recording(&["Game"], 10),
        }
        .apply(&p)
        .unwrap();
        (p, seq)
    }

    fn first_video(p: &Project, i: usize) -> Clip {
        p.active().unwrap().video_tracks[0].clips[i].clone()
    }

    #[test]
    fn moving_takes_linked_clips_along() {
        let (p, seq) = imported();
        let second = first_video(&p, 1).id;
        let q = Command::MoveLinked {
            sequence: seq,
            clip: second,
            by: Time::seconds(5),
        }
        .apply(&p)
        .unwrap();
        let s = q.active().unwrap();
        assert_eq!(s.video_tracks[0].clips[1].start, Time::seconds(35));
        assert_eq!(
            s.audio_tracks[0].clips[1].start,
            Time::seconds(35),
            "its sound came along"
        );
        let back = Command::MoveLinked {
            sequence: seq,
            clip: second,
            by: Time::seconds(-5),
        }
        .apply(&p);
        assert!(
            back.unwrap_err().what().contains("already a clip"),
            "it would overlap the first"
        );
        let first = first_video(&p, 0).id;
        assert!(
            Command::MoveLinked {
                sequence: seq,
                clip: first,
                by: Time::seconds(-1)
            }
            .apply(&p)
            .is_err()
        );
    }

    #[test]
    fn trimming_either_edge_keeps_the_picture_in_place() {
        let (p, seq) = imported();
        let first = first_video(&p, 0).id;
        let q = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(4),
        }
        .apply(&p)
        .unwrap();
        let c = first_video(&q, 0);
        assert_eq!(
            (c.start, c.source_in, c.duration),
            (Time::seconds(4), Time::seconds(4), Time::seconds(26))
        );
        let mic = &q.active().unwrap().audio_tracks[1].clips[0];
        assert_eq!(mic.start, Time::seconds(4), "linked sound trimmed too");
        // Back out again, but not before the file starts.
        let r = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::Start,
            to: Time::seconds(1),
        }
        .apply(&q)
        .unwrap();
        assert_eq!(first_video(&r, 0).source_in, Time::seconds(1));
        assert!(
            Command::Trim {
                sequence: seq,
                clip: first,
                edge: Edge::Start,
                to: Time::seconds(-1)
            }
            .apply(&q)
            .is_err()
        );

        let e = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::seconds(20),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(first_video(&e, 0).duration, Time::seconds(20));
        let long = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::seconds(31),
        }
        .apply(&e);
        assert!(
            long.unwrap_err()
                .why_text()
                .unwrap_or_default()
                .contains("ends before")
        );
        let gone = Command::Trim {
            sequence: seq,
            clip: first,
            edge: Edge::End,
            to: Time::ZERO,
        }
        .apply(&p);
        assert!(gone.is_err());
    }
}
