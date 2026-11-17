//! Transitions between two pictures (format 25): kept on the clip that
//! comes in, and played across its cut with the clip that ends where it
//! starts on the same track, half before the cut and half after. Nothing
//! is trimmed or overlapped for it: where a file has nothing past a clip's
//! edge, its first or last frame is held. A transition whose clip no
//! longer has one just before it waits, unused, until it does again.

use crate::ids::ClipId;
use crate::model::{Clip, Sequence, Track};
use crate::time::Time;
use diag::AppError;
use serde::{Deserialize, Serialize};

/// How one picture gives way to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionKind {
    /// The next picture fades in over the last.
    Dissolve,
    /// The last fades out to black, then the next fades in.
    DipToBlack,
    /// The next picture is uncovered from the left edge across.
    Wipe,
    /// The next picture slides in from the right, pushing the last out.
    Push,
    /// The last grows and fades as the next settles in from smaller.
    Zoom,
}

impl TransitionKind {
    pub const ALL: [TransitionKind; 5] = [
        TransitionKind::Dissolve,
        TransitionKind::DipToBlack,
        TransitionKind::Wipe,
        TransitionKind::Push,
        TransitionKind::Zoom,
    ];

    /// Its name in the interface.
    pub fn name(self) -> &'static str {
        match self {
            TransitionKind::Dissolve => "Cross dissolve",
            TransitionKind::DipToBlack => "Dip to black",
            TransitionKind::Wipe => "Wipe",
            TransitionKind::Push => "Push",
            TransitionKind::Zoom => "Zoom through",
        }
    }
}

/// A transition into a clip from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    /// How long it takes, centred on the cut.
    pub length: Time,
}

impl Transition {
    /// A new one of `kind` at the usual length, half a second.
    pub fn new(kind: TransitionKind) -> Transition {
        Transition {
            kind,
            length: Time::from_i128(1, 2),
        }
    }

    /// Where it starts and ends on the timeline into `to` from `from`:
    /// centred on the cut, never longer than either clip (one trimmed
    /// shorter than its transition plays a shorter one).
    pub fn span(&self, from: &Clip, to: &Clip) -> (Time, Time) {
        let length = self.length.min(from.duration).min(to.duration).max(Time::ZERO);
        let half = Time::from_i128(i128::from(length.num()), 2 * i128::from(length.den()));
        (to.start - half, to.start - half + length)
    }
}

/// The clip of `track` that ends exactly where `clip` starts: the one a
/// transition into `clip` comes from.
pub fn before<'a>(track: &'a Track, clip: &Clip) -> Option<&'a Clip> {
    // A track's clips are in start order and apart: the one before is the
    // last that starts earlier.
    let i = starting_before(track, clip.start);
    let previous = track.clips.get(i.checked_sub(1)?)?;
    (previous.id != clip.id && previous.end() == clip.start).then_some(previous)
}

/// How many clips of `track` start before `at`.
fn starting_before(track: &Track, at: Time) -> usize {
    track
        .clips
        .binary_search_by(|c| match c.start < at {
            true => std::cmp::Ordering::Less,
            false => std::cmp::Ordering::Greater,
        })
        .unwrap_or_else(|i| i)
}

/// Refuses a transition that can't be played: not into a clip with one
/// just before it, of no length, or longer than either clip.
pub fn check(track: &Track, clip: &Clip, transition: &Transition) -> diag::Result<()> {
    let Some(previous) = before(track, clip) else {
        return Err(AppError::new("No transition there")
            .why("A transition plays across a cut, and no clip ends where this one starts on its track.")
            .fix("Put the clip right after another one on the same track."));
    };
    let longest = previous.duration.min(clip.duration);
    if transition.length <= Time::ZERO || transition.length > longest {
        return Err(AppError::new("That transition is too long")
            .why("A transition can last no longer than the shorter of the two clips around its cut.")
            .fix("Choose a shorter length."));
    }
    Ok(())
}

/// Sets or takes away the transition into `clip` (see
/// [`crate::Command::SetTransition`]).
pub(crate) fn set(seq: &mut Sequence, clip: ClipId, transition: Option<Transition>) -> diag::Result<()> {
    if seq
        .audio_tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == clip))
    {
        return Err(AppError::new("Transitions are between pictures")
            .why("This clip is sound, on an audio track.")
            .fix("Select the picture it belongs to."));
    }
    for track in seq.video_tracks.iter_mut() {
        let Some(i) = track.clips.iter().position(|c| c.id == clip) else {
            continue;
        };
        if let Some(transition) = &transition {
            if !track.clips[i].is_file() {
                return Err(AppError::new("Transitions are between recordings for now")
                    .why("A nested timeline or an adjustment layer can't have one yet.")
                    .fix("Put the transition between two clips of files."));
            }
            check(track, &track.clips[i], transition)?;
            if before(track, &track.clips[i]).is_some_and(|c| !c.is_file()) {
                return Err(AppError::new("Transitions are between recordings for now")
                    .why("The clip before this one is a nested timeline or an adjustment layer.")
                    .fix("Put the transition between two clips of files."));
            }
        }
        track.clips[i].transition = transition;
        return Ok(());
    }
    Err(crate::command::missing("clip"))
}

/// Which two clips a track shows across a cut at `at`, and how far through
/// the transition it is (0 to 1), when `at` is inside one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Across<'a> {
    pub from: &'a Clip,
    pub to: &'a Clip,
    pub kind: TransitionKind,
    pub done: f64,
}

/// Where the transition into `to` plays on the timeline, and the clip it
/// comes from: `None` while it has none, or none that can play.
pub fn played<'a>(track: &'a Track, to: &Clip) -> Option<(&'a Clip, Time, Time)> {
    let transition = to.transition?;
    let from = before(track, to).filter(|c| c.is_file() && to.is_file())?;
    let (start, end) = transition.span(from, to);
    (start < end).then_some((from, start, end))
}

/// The transition of `track` playing at `at`, if any.
pub fn across(track: &Track, at: Time) -> Option<Across<'_>> {
    // A transition is never longer than either clip, so it lies inside the
    // clip at `at` and the ones either side: the one into the clip under
    // `at` or into the one after it.
    let i = starting_before(track, at);
    let first = i.saturating_sub(1);
    track.clips.iter().skip(first).take(2).find_map(|to| {
        let transition = to.transition?;
        let (from, start, end) = played(track, to)?;
        if at < start || at >= end {
            return None;
        }
        let done = (at - start).as_secs_f64() / (end - start).as_secs_f64();
        Some(Across {
            from,
            to,
            kind: transition.kind,
            done: done.clamp(0.0, 1.0),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackKind;
    use crate::{ClipId, MediaId, TrackId};

    fn clip(id: u64, start: i64, length: i64) -> Clip {
        Clip {
            lines: Default::default(),
            effects: Default::default(),
            mode: Default::default(),
            transition: None,
            title: None,
            grade: Default::default(),
            id: ClipId(id),
            media: MediaId(1),
            stream: 0,
            start: Time::seconds(start),
            source_in: Time::ZERO,
            duration: Time::seconds(length),
            link: None,
            speed: Default::default(),
            look: Default::default(),
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
        }
    }

    fn track(clips: Vec<Clip>) -> Track {
        Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: "V1".into(),
            clips: clips.into_iter().collect(),
            muted: false,
            solo: false,
            locked: false,
            hidden: false,
            gain_db: 0.0,
            sound: Default::default(),
        }
    }

    #[test]
    fn a_transition_plays_across_its_cut_half_each_side() {
        let mut to = clip(2, 4, 6);
        to.transition = Some(Transition {
            kind: TransitionKind::Dissolve,
            length: Time::seconds(2),
        });
        let track = track(vec![clip(1, 0, 4), to]);
        assert!(
            across(&track, Time::from_ratio(29, 10).unwrap()).is_none(),
            "before it"
        );
        let start = across(&track, Time::seconds(3)).unwrap();
        assert_eq!(
            (start.from.id, start.to.id, start.done),
            (ClipId(1), ClipId(2), 0.0)
        );
        assert_eq!(
            across(&track, Time::seconds(4)).unwrap().done,
            0.5,
            "the cut is its middle"
        );
        assert!(across(&track, Time::seconds(5)).is_none(), "over by then");
    }

    #[test]
    fn a_transition_needs_a_clip_just_before_and_fits_both() {
        let to = clip(2, 4, 6);
        let joined = track(vec![clip(1, 0, 4), to.clone()]);
        let one = Transition::new(TransitionKind::Wipe);
        assert!(check(&joined, &to, &one).is_ok());
        let long = Transition {
            length: Time::seconds(5),
            ..one
        };
        assert!(check(&joined, &to, &long).is_err(), "the clip before is 4 s");
        let apart = track(vec![clip(1, 0, 3), to.clone()]);
        assert!(check(&apart, &to, &one).is_err(), "a gap, not a cut");
        // Trimmed shorter than it: a shorter one plays.
        let mut short = clip(2, 4, 1);
        short.transition = Some(long);
        let trimmed = track(vec![clip(1, 0, 4), short]);
        let (start, end) = long.span(&trimmed.clips[0], &trimmed.clips[1]);
        assert_eq!(
            (start, end),
            (Time::from_ratio(7, 2).unwrap(), Time::from_ratio(9, 2).unwrap())
        );
        // Moved apart later: it waits, unused.
        let mut waiting = to;
        waiting.transition = Some(one);
        let apart = track(vec![clip(1, 0, 3), waiting]);
        assert!(across(&apart, Time::seconds(4)).is_none());
    }
}
