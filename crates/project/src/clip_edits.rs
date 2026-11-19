//! The plain clip edits: importing and placing files (at the end, inserted,
//! over what is there, dropped on a track), placing a clip, trimming,
//! removing (with or without closing the gaps), closing a gap and moving
//! clips (on their track, with their linked clips, to another track, or
//! several together). [`crate::Command::apply`] hands each of these commands to one
//! function here; the trim tools beyond a plain trim and the room-making
//! behind insert and overwrite are in `edits`, laying over a track in
//! `layers`.

use crate::command::{
    Edge, insert_clip, missing, overlap_error, sequence_mut, sequence_ref, track_mut, tracks_mut, with_linked,
};
use crate::ids::{ClipId, LinkId, MediaId, SequenceId, TrackId};
use crate::look::Look;
use crate::model::{Clip, MediaKind, MediaRef, Project, Sequence, SequenceSettings, Track, TrackKind};
use crate::time::{Speed, Time};
use diag::AppError;
use std::collections::HashSet;

/// See [`crate::Command::ImportSettled`]: an automatic timeline takes `settings`,
/// then the file is imported as [`crate::Command::Import`] imports it.
pub(crate) fn import_settled(
    p: &mut Project,
    sequence: SequenceId,
    media: &MediaRef,
    settings: &SequenceSettings,
) -> diag::Result<()> {
    crate::timelines::check_settings(settings)?;
    let seq = sequence_mut(p, sequence)?;
    if seq.automatic {
        seq.settings = *settings;
        seq.automatic = false;
    }
    import(p, sequence, media)
}

/// See [`crate::Command::InsertMedia`]: everything from `at` on moves later by the
/// file's length, then the file goes in at `at`.
pub(crate) fn insert_media(
    p: &mut Project,
    sequence: SequenceId,
    media: MediaId,
    at: Time,
) -> diag::Result<()> {
    let length = placed_length(p.media.get(&media).ok_or_else(|| missing("media file"))?);
    crate::edits::open_space(p, sequence, at, length)?;
    place_media(p, sequence, media, Some(at), None)
}

/// See [`crate::Command::OverwriteMedia`]: what the file covers from `at` is cut
/// away on every track, then the file goes in at `at`.
pub(crate) fn overwrite_media(
    p: &mut Project,
    sequence: SequenceId,
    media: MediaId,
    at: Time,
) -> diag::Result<()> {
    let length = placed_length(p.media.get(&media).ok_or_else(|| missing("media file"))?);
    crate::edits::clear_space(p, sequence, at, length)?;
    place_media(p, sequence, media, Some(at), None)
}

/// See [`crate::Command::PlaceClip`]: `clip` (given a new id) on `track`.
pub(crate) fn place_clip(
    p: &mut Project,
    sequence: SequenceId,
    track: TrackId,
    clip: &Clip,
) -> diag::Result<()> {
    if clip.is_file() && !p.media.contains_key(&clip.media) {
        return Err(missing("media file"));
    }
    if clip.duration <= Time::ZERO || clip.start.is_negative() || clip.starts_before_its_source() {
        return Err(AppError::new("The clip can't be placed there")
            .why("It would start before the timeline or have no length."));
    }
    let id: ClipId = p.allocate_id();
    let track = track_mut(sequence_mut(p, sequence)?, track)?;
    insert_clip(track, Clip { id, ..clip.clone() })
}

/// See [`crate::Command::CloseGap`]: the stretch from `start` to `end` closes on
/// every track, or nothing changes.
pub(crate) fn close_gap_between(
    p: &mut Project,
    sequence: SequenceId,
    start: Time,
    end: Time,
) -> diag::Result<()> {
    if end <= start || start.is_negative() {
        return Err(AppError::new("There is no gap there").why("The stretch ends before it starts."));
    }
    close_stretches(p, sequence, &[(start, end)], &HashSet::new())
}

/// See [`crate::Command::MoveClip`]: `clip` alone to `start`, on `track` (of its
/// own kind) or its own.
pub(crate) fn move_clip(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    start: Time,
    track: Option<TrackId>,
) -> diag::Result<()> {
    if start.is_negative() {
        return Err(AppError::new("The clip can't move there").why("That is before the timeline starts."));
    }
    let seq = sequence_mut(p, sequence)?;
    let (from, mut moved) = take_clip(seq, clip)?;
    moved.start = start;
    let to = track.unwrap_or(from);
    let from_kind = track_mut(seq, from)?.kind;
    let target = track_mut(seq, to)?;
    if target.kind != from_kind {
        return Err(wrong_kind_of_track());
    }
    insert_clip(target, moved)
}

/// Moves `clip` and the clips linked to it by `by`. With `to`, `clip`
/// itself lands on that track (of its own kind); the linked ones stay on theirs.
pub(crate) fn move_linked(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    by: Time,
    to: Option<TrackId>,
) -> diag::Result<()> {
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
        moved.push((track.id, track.kind, ours));
    }
    for (track, kind, clips) in moved {
        for mut c in clips {
            c.start = c.start + by;
            if c.start.is_negative() {
                return Err(
                    AppError::new("The clip can't move there").why("That is before the timeline starts.")
                );
            }
            let target = track_mut(seq, to.filter(|_| c.id == clip).unwrap_or(track))?;
            if target.kind != kind {
                return Err(wrong_kind_of_track());
            }
            insert_clip(target, c)?;
        }
    }
    Ok(())
}

pub(crate) fn wrong_kind_of_track() -> AppError {
    AppError::new("The clip can't move to that track")
        .why("Video clips go on video tracks and audio clips on audio tracks.")
}

pub(crate) fn trim(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    edge: Edge,
    to: Time,
) -> diag::Result<()> {
    let lengths: Vec<(ClipId, Option<Time>)> = sequence_ref(p, sequence)?
        .tracks()
        .flat_map(|t| t.clips.iter())
        .map(|c| (c.id, p.source_length(c)))
        .collect();
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
                    t = c.start_cut_by(delta);
                    t.start = c.start + delta;
                    t.duration = c.duration - delta;
                }
                Edge::End => t.duration = c.duration + delta,
            }
            if t.duration <= Time::ZERO {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("It would have no length left.")
                    .fix("Delete the clip instead."));
            }
            if t.starts_before_its_source() || t.start.is_negative() {
                return Err(AppError::new("The clip can't be trimmed that far")
                    .why("Its file has nothing earlier to show."));
            }
            let length = lengths.iter().find(|(id, _)| *id == c.id).and_then(|(_, l)| *l);
            if let Some(length) = length
                && t.source_in + t.source_len() > length
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

pub(crate) fn import(p: &mut Project, sequence: SequenceId, media: &MediaRef) -> diag::Result<()> {
    settle_automatic(p, sequence, media)?;
    let id: MediaId = p.allocate_id();
    p.media.insert(id, MediaRef { id, ..media.clone() });
    place_media(p, sequence, id, None, None)
}

/// An automatic timeline takes its settings from the first video it gets.
pub(crate) fn settle_automatic(p: &mut Project, sequence: SequenceId, media: &MediaRef) -> diag::Result<()> {
    if let Some(v) = &media.info.video
        && media.kind == MediaKind::Video
    {
        let seq = sequence_mut(p, sequence)?;
        if seq.automatic {
            seq.settings = SequenceSettings::matching(v, &media.info.audio);
            seq.automatic = false;
        }
    }
    Ok(())
}

/// How long a file's clips are when placed.
pub(crate) fn placed_length(media: &MediaRef) -> Time {
    match media.kind {
        MediaKind::Image => STILL_LENGTH,
        MediaKind::Video | MediaKind::Audio => media.duration,
    }
}

/// How long a file's clips are when placed, or why it can't be placed: its
/// length is not known, or it has neither picture nor sound.
pub(crate) fn placeable_length(media: &MediaRef) -> diag::Result<Time> {
    let length = placed_length(media);
    if length <= Time::ZERO {
        return Err(AppError::new("This file can't be placed")
            .why("Its length could not be read, so the app doesn't know how long the clip is.")
            .fix("Try re-saving the file with a recording or conversion tool, then import it again."));
    }
    let (video, audio) = placed_streams(media);
    if video.is_none() && audio.is_empty() {
        return Err(
            AppError::new("This file has nothing to place").why("No picture or sound was found in it.")
        );
    }
    Ok(length)
}

/// What of a file is placed: its picture stream (none for a sound file) and
/// its sound streams (none for a still).
pub(crate) fn placed_streams(
    media: &MediaRef,
) -> (Option<&crate::model::VideoStream>, &[crate::model::AudioStream]) {
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
    (video, audio)
}

/// The video track a file's picture goes on: `chosen`, or the first one
/// (`None` when the timeline has no video track yet: one is made).
pub(crate) fn picture_track(seq: &Sequence, chosen: Option<TrackId>) -> diag::Result<Option<TrackId>> {
    match chosen {
        Some(id) => match seq.track(id) {
            Some(track) if track.kind == TrackKind::Video => Ok(Some(id)),
            Some(track) => Err(AppError::new("The picture can't go on that track")
                .why(format!("{} is an audio track.", track.name))
                .fix("Drop the file on a video track.")),
            None => Err(missing("track")),
        },
        None => Ok(seq.video_tracks.front().map(|t| t.id)),
    }
}

/// Places a file of the project at `at` on the track under the pointer:
/// pushing everything later, or over what is on the tracks it lands on.
pub(crate) fn drop_media(
    p: &mut Project,
    sequence: SequenceId,
    id: MediaId,
    at: Time,
    track: Option<TrackId>,
    insert: bool,
) -> diag::Result<()> {
    if at.is_negative() {
        return Err(
            AppError::new("The file can't be placed there").why("That is before the timeline starts.")
        );
    }
    let media = p.media.get(&id).cloned().ok_or_else(|| missing("media file"))?;
    let length = placed_length(&media);
    let seq = sequence_ref(p, sequence)?;
    let track = picture_track(seq, track)?;
    if insert {
        crate::edits::open_space(p, sequence, at, length)?;
    } else {
        // The tracks it lands on: its picture's, and one audio track per
        // sound stream, from the first.
        let (video, audio) = placed_streams(&media);
        let landing: Vec<TrackId> = track
            .filter(|_| video.is_some())
            .into_iter()
            .chain(seq.audio_tracks.iter().take(audio.len()).map(|t| t.id))
            .collect();
        crate::edits::clear_space_on(p, sequence, at, length, &landing)?;
    }
    place_media(p, sequence, id, Some(at), track)
}

/// Places a file of the project at `at` (the end of the timeline when
/// `None`): its picture on the video track `picture_on` (the first when
/// `None`) and each sound stream on its own audio track (made when missing,
/// named after the stream), all linked.
pub(crate) fn place_media(
    p: &mut Project,
    sequence: SequenceId,
    id: MediaId,
    at: Option<Time>,
    picture_on: Option<TrackId>,
) -> diag::Result<()> {
    let media = p.media.get(&id).cloned().ok_or_else(|| missing("media file"))?;
    let media = &media;
    let length = placeable_length(media)?;
    let (video, audio) = placed_streams(media);
    let link: Option<LinkId> = (usize::from(video.is_some()) + audio.len() > 1).then(|| p.allocate_id());
    let at = match at {
        Some(at) => at,
        None => sequence_ref(p, sequence)?.length(),
    };
    let clip = |p: &mut Project, stream: u32| Clip {
        lines: Default::default(),
        effects: Default::default(),
        mode: Default::default(),
        transition: None,
        title: None,
        grade: Default::default(),
        fades: crate::fades::PictureFades::NONE,
        id: p.allocate_id(),
        media: id,
        stream,
        start: at,
        source_in: Time::ZERO,
        duration: length,
        link,
        speed: Speed::NORMAL,
        look: Look::PLAIN,
        sound: Default::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
        kind: Default::default(),
    };

    if let Some(v) = video {
        let c = clip(p, v.index);
        if sequence_ref(p, sequence)?.video_tracks.is_empty() {
            let track = p.new_track(TrackKind::Video, "V1");
            sequence_mut(p, sequence)?.video_tracks.push_back(track);
        }
        let seq = sequence_mut(p, sequence)?;
        let track = match picture_track(seq, picture_on)? {
            Some(id) => track_mut(seq, id)?,
            None => return Err(missing("track")),
        };
        insert_clip(track, c)?;
    }
    for (i, stream) in audio.iter().enumerate() {
        let c = clip(p, stream.index);
        if sequence_ref(p, sequence)?.audio_tracks.len() <= i {
            let name = crate::tracks::name_for_stream(sequence_ref(p, sequence)?, &stream.name);
            let track = p.new_track(TrackKind::Audio, &name);
            sequence_mut(p, sequence)?.audio_tracks.push_back(track);
        }
        let track = &mut sequence_mut(p, sequence)?.audio_tracks[i];
        // A track nobody has used yet takes the stream's name.
        if track.clips.is_empty()
            && track.name == format!("A{}", i + 1)
            && crate::tracks::titled(&stream.name)
        {
            track.name = stream.name.clone();
        }
        insert_clip(track, c)?;
    }
    Ok(())
}

/// Removes `clips` (refused when there are none) and every clip linked to
/// them. Each is looked up before
/// anything changes, so a missing one leaves the project as it was. With
/// `ripple`, each track closes its gaps from the last one back, which is
/// what removing the clips one at a time would do.
pub(crate) fn remove_clips(
    p: &mut Project,
    sequence: SequenceId,
    clips: &[ClipId],
    ripple: bool,
) -> diag::Result<()> {
    if clips.is_empty() {
        return Err(AppError::new("No clips to remove")
            .why("None were selected.")
            .fix("Click a clip on the timeline first."));
    }
    let seq = sequence_mut(p, sequence)?;
    let targets = with_all_linked(seq, clips)?;
    for track in tracks_mut(seq) {
        let removed: Vec<Clip> = track
            .clips
            .iter()
            .filter(|c| targets.contains(&c.id))
            .cloned()
            .collect();
        track.clips.retain(|c| !targets.contains(&c.id));
        if ripple {
            for gone in removed.iter().rev() {
                close_gap(track, gone.start, gone.duration);
            }
        }
    }
    Ok(())
}

/// `clips` and every clip linked to them; refused when one no longer exists.
/// Two passes over the timeline whatever the number of clips: a selection of
/// thousands costs no more per clip than one.
fn with_all_linked(seq: &Sequence, clips: &[ClipId]) -> diag::Result<HashSet<ClipId>> {
    let asked: HashSet<ClipId> = clips.iter().copied().collect();
    let mut found = 0;
    let mut links: HashSet<LinkId> = HashSet::new();
    for c in seq.tracks().flat_map(|t| t.clips.iter()) {
        if asked.contains(&c.id) {
            found += 1;
            links.extend(c.link);
        }
    }
    if found < asked.len() {
        return Err(missing("clip"));
    }
    Ok(seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| asked.contains(&c.id) || c.link.is_some_and(|l| links.contains(&l)))
        .map(|c| c.id)
        .collect())
}

/// See [`crate::Command::RippleRemoveClips`]: the gaps close on every track, from
/// the last back, as a ripple trim moves everything after its clip.
pub(crate) fn ripple_remove_clips(
    p: &mut Project,
    sequence: SequenceId,
    clips: &[ClipId],
) -> diag::Result<()> {
    if clips.is_empty() {
        return Err(AppError::new("No clips to remove")
            .why("None were selected.")
            .fix("Click a clip on the timeline first."));
    }
    let seq = sequence_ref(p, sequence)?;
    let targets = with_all_linked(seq, clips)?;
    let mut covered: Vec<(Time, Time)> = seq
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter(|c| targets.contains(&c.id))
        .map(|c| (c.start, c.end()))
        .collect();
    covered.sort();
    let mut stretches: Vec<(Time, Time)> = Vec::new();
    for (start, end) in covered {
        match stretches.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => stretches.push((start, end)),
        }
    }
    close_stretches(p, sequence, &stretches, &targets)
}

/// Takes `going` out and closes `stretches` (in time order, apart) on
/// every track, from the last back: every clip after one moves back by its
/// length. Refused, with nothing changed, when a clip that stays is in one.
fn close_stretches(
    p: &mut Project,
    sequence: SequenceId,
    stretches: &[(Time, Time)],
    going: &HashSet<ClipId>,
) -> diag::Result<()> {
    let seq = sequence_ref(p, sequence)?;
    // The first stretch a clip that stays is in, by a search: stretches are
    // in time order and apart. Named as a walk stretch by stretch, then
    // track by track, would find it.
    let mut first_in_the_way: Option<((usize, usize, usize), &Track, &Clip)> = None;
    for (ti, t) in seq.tracks().enumerate() {
        for (ci, c) in t.clips.iter().enumerate() {
            if going.contains(&c.id) {
                continue;
            }
            let k = stretches.partition_point(|&(_, end)| end <= c.start);
            if stretches.get(k).is_some_and(|&(start, _)| start < c.end())
                && first_in_the_way
                    .as_ref()
                    .is_none_or(|(at, _, _)| (k, ti, ci) < *at)
            {
                first_in_the_way = Some(((k, ti, ci), t, c));
            }
        }
    }
    {
        let staying = first_in_the_way.map(|(_, t, c)| (t.name.clone(), p.clip_name(c)));
        if let Some((track, clip)) = staying {
            return Err(AppError::new("The gap can't be closed")
                .why(format!(
                    "{clip} on {track} is in the gap. Closing the gap moves everything after it on every track, so {clip} would be cut."
                ))
                .fix(format!(
                    "Remove the clip and leave the gap instead, or cut {clip} at the gap's edges (the Blade tool) and remove that part as well."
                )));
        }
    }
    // Each clip moves back by the stretches that end before it starts,
    // added up: what closing them one at a time, from the last back, does.
    let mut closed = vec![Time::ZERO];
    for &(start, end) in stretches {
        let before = closed[closed.len() - 1];
        closed.push(before + (end - start));
    }
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        if !going.is_empty() {
            track.clips.retain(|c| !going.contains(&c.id));
        }
        for clip in track.clips.iter_mut() {
            let k = stretches.partition_point(|&(_, end)| end <= clip.start);
            if k > 0 {
                clip.start = clip.start - closed[k];
            }
        }
    }
    Ok(())
}

/// See [`crate::Command::MoveClips`].
pub(crate) fn move_clips(
    p: &mut Project,
    sequence: SequenceId,
    clips: &[ClipId],
    by: Time,
) -> diag::Result<()> {
    if clips.is_empty() {
        return Err(AppError::new("No clips to move").why("None were selected."));
    }
    let seq = sequence_mut(p, sequence)?;
    let group = with_all_linked(seq, clips)?;
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
        let track = track_mut(seq, track)?;
        // The moved clips keep their spacing, so they can only land on the
        // clips that stay: each is checked against its neighbours there by
        // a search, then the two lists, both in start order, are merged.
        let mut landed = Vec::with_capacity(clips.len());
        for mut c in clips {
            c.start = c.start + by;
            if c.start.is_negative() {
                return Err(
                    AppError::new("The clips can't move there").why("One would start before the timeline.")
                );
            }
            if overlaps_any(&track.clips, &c) {
                return Err(overlap_error(track));
            }
            landed.push(c);
        }
        let mut merged = imbl::Vector::new();
        let mut staying = std::mem::take(&mut track.clips).into_iter().peekable();
        for c in landed {
            while let Some(s) = staying.next_if(|s| s.start <= c.start) {
                merged.push_back(s);
            }
            merged.push_back(c);
        }
        merged.extend(staying);
        track.clips = merged;
    }
    Ok(())
}

/// Whether `clip` overlaps one of `clips` (in start order, apart): only the
/// clip starting last before its end can.
fn overlaps_any(clips: &imbl::Vector<Clip>, clip: &Clip) -> bool {
    let after = clips.binary_search_by(|c| {
        if c.start < clip.end() {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let after = after.unwrap_or_else(|i| i);
    after > 0 && clips.get(after - 1).is_some_and(|c| clip.start < c.end())
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

/// Moves every clip that starts at or after `from` back by `by`.
fn close_gap(track: &mut Track, from: Time, by: Time) {
    for clip in track.clips.iter_mut() {
        if clip.start >= from {
            clip.start = clip.start - by;
        }
    }
}

#[cfg(test)]
mod tests;
