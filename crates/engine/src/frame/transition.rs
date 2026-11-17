//! A transition's two pictures at a moment inside it: the clip it comes
//! from and the clip it goes into, each read past its edge where its file
//! goes on (a clip with a speed ramp, or at the end of its file, holds its
//! edge frame), with their looks changed by how far through it is.

use super::{Layer, layer_seen};
use project::transitions::Across;
use project::{Clip, Look, MediaRef, Project, Rate, Time, TransitionKind};

/// One whole in a look's millionths.
const WHOLE: f64 = 1_000_000.0;

/// The pictures a track shows across a transition at `at`, bottom first.
/// `rate` is the timeline's frame rate, `frame` its picture size.
pub(super) fn layers(
    project: &Project,
    across: &Across,
    rate: Rate,
    frame: (u32, u32),
    at: Time,
) -> Vec<Layer> {
    let done = across.done.clamp(0.0, 1.0);
    let (from_look, to_look) = looks(across.kind, done);
    let from = from_look.and_then(|change| seen(project, across.from, rate, frame, at, change));
    let to = to_look.and_then(|change| seen(project, across.to, rate, frame, at, change));
    // The picture coming in goes over the one leaving, but for Zoom
    // through, where the one leaving fades away over the next.
    let mut layers: Vec<Layer> = match across.kind {
        TransitionKind::Zoom => [to, from],
        _ => [from, to],
    }
    .into_iter()
    .flatten()
    .collect();
    layers.retain(|layer| layer.opacity > 0.0);
    layers
}

/// How each picture's look changes `done` of the way through: `None` when
/// it isn't seen then.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Change {
    /// Its opacity times this.
    opacity: f64,
    /// Its size times this.
    scale: f64,
    /// Moved across by this much of the frame's width.
    shift: f64,
    /// Its picture cut back from the right to this much of it.
    keep: f64,
}

impl Change {
    const NONE: Change = Change {
        opacity: 1.0,
        scale: 1.0,
        shift: 0.0,
        keep: 1.0,
    };

    fn applied(&self, mut look: Look) -> Look {
        let scaled = |value: i32, by: f64| (f64::from(value) * by).round() as i32;
        look.opacity = scaled(look.opacity, self.opacity);
        look.scale = scaled(look.scale, self.scale);
        look.x = look.x.saturating_add((self.shift * WHOLE).round() as i32);
        if self.keep < 1.0 {
            // What is left of the picture after its own crop, cut back so
            // only `keep` of the picture's width shows from its left side.
            let right = ((1.0 - self.keep) * WHOLE).round() as i32;
            look.crop_right = look.crop_right.max(right);
        }
        look
    }
}

/// The change to the picture leaving and the one coming in.
fn looks(kind: TransitionKind, done: f64) -> (Option<Change>, Option<Change>) {
    let some = |change: Change| Some(change);
    match kind {
        TransitionKind::Dissolve => (
            some(Change::NONE),
            some(Change {
                opacity: done,
                ..Change::NONE
            }),
        ),
        TransitionKind::DipToBlack if done < 0.5 => (
            some(Change {
                opacity: 1.0 - 2.0 * done,
                ..Change::NONE
            }),
            None,
        ),
        TransitionKind::DipToBlack => (
            None,
            some(Change {
                opacity: 2.0 * done - 1.0,
                ..Change::NONE
            }),
        ),
        TransitionKind::Wipe if done <= 0.0 => (some(Change::NONE), None),
        TransitionKind::Wipe => (
            some(Change::NONE),
            some(Change {
                keep: done,
                ..Change::NONE
            }),
        ),
        TransitionKind::Push => {
            let eased = ease(done);
            (
                some(Change {
                    shift: -eased,
                    ..Change::NONE
                }),
                some(Change {
                    shift: 1.0 - eased,
                    ..Change::NONE
                }),
            )
        }
        TransitionKind::Zoom => {
            let eased = ease(done);
            (
                some(Change {
                    opacity: 1.0 - eased,
                    scale: 1.0 + eased,
                    ..Change::NONE
                }),
                some(Change {
                    scale: 0.8 + 0.2 * eased,
                    ..Change::NONE
                }),
            )
        }
    }
}

/// Smoothly in and out.
fn ease(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// `clip`'s picture at `at` (which may lie past its edges) with `change`.
fn seen(
    project: &Project,
    clip: &Clip,
    rate: Rate,
    frame: (u32, u32),
    at: Time,
    change: Change,
) -> Option<Layer> {
    let media = project.media.get(&clip.media)?;
    // Its look and effects are as at its nearest frame.
    let last = rate.frame_start(rate.frame_at(clip.end()) - 1).max(clip.start);
    let inside = at.max(clip.start).min(last);
    let look = change.applied(clip.look_at(inside));
    layer_seen(
        clip,
        media,
        frame,
        inside,
        look,
        source_time(clip, media, at, inside),
    )
}

/// Where in its file `clip` is read at `at`: past its edge while its file
/// goes on and it plays at one speed, else at its nearest frame `inside`.
fn source_time(clip: &Clip, media: &MediaRef, at: Time, inside: Time) -> Time {
    if !clip.ramp.is_empty() {
        return clip.source_at(inside);
    }
    let wanted = clip.source_at(at);
    let last_frame = media
        .info
        .video
        .as_ref()
        .map(|video| video.frame_rate)
        .map(|rate| rate.frame_start(rate.frame_at(media.duration) - 1))
        .unwrap_or(media.duration);
    if wanted < Time::ZERO || wanted > last_frame {
        clip.source_at(inside)
    } else {
        wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dissolve_fades_the_next_picture_in_over_the_last() {
        let (from, to) = looks(TransitionKind::Dissolve, 0.25);
        assert_eq!(from, Some(Change::NONE));
        assert_eq!(to.map(|c| c.opacity), Some(0.25));
    }

    #[test]
    fn a_dip_shows_one_picture_at_a_time_through_black() {
        let (from, to) = looks(TransitionKind::DipToBlack, 0.25);
        assert_eq!((from.map(|c| c.opacity), to), (Some(0.5), None));
        let (from, to) = looks(TransitionKind::DipToBlack, 0.75);
        assert_eq!((from, to.map(|c| c.opacity)), (None, Some(0.5)));
    }

    #[test]
    fn a_wipe_and_a_push_move_across_the_frame() {
        let (_, to) = looks(TransitionKind::Wipe, 0.3);
        let look = to.expect("seen").applied(Look::PLAIN);
        assert_eq!(look.crop_right, 700_000);
        assert_eq!(looks(TransitionKind::Wipe, 0.0).1, None, "nothing of it yet");
        let (from, to) = looks(TransitionKind::Push, 0.5);
        assert_eq!(from.expect("seen").applied(Look::PLAIN).x, -500_000);
        assert_eq!(to.expect("seen").applied(Look::PLAIN).x, 500_000);
    }

    #[test]
    fn its_own_crop_and_opacity_are_kept_under_the_change() {
        let look = Look {
            crop_right: 900_000,
            opacity: 500_000,
            ..Look::PLAIN
        };
        let changed = Change {
            opacity: 0.5,
            keep: 0.5,
            ..Change::NONE
        }
        .applied(look);
        assert_eq!((changed.crop_right, changed.opacity), (900_000, 250_000));
    }
}
