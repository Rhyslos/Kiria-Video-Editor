//! Several timelines in one project, timelines nested in others, and
//! adjustment layers: making, copying, renaming and removing a timeline,
//! moving clips into a timeline of their own ("nest"), placing a timeline in
//! another, and laying an adjustment layer over a stretch.
//!
//! Which timeline is open is not an edit: see [`crate::History::open`].

use crate::command::{insert_clip, missing, sequence_mut, sequence_ref, with_linked};
use crate::ids::{ClipId, LinkId, MediaId, SequenceId, TrackId};
use crate::look::Look;
use crate::model::{Clip, ClipKind, Project, Sequence, SequenceSettings, Track, TrackKind};
use crate::time::{Speed, Time};
use diag::AppError;
use std::collections::{HashMap, HashSet};

/// The longest name a timeline can be given, in characters.
pub const LONGEST_NAME: usize = 40;

/// How long an adjustment layer is when it is made.
pub const ADJUSTMENT_LENGTH: Time = Time::seconds(5);

/// What an adjustment layer is called on the timeline.
pub const ADJUSTMENT_NAME: &str = "Adjustment layer";

impl Project {
    /// A name for a new timeline: `base` when no timeline has it, else
    /// `base` with the first free number after it ("Timeline 2").
    pub fn free_sequence_name(&self, base: &str) -> String {
        if !self.has_sequence_named(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !self.has_sequence_named(name))
            .unwrap_or_else(|| base.to_owned())
    }

    fn has_sequence_named(&self, name: &str) -> bool {
        self.sequences.iter().any(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Whether `outer` plays `inner`: it is `inner`, or a clip on it (or on
    /// a timeline nested in it, and so on) plays `inner`.
    pub fn plays(&self, outer: SequenceId, inner: SequenceId) -> bool {
        let mut seen = HashSet::new();
        let mut to_visit = vec![outer];
        while let Some(id) = to_visit.pop() {
            if id == inner {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            if let Some(sequence) = self.sequence(id) {
                to_visit.extend(nested_in(sequence));
            }
        }
        false
    }

    /// The timelines whose clips play `id`, by name.
    pub fn sequences_playing(&self, id: SequenceId) -> Vec<String> {
        self.sequences
            .iter()
            .filter(|s| s.id != id && nested_in(s).any(|n| n == id))
            .map(|s| s.name.clone())
            .collect()
    }
}

/// The timelines the clips of `sequence` play, directly.
fn nested_in(sequence: &Sequence) -> impl Iterator<Item = SequenceId> + '_ {
    sequence
        .tracks()
        .flat_map(|t| t.clips.iter())
        .filter_map(Clip::nested)
}

/// The name of a timeline that plays itself through the timelines nested
/// in it, if any does: a loop that could never be played.
pub(crate) fn in_a_loop(p: &Project) -> Option<String> {
    p.sequences
        .iter()
        .find(|s| nested_in(s).any(|inner| p.plays(inner, s.id)))
        .map(|s| s.name.clone())
}

/// A timeline's name as it is kept: without spaces around it. Refused when
/// nothing is left, it is too long, or another timeline (other than
/// `except`) has it, whatever its capitals.
fn tidy(p: &Project, name: &str, except: Option<SequenceId>) -> diag::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::new("A timeline needs a name").fix("Type at least one letter."));
    }
    if name.chars().count() > LONGEST_NAME {
        return Err(AppError::new("That timeline name is too long")
            .why(format!(
                "A timeline's name can be {LONGEST_NAME} characters at most."
            ))
            .fix("Type a shorter name."));
    }
    if p.sequences
        .iter()
        .any(|s| Some(s.id) != except && s.name.eq_ignore_ascii_case(name))
    {
        return Err(AppError::new(format!("There is already a timeline called {name}"))
            .why("Each timeline of a project has a name of its own, so the timeline picker can tell them apart.")
            .fix("Type another name."));
    }
    Ok(name.to_owned())
}

/// See [`crate::Command::AddSequence`].
pub(crate) fn add(p: &mut Project, name: &str, settings: SequenceSettings) -> diag::Result<()> {
    check_settings(&settings)?;
    let name = tidy(p, name, None)?;
    let sequence = p.new_sequence(&name, settings);
    p.active_sequence = sequence.id;
    p.sequences.push_back(sequence);
    Ok(())
}

pub(crate) fn check_settings(settings: &SequenceSettings) -> diag::Result<()> {
    if settings.width == 0 || settings.height == 0 || settings.sample_rate == 0 {
        return Err(AppError::new("Those timeline settings can't be used")
            .why("The picture size and sound rate must be above zero."));
    }
    Ok(())
}

/// See [`crate::Command::RenameSequence`].
pub(crate) fn rename(p: &mut Project, sequence: SequenceId, name: &str) -> diag::Result<()> {
    let name = tidy(p, name, Some(sequence))?;
    sequence_mut(p, sequence)?.name = name;
    Ok(())
}

/// See [`crate::Command::DuplicateSequence`].
pub(crate) fn duplicate(p: &mut Project, sequence: SequenceId, name: &str) -> diag::Result<()> {
    let name = tidy(p, name, None)?;
    let original = sequence_ref(p, sequence)?.clone();
    let mut copy = original.clone();
    copy.id = p.allocate_id();
    copy.name = name;
    // Every track, clip, link and label gets an id of its own; clips linked
    // in the original are linked in the copy.
    let mut links: HashMap<LinkId, LinkId> = HashMap::new();
    for track in copy.video_tracks.iter_mut().chain(copy.audio_tracks.iter_mut()) {
        track.id = p.allocate_id();
        for clip in track.clips.iter_mut() {
            clip.id = p.allocate_id();
            if let Some(link) = clip.link {
                let new = match links.get(&link) {
                    Some(new) => *new,
                    None => {
                        let new: LinkId = p.allocate_id();
                        links.insert(link, new);
                        new
                    }
                };
                clip.link = Some(new);
            }
        }
    }
    for label in copy.labels.iter_mut() {
        label.id = p.allocate_id();
    }
    p.active_sequence = copy.id;
    p.sequences.push_back(copy);
    Ok(())
}

/// See [`crate::Command::RemoveSequence`].
pub(crate) fn remove(p: &mut Project, sequence: SequenceId) -> diag::Result<()> {
    let name = sequence_ref(p, sequence)?.name.clone();
    if p.sequences.len() <= 1 {
        return Err(AppError::new(format!("{name} is the only timeline"))
            .why("A project keeps at least one timeline.")
            .fix("Make another timeline first if this one should go."));
    }
    let users = p.sequences_playing(sequence);
    if !users.is_empty() {
        return Err(AppError::new(format!("{name} is used in another timeline"))
            .why(format!(
                "Clips in {} play it; removing it would leave them with nothing to play.",
                users.join(", ")
            ))
            .fix("Delete those clips first, then remove the timeline."));
    }
    p.sequences.retain(|s| s.id != sequence);
    if p.active_sequence == sequence
        && let Some(first) = p.sequences.front()
    {
        p.active_sequence = first.id;
    }
    Ok(())
}

/// See [`crate::Command::NestClips`].
pub(crate) fn nest(p: &mut Project, sequence: SequenceId, clips: &[ClipId], name: &str) -> diag::Result<()> {
    let name = tidy(p, name, None)?;
    let outer = sequence_ref(p, sequence)?;
    if clips.is_empty() {
        return Err(
            AppError::new("No clips to nest").fix("Select the clips to put in a timeline of their own.")
        );
    }
    let mut group: HashSet<ClipId> = HashSet::new();
    for clip in clips {
        group.extend(with_linked(outer, *clip)?);
    }
    let in_group = |c: &&Clip| group.contains(&c.id);
    let moved = || outer.tracks().flat_map(|t| t.clips.iter()).filter(in_group);
    let start = moved().map(|c| c.start).min().ok_or_else(|| missing("clip"))?;
    let end = moved().map(Clip::end).max().ok_or_else(|| missing("clip"))?;
    // The tracks the clips leave, by kind and place from the first.
    let used = |tracks: &imbl::Vector<Track>| -> Vec<usize> {
        tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.clips.iter().any(|c| group.contains(&c.id)))
            .map(|(i, _)| i)
            .collect()
    };
    let (video_used, audio_used) = (used(&outer.video_tracks), used(&outer.audio_tracks));
    // The nested clips take the place of the lowest track of each kind
    // that clips left; what else lies in the stretch there would be covered.
    let rate = outer.settings.frame_rate;
    let lands = |tracks: &imbl::Vector<Track>, used: &[usize]| -> diag::Result<Option<TrackId>> {
        let Some(track) = used.first().and_then(|i| tracks.get(*i)) else {
            return Ok(None);
        };
        if let Some(other) = track
            .clips
            .iter()
            .find(|c| !group.contains(&c.id) && c.start < end && start < c.end())
        {
            return Err(AppError::new("Those clips can't be nested as they are")
                .why(format!(
                    "{} has a clip that isn't selected between them (at {}), where the nested timeline would go.",
                    track.name,
                    other.start.timecode(rate)
                ))
                .fix("Select that clip too, or nest a stretch without it."));
        }
        Ok(Some(track.id))
    };
    let picture_on = lands(&outer.video_tracks, &video_used)?;
    let sound_on = lands(&outer.audio_tracks, &audio_used)?;
    let settings = outer.settings;
    let (outer_video, outer_audio) = (outer.video_tracks.clone(), outer.audio_tracks.clone());

    // The new timeline: as many tracks of each kind as reach the last one
    // used, with the same names, holding the clips from its start.
    let mut inner = p.new_sequence(&name, settings);
    inner.video_tracks.clear();
    inner.audio_tracks.clear();
    for (tracks, used, kind) in [
        (&outer_video, &video_used, TrackKind::Video),
        (&outer_audio, &audio_used, TrackKind::Audio),
    ] {
        let count = used.last().map_or(1, |last| last + 1);
        for from in tracks.iter().take(count) {
            let mut track = p.new_track(kind, &from.name);
            for clip in from.clips.iter().filter(|c| group.contains(&c.id)) {
                track.clips.push_back(Clip {
                    start: clip.start - start,
                    ..clip.clone()
                });
            }
            match kind {
                TrackKind::Video => inner.video_tracks.push_back(track),
                TrackKind::Audio => inner.audio_tracks.push_back(track),
            }
        }
    }
    let inner_id = inner.id;
    p.sequences.push_back(inner);

    let outer = sequence_mut(p, sequence)?;
    for track in outer.video_tracks.iter_mut().chain(outer.audio_tracks.iter_mut()) {
        track.clips.retain(|c| !group.contains(&c.id));
    }
    lay_nested(p, sequence, inner_id, start, end - start, picture_on, sound_on)
}

/// Puts a nested timeline's clips on `sequence` from `at` for `length`: its
/// picture on the video track `picture_on` and its sound on the audio track
/// `sound_on` (each when given), linked when both are.
fn lay_nested(
    p: &mut Project,
    sequence: SequenceId,
    nested: SequenceId,
    at: Time,
    length: Time,
    picture_on: Option<TrackId>,
    sound_on: Option<TrackId>,
) -> diag::Result<()> {
    let link: Option<LinkId> = (picture_on.is_some() && sound_on.is_some()).then(|| p.allocate_id());
    for track in [picture_on, sound_on].into_iter().flatten() {
        let clip = Clip {
            id: p.allocate_id(),
            link,
            ..unfilled(ClipKind::Timeline(nested), at, length)
        };
        let seq = sequence_mut(p, sequence)?;
        let track = crate::command::track_mut(seq, track)?;
        insert_clip(track, clip)?;
    }
    Ok(())
}

/// A clip of `kind` that plays no file, from `at` for `length`, as it
/// comes: no id or link yet.
fn unfilled(kind: ClipKind, at: Time, length: Time) -> Clip {
    Clip {
        id: ClipId(0),
        media: MediaId::NONE,
        stream: 0,
        start: at,
        source_in: Time::ZERO,
        duration: length,
        link: None,
        speed: Speed::NORMAL,
        look: Look::PLAIN,
        sound: Default::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
        kind,
    }
}

/// Whether nothing on `track` lies anywhere from `at` for `length`.
fn free(track: &Track, at: Time, length: Time) -> bool {
    let end = at + length;
    !track.clips.iter().any(|c| c.start < end && at < c.end())
}

/// See [`crate::Command::PlaceSequence`].
pub(crate) fn place(
    p: &mut Project,
    sequence: SequenceId,
    nested: SequenceId,
    at: Option<Time>,
    track: Option<TrackId>,
) -> diag::Result<()> {
    let inner = sequence_ref(p, nested)?;
    let outer = sequence_ref(p, sequence)?;
    if p.plays(nested, sequence) {
        let why = if nested == sequence {
            format!("{} can't play inside itself.", inner.name)
        } else {
            format!(
                "{} already plays {}, so {} would end up inside itself.",
                inner.name, outer.name, outer.name
            )
        };
        return Err(
            AppError::new(format!("{} can't be placed in {}", inner.name, outer.name))
                .why(why)
                .fix("Open another timeline to place it in."),
        );
    }
    let length = inner.length();
    if length <= Time::ZERO {
        return Err(AppError::new(format!("{} is empty", inner.name))
            .why("A timeline with no clips has nothing to play.")
            .fix("Put some clips on it first."));
    }
    let has = |tracks: &imbl::Vector<Track>| tracks.iter().any(|t| !t.clips.is_empty());
    let (picture, sound) = (has(&inner.video_tracks), has(&inner.audio_tracks));
    let at = at.unwrap_or_else(|| outer.length());
    if at.is_negative() {
        return Err(
            AppError::new("The timeline can't be placed there").why("That is before the timeline starts.")
        );
    }
    let picture_on = match picture {
        true => crate::command::picture_track(outer, track)?,
        false => None,
    };
    if let Some(on) = picture_on.and_then(|id| outer.track(id))
        && !free(on, at, length)
    {
        return Err(AppError::new("There is already a clip there")
            .why(format!(
                "{} would overlap another clip on {}.",
                inner.name, on.name
            ))
            .fix("Drop it on an empty stretch or another track."));
    }
    // The sound takes the first audio track that is free for its length;
    // a track is made when none is.
    let sound_on = match sound {
        false => None,
        true => match outer.audio_tracks.iter().find(|t| free(t, at, length)) {
            Some(track) => Some(track.id),
            None => {
                let name = outer.next_track_name(TrackKind::Audio);
                let track = p.new_track(TrackKind::Audio, &name);
                let id = track.id;
                sequence_mut(p, sequence)?.audio_tracks.push_back(track);
                Some(id)
            }
        },
    };
    lay_nested(p, sequence, nested, at, length, picture_on, sound_on)
}

/// See [`crate::Command::AddAdjustment`].
pub(crate) fn add_adjustment(
    p: &mut Project,
    sequence: SequenceId,
    at: Time,
    length: Time,
    track: Option<TrackId>,
) -> diag::Result<()> {
    if at.is_negative() || length <= Time::ZERO {
        return Err(AppError::new("An adjustment layer can't go there")
            .why("It would start before the timeline or have no length."));
    }
    let seq = sequence_ref(p, sequence)?;
    // From the chosen video track up (from the first when none is chosen),
    // the first that is free for the whole stretch; a new track on top
    // when none is, so the layer is above what it changes.
    let from = match track {
        Some(id) => match seq.track(id) {
            Some(t) if t.kind == TrackKind::Video => {
                seq.video_tracks.iter().position(|v| v.id == id).unwrap_or(0)
            }
            Some(t) => {
                return Err(AppError::new("An adjustment layer goes on a video track")
                    .why(format!(
                        "{} is an audio track; an adjustment layer changes the picture.",
                        t.name
                    ))
                    .fix("Right-click a video track."));
            }
            None => return Err(missing("track")),
        },
        None => 0,
    };
    let found = seq
        .video_tracks
        .iter()
        .skip(from)
        .find(|t| free(t, at, length))
        .map(|t| t.id);
    let on = match found {
        Some(id) => id,
        None => {
            let name = seq.next_track_name(TrackKind::Video);
            let new = p.new_track(TrackKind::Video, &name);
            let id = new.id;
            sequence_mut(p, sequence)?.video_tracks.push_back(new);
            id
        }
    };
    let clip = Clip {
        id: p.allocate_id(),
        ..unfilled(ClipKind::Adjustment, at, length)
    };
    let seq = sequence_mut(p, sequence)?;
    insert_clip(crate::command::track_mut(seq, on)?, clip)
}

#[cfg(test)]
mod tests;
