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
use crate::ramp::Ramp;
use crate::time::Rate;
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
                    t = c.start_cut_by(delta);
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
            // A ramp gives way to the one speed.
            changed.push(Clip {
                duration: speed.unscale(c.source_len()),
                speed,
                ramp: Ramp::default(),
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

/// See [`crate::Command::SetRamp`].
pub(crate) fn set_ramp(p: &mut Project, sequence: SequenceId, clip: ClipId, ramp: &Ramp) -> diag::Result<()> {
    if let Some(problem) = ramp.problems("The ramp").into_iter().next() {
        return Err(AppError::new("That speed ramp can't be used").why(format!("{problem}.")));
    }
    let seq = sequence_ref(p, sequence)?;
    let rate = seq.settings.frame_rate;
    let reference = find(seq, clip)?;
    let group = with_linked(seq, clip)?;
    let file_end = |c: &Clip| {
        p.media
            .get(&c.media)
            .filter(|m| m.kind != MediaKind::Image && m.duration > Time::ZERO)
            .map(|m| m.duration)
    };
    let new_end = ramped(&reference, ramp.clone(), rate, file_end(&reference)).end();
    let shift = new_end - reference.end();
    let after = reference.end();
    let mut changed = Vec::new();
    for c in seq.tracks().flat_map(|t| t.clips.iter()) {
        if group.contains(&c.id) {
            // Linked clips get the same ramp at the same place on the timeline.
            let own = ramp.shifted(reference.start - c.start);
            changed.push(ramped(c, own, rate, file_end(c)));
        } else if c.start >= after && shift != Time::ZERO {
            changed.push(Clip {
                start: c.start + shift,
                ..c.clone()
            });
        }
    }
    put_back(p, sequence, changed)
}

/// `c` with `ramp`, playing the same part of its file from the same start:
/// its length is what the ramp needs for that, brought to the nearest
/// timeline frame (at `rate`) and never past `file_end`. An empty ramp
/// leaves it at its one speed, exactly.
fn ramped(c: &Clip, ramp: Ramp, rate: Rate, file_end: Option<Time>) -> Clip {
    let end_in_file = c.source_in + c.source_len();
    let mut new = Clip { ramp, ..c.clone() };
    if new.ramp.is_empty() {
        new.duration = c.speed.unscale(c.source_len());
        return new;
    }
    let reached = |frame: i64| new.source_at(rate.frame_start(frame));
    // The first frame boundary after the start, then the first where the
    // file is played to its end (speeds are never below 1/100, so the
    // doubling finds it within a few steps).
    let first = rate.frame_at(c.start) + 1;
    let (mut low, mut high, mut step) = (first, first, 1i64);
    for _ in 0..62 {
        if reached(high) >= end_in_file {
            break;
        }
        low = high;
        high = high.saturating_add(step);
        step = step.saturating_mul(2);
    }
    while low < high {
        let middle = low + (high - low) / 2;
        if reached(middle) >= end_in_file {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let past = high;
    let before = past - 1;
    let nearer = if before >= first && end_in_file - reached(before) <= reached(past) - end_in_file {
        before
    } else {
        past
    };
    let beyond_file = file_end.is_some_and(|end| reached(nearer) > end);
    let end = if beyond_file && before >= first {
        before
    } else {
        nearer
    };
    new.duration = rate.frame_start(end) - c.start;
    new
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
                duration: next.duration - delta,
                ..next.start_cut_by(delta)
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
        // A ramp stays where it is in the clip; the picture slips under it.
        changed.push(Clip {
            source_in: c.source_in + c.source_offset(by),
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
                duration: next.duration - by,
                ..next.start_cut_by(by)
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

/// Empties `at`..`at + length` on `tracks` only, leaving the gap; every
/// other track keeps what it has. A clip that is cut is cut together with
/// the clips linked to it, wherever they are, so linked clips stay the
/// same length as each other.
pub(crate) fn clear_space_on(
    p: &mut Project,
    sequence: SequenceId,
    at: Time,
    length: Time,
    tracks: &[TrackId],
) -> diag::Result<()> {
    let end = at + length;
    cut_edges_on(p, sequence, at, end, tracks)?;
    let seq = sequence_mut(p, sequence)?;
    for track in tracks_mut(seq).filter(|t| tracks.contains(&t.id)) {
        track.clips.retain(|c| c.end() <= at || c.start >= end);
    }
    Ok(())
}

/// Cuts the clips of `tracks` that cross `at` or `end` there, each together
/// with the clips linked to it, wherever they are. Afterwards every clip of
/// `tracks` lies inside `at`..`end` or outside it.
pub(crate) fn cut_edges_on(
    p: &mut Project,
    sequence: SequenceId,
    at: Time,
    end: Time,
    tracks: &[TrackId],
) -> diag::Result<()> {
    for cut in [end, at] {
        // One clip at a time: cutting it may cut a linked clip on another
        // of these tracks too.
        loop {
            let crossing = sequence_ref(p, sequence)?
                .tracks()
                .filter(|t| tracks.contains(&t.id))
                .flat_map(|t| t.clips.iter())
                .find(|c| c.start < cut && cut < c.end())
                .map(|c| c.id);
            let Some(crossing) = crossing else { break };
            split(p, sequence, cut, Some(crossing))?;
        }
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
