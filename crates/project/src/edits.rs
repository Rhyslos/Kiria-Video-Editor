//! The trim tools beyond a plain trim (ripple, roll, slip and slide), and
//! cutting stretches out of the whole timeline.
//!
//! Each works out the new place of every clip it touches, checks that each
//! still fits inside its file, then puts them all back at once, so a refused
//! edit changes nothing.

use crate::command::{
    Edge, insert_clip, missing, sequence_mut, sequence_ref, split, tracks_mut, with_linked,
};
use crate::ids::{ClipId, SequenceId, TrackId};
use crate::model::{Clip, MediaKind, Project, Region, Sequence};
use crate::time::{Speed, Time};
use diag::AppError;

pub(crate) fn ripple_trim(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    edge: Edge,
    to: Time,
) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let reference = find(seq, clip)?;
    let group = with_linked(seq, clip)?;
    let delta = match edge {
        Edge::Start => to - reference.start,
        Edge::End => to - reference.end(),
    };
    // Everything after the clip follows its end.
    let shift = match edge {
        Edge::Start => Time::ZERO - delta,
        Edge::End => delta,
    };
    let after = reference.end();
    let mut changed = Vec::new();
    for c in seq.tracks().flat_map(|t| t.clips.iter()) {
        let mut t = c.clone();
        if group.contains(&c.id) {
            match edge {
                Edge::Start => {
                    t.source_in = c.source_in + c.speed.scale(delta);
                    t.duration = c.duration - delta;
                }
                Edge::End => t.duration = c.duration + delta,
            }
        } else if c.start >= after {
            t.start = c.start + shift;
        } else {
            continue;
        }
        changed.push(t);
    }
    put_back(p, sequence, changed)
}

pub(crate) fn set_speed(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    speed: Speed,
) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let reference = find(seq, clip)?;
    let group = with_linked(seq, clip)?;
    let new_end = reference.start + speed.unscale(reference.source_len());
    let shift = new_end - reference.end();
    let after = reference.end();
    let mut changed = Vec::new();
    for c in seq.tracks().flat_map(|t| t.clips.iter()) {
        if group.contains(&c.id) {
            changed.push(Clip {
                duration: speed.unscale(c.source_len()),
                speed,
                ..c.clone()
            });
        } else if c.start >= after {
            changed.push(Clip {
                start: c.start + shift,
                ..c.clone()
            });
        }
    }
    put_back(p, sequence, changed)
}

pub(crate) fn roll(p: &mut Project, sequence: SequenceId, clip: ClipId, to: Time) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let reference = find(seq, clip)?;
    if next_to(seq, &reference).is_none() {
        return Err(AppError::new("There is no cut to roll there")
            .why("Rolling moves the cut between two clips that touch; this clip has nothing right after it.")
            .fix("Trim the clip instead, or close the gap first."));
    }
    let delta = to - reference.end();
    let mut changed = Vec::new();
    for id in with_linked(seq, clip)? {
        let c = find(seq, id)?;
        if let Some(next) = next_to(seq, &c) {
            changed.push(Clip {
                start: next.start + delta,
                source_in: next.source_in + next.speed.scale(delta),
                duration: next.duration - delta,
                ..next
            });
        }
        changed.push(Clip {
            duration: c.duration + delta,
            ..c
        });
    }
    put_back(p, sequence, changed)
}

pub(crate) fn slip(p: &mut Project, sequence: SequenceId, clip: ClipId, by: Time) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let mut changed = Vec::new();
    for id in with_linked(seq, clip)? {
        let c = find(seq, id)?;
        changed.push(Clip {
            source_in: c.source_in + c.speed.scale(by),
            ..c
        });
    }
    put_back(p, sequence, changed)
}

pub(crate) fn slide(p: &mut Project, sequence: SequenceId, clip: ClipId, by: Time) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let mut changed = Vec::new();
    for id in with_linked(seq, clip)? {
        let c = find(seq, id)?;
        if let Some(before) = before(seq, &c) {
            changed.push(Clip {
                duration: before.duration + by,
                ..before
            });
        }
        if let Some(next) = next_to(seq, &c) {
            changed.push(Clip {
                start: next.start + by,
                source_in: next.source_in + next.speed.scale(by),
                duration: next.duration - by,
                ..next
            });
        }
        changed.push(Clip {
            start: c.start + by,
            ..c
        });
    }
    put_back(p, sequence, changed)
}

/// Makes `length` of room at `at` on every track: clips under `at` are
/// split and everything from `at` on moves later. Labels move too.
pub(crate) fn open_space(p: &mut Project, sequence: SequenceId, at: Time, length: Time) -> diag::Result<()> {
    split_where_crossing(p, sequence, at)?;
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        for c in track.clips.iter_mut().filter(|c| c.start >= at) {
            c.start = c.start + length;
        }
    }
    for label in seq.labels.iter_mut().filter(|l| l.at >= at) {
        label.at = label.at + length;
    }
    Ok(())
}

/// Empties `at`..`at + length` on every track, leaving the gap.
pub(crate) fn clear_space(p: &mut Project, sequence: SequenceId, at: Time, length: Time) -> diag::Result<()> {
    let end = at + length;
    split_where_crossing(p, sequence, end)?;
    split_where_crossing(p, sequence, at)?;
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq) {
        track.clips.retain(|c| c.end() <= at || c.start >= end);
    }
    Ok(())
}

/// Splits every clip that crosses `at`, if any does.
fn split_where_crossing(p: &mut Project, sequence: SequenceId, at: Time) -> diag::Result<()> {
    let crosses = sequence_ref(p, sequence)?
        .tracks()
        .flat_map(|t| t.clips.iter())
        .any(|c| c.start < at && at < c.end());
    if crosses {
        split(p, sequence, at, None)?;
    }
    Ok(())
}

pub(crate) fn remove_ranges(p: &mut Project, sequence: SequenceId, ranges: &[Region]) -> diag::Result<()> {
    if ranges.is_empty() {
        return Err(AppError::new("Nothing to remove").why("No stretches were given."));
    }
    // Merged and from last to first, so earlier stretches keep their times.
    let mut sorted: Vec<Region> = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);
    let mut merged: Vec<Region> = Vec::new();
    for r in sorted {
        if r.start.is_negative() || r.end <= r.start {
            return Err(AppError::new("That stretch can't be removed").why("It ends before it starts."));
        }
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    for r in merged.iter().rev() {
        split_where_crossing(p, sequence, r.end)?;
        split_where_crossing(p, sequence, r.start)?;
        let length = r.length();
        let seq = sequence_mut(p, sequence)?;
        for track in tracks_mut(seq) {
            track.clips.retain(|c| c.end() <= r.start || c.start >= r.end);
            for c in track.clips.iter_mut().filter(|c| c.start >= r.end) {
                c.start = c.start - length;
            }
        }
        for label in seq.labels.iter_mut().filter(|l| l.at > r.start) {
            label.at = (label.at - length).max(r.start);
        }
    }
    Ok(())
}

fn find(seq: &Sequence, clip: ClipId) -> diag::Result<Clip> {
    seq.tracks()
        .flat_map(|t| t.clips.iter())
        .find(|c| c.id == clip)
        .cloned()
        .ok_or_else(|| missing("clip"))
}

/// The track holding `clip`.
fn track_of(seq: &Sequence, clip: ClipId) -> Option<TrackId> {
    seq.tracks()
        .find(|t| t.clips.iter().any(|c| c.id == clip))
        .map(|t| t.id)
}

/// The clip on the same track that starts exactly where `c` ends.
fn next_to(seq: &Sequence, c: &Clip) -> Option<Clip> {
    let track = track_of(seq, c.id)?;
    seq.tracks()
        .find(|t| t.id == track)?
        .clips
        .iter()
        .find(|n| n.start == c.end() && n.id != c.id)
        .cloned()
}

/// The clip on the same track that ends exactly where `c` starts.
fn before(seq: &Sequence, c: &Clip) -> Option<Clip> {
    let track = track_of(seq, c.id)?;
    seq.tracks()
        .find(|t| t.id == track)?
        .clips
        .iter()
        .find(|b| b.end() == c.start && b.id != c.id)
        .cloned()
}

/// Checks every changed clip, then swaps them in on their own tracks.
fn put_back(p: &mut Project, sequence: SequenceId, changed: Vec<Clip>) -> diag::Result<()> {
    for c in &changed {
        fits(p, c)?;
    }
    let seq = sequence_mut(p, sequence)?;
    let mut placed: Vec<(TrackId, Clip)> = Vec::new();
    for c in changed {
        let track = track_of(seq, c.id).ok_or_else(|| missing("clip"))?;
        placed.push((track, c));
    }
    for track in tracks_mut(seq) {
        track.clips.retain(|c| !placed.iter().any(|(_, n)| n.id == c.id));
    }
    for (track, c) in placed {
        let target = tracks_mut(seq)
            .find(|t| t.id == track)
            .ok_or_else(|| missing("track"))?;
        insert_clip(target, c)?;
    }
    Ok(())
}

/// A clip must keep some length, start on the timeline, and stay inside
/// its file (stills can be any length).
fn fits(p: &Project, c: &Clip) -> diag::Result<()> {
    let refuse = |why: &str| {
        Err(AppError::new("The edit goes too far")
            .why(why.to_owned())
            .fix("Drag a shorter distance."))
    };
    if c.duration <= Time::ZERO {
        return refuse("A clip would have no length left.");
    }
    if c.start.is_negative() {
        return refuse("A clip would start before the timeline.");
    }
    if c.source_in.is_negative() {
        return refuse("A clip's file has nothing earlier to show.");
    }
    if let Some(media) = p.media.get(&c.media)
        && media.kind != MediaKind::Image
        && media.duration > Time::ZERO
        && c.source_in + c.source_len() > media.duration
    {
        return refuse("A clip's file ends before that.");
    }
    Ok(())
}
