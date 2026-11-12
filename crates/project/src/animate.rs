//! The commands that change a clip's look at a moment: setting it as seen
//! at the playhead, adding and removing keyframes, and putting numbers back
//! as they come. See [`crate::keyframes`] for what keyframes are.

use crate::command::{missing, sequence_mut};
use crate::ids::{ClipId, SequenceId};
use crate::keyframes::{Ease, LookField};
use crate::look::Look;
use crate::model::{Clip, Project};
use crate::time::{Rate, Time};
use diag::AppError;
use serde::{Deserialize, Serialize};

/// What [`crate::Command::Keyframe`] does to the keyframes in the frame at
/// the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyframeChange {
    /// One holding the value seen there (a number that had none starts
    /// changing over time from here). Where there is one already, nothing
    /// changes.
    Add,
    /// Takes it away. A number left without keyframes keeps, for the whole
    /// clip, the value the last one held.
    Remove,
    /// How the value goes from it to the next keyframe.
    Ease(Ease),
}

/// The picture clip `clip` of `sequence`, with the timeline's frame rate.
/// A clip on an audio track is refused, explained: sound has no picture.
pub(crate) fn picture_clip(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
) -> diag::Result<(&mut Clip, Rate)> {
    let seq = sequence_mut(p, sequence)?;
    let rate = seq.settings.frame_rate;
    if seq
        .audio_tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == clip))
    {
        return Err(AppError::new("Sound has no picture to place")
            .why("Transform, Crop and Quick color belong to clips on video tracks.")
            .fix("Select the clip's picture on a video track."));
    }
    seq.video_tracks
        .iter_mut()
        .flat_map(|track| track.clips.iter_mut())
        .find(|c| c.id == clip)
        .map(|c| (c, rate))
        .ok_or_else(|| missing("clip"))
}

/// See [`crate::Command::SetLookAt`].
pub(crate) fn set_look_at(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    at: Time,
    look: &Look,
) -> diag::Result<()> {
    look.check()?;
    let (clip, rate) = picture_clip(p, sequence, clip)?;
    let at = clip.moment(at, rate);
    let now = clip.look_at(at);
    let source = clip.source_at(at);
    let near = clip.frame_in_file(at, rate);
    for group in LookField::GROUPS {
        if group.iter().all(|field| field.of(look) == field.of(&now)) {
            continue;
        }
        for field in group {
            let value = field.of(look);
            if clip.keyframes.animates(*field) {
                clip.keyframes.set(*field, source, near, value);
            } else if value != field.of(&now) {
                field.set(&mut clip.look, value);
            }
        }
    }
    Ok(())
}

/// See [`crate::Command::Keyframe`].
pub(crate) fn keyframe(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    at: Time,
    fields: &[LookField],
    change: KeyframeChange,
) -> diag::Result<()> {
    if fields.is_empty() {
        return Err(AppError::new("No keyframe to change").why("No setting was given."));
    }
    let (clip, rate) = picture_clip(p, sequence, clip)?;
    let at = clip.moment(at, rate);
    if !change_at(clip, rate, at, fields, change) {
        return Err(match change {
            KeyframeChange::Add => AppError::new("There is a keyframe here already")
                .why("The playhead is on a frame that has one.")
                .fix("Change the value to change that keyframe, or move the playhead."),
            _ => AppError::new("There is no keyframe here")
                .why("The playhead is not on a frame that has one.")
                .fix("Move the playhead onto a keyframe first (the arrows beside the diamond jump to one)."),
        });
    }
    Ok(())
}

/// Makes `change` to `fields`' keyframes in the timeline frame at `at`
/// (on the clip). Returns whether anything changed.
fn change_at(clip: &mut Clip, rate: Rate, at: Time, fields: &[LookField], change: KeyframeChange) -> bool {
    let now = clip.look_at(at);
    let source = clip.source_at(at);
    let near = clip.frame_in_file(at, rate);
    let mut changed = false;
    for field in fields {
        match change {
            KeyframeChange::Add => {
                let there = clip
                    .keyframes
                    .of(*field)
                    .iter()
                    .any(|k| near.0 <= k.at && k.at < near.1);
                if !there {
                    clip.keyframes.set(*field, source, near, field.of(&now));
                    changed = true;
                }
            }
            KeyframeChange::Remove => {
                if let Some(value) = clip.keyframes.remove(*field, near) {
                    if !clip.keyframes.animates(*field) {
                        field.set(&mut clip.look, value);
                    }
                    changed = true;
                }
            }
            KeyframeChange::Ease(ease) => changed |= clip.keyframes.set_ease(*field, near, ease),
        }
    }
    // The look stays valid whatever the keyframes did to it.
    clip.look = clip.look.within_limits();
    changed
}

/// What [`crate::Command::ChangeKeyframes`] does to every keyframe in each
/// of its frames (the keyframe marks on the timeline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyframesChange {
    /// Moves them this far along the timeline (whole frames), staying on
    /// the clip. They keep their values and eases.
    Move(Time),
    /// Takes them away. A number left without keyframes keeps, for the
    /// whole clip, the value the last one held.
    Remove,
    /// How the values go from them to the next keyframes.
    Ease(Ease),
}

/// See [`crate::Command::ChangeKeyframes`].
pub(crate) fn change_keyframes(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    frames: &[Time],
    change: KeyframesChange,
) -> diag::Result<()> {
    let (clip, rate) = picture_clip(p, sequence, clip)?;
    // One moment per timeline frame, each a frame with a mark.
    let marked = clip.keyframe_frames(&LookField::ALL, rate);
    let mut wanted: Vec<i64> = frames.iter().map(|at| rate.frame_at(*at)).collect();
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() || wanted.iter().any(|frame| !marked.contains(frame)) {
        return Err(AppError::new("There is no keyframe there")
            .why("The clip has no keyframe on one of the frames that were given.")
            .fix("Pick a keyframe mark along the bottom of the selected clip."));
    }
    let starts: Vec<Time> = wanted.iter().map(|frame| rate.frame_start(*frame)).collect();
    match change {
        KeyframesChange::Move(by) => move_keyframes(clip, rate, &starts, by),
        KeyframesChange::Remove | KeyframesChange::Ease(_) => {
            let change = match change {
                KeyframesChange::Ease(ease) => KeyframeChange::Ease(ease),
                _ => KeyframeChange::Remove,
            };
            for at in starts {
                change_at(clip, rate, at, &LookField::ALL, change);
            }
            Ok(())
        }
    }
}

/// Moves the keyframes in the timeline frames starting at `starts` by `by`.
fn move_keyframes(clip: &mut Clip, rate: Rate, starts: &[Time], by: Time) -> diag::Result<()> {
    if by == Time::ZERO {
        return Err(AppError::new("The keyframes were not moved").why("They were let go where they were."));
    }
    if rate.snap(by) != by {
        return Err(AppError::new("Keyframes move by whole frames")
            .why("The distance given ends between two frames.")
            .fix("Drag the mark again: it moves a frame at a time."));
    }
    let (first, end) = (rate.frame_at(clip.start), rate.frame_at(clip.end()));
    let outside = starts
        .iter()
        .map(|at| rate.frame_at(*at + by))
        .find(|frame| !(first..end).contains(frame));
    if let Some(frame) = outside {
        let side = if frame < first { "start" } else { "end" };
        return Err(AppError::new("Keyframes stay on their clip")
            .why(format!("That would put them past the clip's {side}."))
            .fix("Drag them less far, or trim the clip longer first."));
    }
    // Each frame's keyframes move as far in the file as the clip plays
    // between that frame and the one it is dropped on (the same for every
    // frame at one speed; not with a ramp).
    let moves: Vec<((Time, Time), Time)> = starts
        .iter()
        .map(|at| {
            let in_file = clip.source_at(*at + by) - clip.source_at(*at);
            (clip.frame_in_file(*at, rate), in_file)
        })
        .collect();
    clip.keyframes.move_by(&moves).map_err(|fields| {
        let mut names: Vec<&str> = fields.iter().map(|field| field.name()).collect();
        names.dedup();
        let (names, has) = match names.as_slice() {
            [one] => ((*one).to_owned(), "has"),
            [most @ .., last] => (format!("{} and {last}", most.join(", ")), "have"),
            [] => (String::new(), "has"),
        };
        AppError::new(format!("{} {has} a keyframe there already", capitalised(&names)))
            .why(
                "A number has one keyframe a frame, and the frame these were dropped on has one of the same.",
            )
            .fix("Drop them on another frame, or remove the one there first (right-click its mark).")
    })
}

/// `text` with its first letter a capital.
fn capitalised(text: &str) -> String {
    let mut letters = text.chars();
    letters
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
}

/// See [`crate::Command::ResetLook`].
pub(crate) fn reset_look(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    fields: &[LookField],
) -> diag::Result<()> {
    let (clip, _) = picture_clip(p, sequence, clip)?;
    for field in fields {
        clip.keyframes.clear(*field);
        field.set(&mut clip.look, field.of(&Look::PLAIN));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Command;
    use crate::look::ONE;
    use crate::model::{Fingerprint, MediaInfo, MediaKind, MediaRef, SequenceSettings, VideoStream};
    use crate::time::Speed;

    /// A 10-second recording placed at 2 s, its file from 5 s on.
    fn project() -> (Project, SequenceId, ClipId) {
        let mut p = Project::new("Keys", SequenceSettings::GAME_CLIP);
        let sequence = p.active_sequence;
        let media = MediaRef {
            id: crate::MediaId(0),
            path: "game.mkv".into(),
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(60),
            info: MediaInfo {
                container: String::new(),
                video: Some(VideoStream {
                    index: 0,
                    codec: "h264".into(),
                    width: 1920,
                    height: 1080,
                    frame_rate: Rate::FPS_60,
                    variable_frame_rate: false,
                    rotation: 0,
                }),
                audio: Vec::new(),
            },
        };
        p = Command::AddMedia { media }.apply(&p).unwrap();
        let media = *p.media.keys().next().unwrap();
        let track = p.active().unwrap().video_tracks[0].id;
        p = Command::PlaceClip {
            sequence,
            track,
            clip: Clip {
                id: ClipId(0),
                media,
                stream: 0,
                start: Time::seconds(2),
                source_in: Time::seconds(5),
                duration: Time::seconds(10),
                link: None,
                speed: Speed::NORMAL,
                look: Look::PLAIN,
                sound: Default::default(),
                keyframes: Default::default(),
                ramp: Default::default(),
                blend: Default::default(),
                kind: Default::default(),
            },
        }
        .apply(&p)
        .unwrap();
        let clip = p.active().unwrap().video_tracks[0].clips[0].id;
        (p, sequence, clip)
    }

    fn the_clip(p: &Project) -> &Clip {
        &p.active().unwrap().video_tracks[0].clips[0]
    }

    fn opacity_at(p: &Project, seconds: i64) -> i32 {
        the_clip(p).look_at(Time::seconds(seconds)).opacity
    }

    /// Opacity from 0 at 3 s to full at 7 s (timeline).
    fn faded_in() -> (Project, SequenceId, ClipId) {
        let (mut p, sequence, clip) = project();
        let fields = vec![LookField::Opacity];
        p = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(3),
            fields: fields.clone(),
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        for (at, opacity) in [(3, 0), (7, ONE)] {
            p = Command::SetLookAt {
                sequence,
                clip,
                at: Time::seconds(at),
                look: Look {
                    opacity,
                    ..the_clip(&p).look_at(Time::seconds(at))
                },
            }
            .apply(&p)
            .unwrap();
        }
        (p, sequence, clip)
    }

    #[test]
    fn a_value_set_on_an_animated_number_becomes_a_keyframe_at_the_playhead() {
        let (p, _, _) = faded_in();
        let keys = the_clip(&p).keyframes.of(LookField::Opacity);
        assert_eq!(keys.len(), 2);
        assert_eq!(
            keys[0].at,
            Time::seconds(6),
            "kept in the file's time: 3 s on the timeline is 6 s in"
        );
        assert_eq!(
            (opacity_at(&p, 1), opacity_at(&p, 5), opacity_at(&p, 9)),
            (0, ONE / 2, ONE)
        );
        assert!(p.problems().is_empty(), "{:?}", p.problems());
    }

    #[test]
    fn a_number_without_keyframes_is_set_for_the_whole_clip() {
        let (p, sequence, clip) = faded_in();
        let at = Time::seconds(5);
        let p = Command::SetLookAt {
            sequence,
            clip,
            at,
            look: Look {
                tint: 100_000,
                ..the_clip(&p).look_at(at)
            },
        }
        .apply(&p)
        .unwrap();
        let clip = the_clip(&p);
        assert_eq!(clip.look.tint, 100_000);
        assert!(!clip.keyframes.animates(LookField::Tint));
        assert_eq!(
            clip.keyframes.of(LookField::Opacity).len(),
            2,
            "opacity was not touched"
        );
    }

    #[test]
    fn a_place_keys_x_and_y_together() {
        let (p, sequence, clip) = project();
        let at = Time::seconds(4);
        let p = Command::Keyframe {
            sequence,
            clip,
            at,
            fields: LookField::X.group().to_vec(),
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        let p = Command::SetLookAt {
            sequence,
            clip,
            at: Time::seconds(8),
            look: Look {
                x: 200_000,
                ..Look::PLAIN
            },
        }
        .apply(&p)
        .unwrap();
        let keys = &the_clip(&p).keyframes;
        assert_eq!((keys.of(LookField::X).len(), keys.of(LookField::Y).len()), (2, 2));
    }

    #[test]
    fn removing_the_last_keyframe_keeps_its_value() {
        let (mut p, sequence, clip) = faded_in();
        for at in [3, 7] {
            p = Command::Keyframe {
                sequence,
                clip,
                at: Time::seconds(at),
                fields: vec![LookField::Opacity],
                change: KeyframeChange::Remove,
            }
            .apply(&p)
            .unwrap();
        }
        let clip_now = the_clip(&p);
        assert!(clip_now.keyframes.is_empty());
        assert_eq!(clip_now.look.opacity, ONE, "the last one held full opacity");
        let nothing = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(3),
            fields: vec![LookField::Opacity],
            change: KeyframeChange::Remove,
        }
        .apply(&p)
        .unwrap_err();
        assert!(nothing.what().contains("no keyframe"));
    }

    #[test]
    fn a_hold_keyframe_jumps_and_reset_clears_it_all() {
        let (p, sequence, clip) = faded_in();
        let p = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(3),
            fields: vec![LookField::Opacity],
            change: KeyframeChange::Ease(Ease::Hold),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(opacity_at(&p, 6), 0);
        let p = Command::ResetLook {
            sequence,
            clip,
            fields: vec![LookField::Opacity, LookField::Scale],
        }
        .apply(&p)
        .unwrap();
        assert!(the_clip(&p).keyframes.is_empty());
        assert_eq!(the_clip(&p).look, Look::PLAIN);
    }

    #[test]
    fn keyframes_with_the_playhead_off_the_clip_go_on_its_nearest_frame() {
        let (p, sequence, clip) = project();
        let p = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(30),
            fields: vec![LookField::Turn],
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        let last_frame = Rate::FPS_60.frame_start(12 * 60 - 1);
        let key = the_clip(&p).keyframes.of(LookField::Turn)[0];
        assert_eq!(the_clip(&p).timeline_at(key.at), last_frame);
    }

    #[test]
    fn keyframes_stay_on_the_picture_through_trims_speed_and_moves() {
        let (p, sequence, clip) = faded_in();
        // Trim 2 s off the start: the fade's frames stay where they were.
        let trimmed = Command::Trim {
            sequence,
            clip,
            edge: crate::Edge::Start,
            to: Time::seconds(4),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(opacity_at(&trimmed, 5), ONE / 2);
        // Half speed: the fade takes twice as long on the timeline.
        let slow = Command::SetSpeed {
            sequence,
            clip,
            speed: Speed::new(1, 2).unwrap(),
        }
        .apply(&p)
        .unwrap();
        assert_eq!(opacity_at(&slow, 4), 0);
        assert_eq!(opacity_at(&slow, 8), ONE / 2);
        assert_eq!(opacity_at(&slow, 12), ONE);
        // Moved 10 s later: the fade moves along.
        let moved = Command::MoveClip {
            sequence,
            clip,
            start: Time::seconds(12),
            track: None,
        }
        .apply(&p)
        .unwrap();
        assert_eq!(opacity_at(&moved, 15), ONE / 2);
    }

    #[test]
    fn a_split_gives_each_half_its_own_keyframes_and_the_picture_does_not_change() {
        let (p, sequence, clip) = faded_in();
        let before: Vec<i32> = (2..12).map(|s| opacity_at(&p, s)).collect();
        let split = Command::Split {
            sequence,
            at: Time::seconds(4),
            clip: Some(clip),
        }
        .apply(&p)
        .unwrap();
        let clips = &split.active().unwrap().video_tracks[0].clips;
        let after: Vec<i32> = (2..12)
            .map(|s| {
                let at = Time::seconds(s);
                let c = clips.iter().find(|c| c.start <= at && at < c.end()).unwrap();
                c.look_at(at).opacity
            })
            .collect();
        assert_eq!(before, after);
        let left = clips[0].keyframes.of(LookField::Opacity);
        let right = clips[1].keyframes.of(LookField::Opacity);
        assert_eq!(left.len(), 2, "its own keyframe and one at the cut");
        assert_eq!(right.len(), 2, "one at the cut and its own");
        assert_eq!((left[1].at, right[0].at), (Time::seconds(7), Time::seconds(7)));
        assert!(split.problems().is_empty(), "{:?}", split.problems());
    }

    /// Moves, removes or eases the marks at `frames` (seconds on the timeline).
    fn marks(p: &Project, frames: &[i64], change: KeyframesChange) -> diag::Result<Project> {
        let (sequence, clip) = (p.active_sequence, the_clip(p).id);
        Command::ChangeKeyframes {
            sequence,
            clip,
            frames: frames.iter().map(|s| Time::seconds(*s)).collect(),
            change,
        }
        .apply(p)
    }

    fn opacity_keys(p: &Project) -> Vec<(Time, i32)> {
        the_clip(p)
            .keyframes
            .of(LookField::Opacity)
            .iter()
            .map(|k| (k.at, k.value))
            .collect()
    }

    #[test]
    fn a_mark_moves_all_its_keyframes_in_the_files_time_and_undo_brings_it_back() {
        let (p, sequence, clip) = faded_in();
        // A turn keyframe on the same frame as the fade's start (3 s).
        let p = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(3),
            fields: vec![LookField::Turn],
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        let moved = marks(&p, &[3], KeyframesChange::Move(Time::seconds(2))).unwrap();
        assert_eq!(
            opacity_keys(&moved),
            [(Time::seconds(8), 0), (Time::seconds(10), ONE)],
            "the fade's start goes from 6 s to 8 s of the file and keeps its value"
        );
        assert_eq!(
            the_clip(&moved).keyframes.of(LookField::Turn)[0].at,
            Time::seconds(8)
        );
        assert_eq!(
            the_clip(&moved).keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [5 * 60, 7 * 60]
        );
        assert!(moved.problems().is_empty(), "{:?}", moved.problems());
        // Undo is the snapshot before: nothing to do but keep it.
        assert_eq!(opacity_keys(&p)[0].0, Time::seconds(6));
        // Past the other one: they swap places in time order.
        let past = marks(&p, &[3], KeyframesChange::Move(Time::seconds(6))).unwrap();
        assert_eq!(
            opacity_keys(&past),
            [(Time::seconds(10), ONE), (Time::seconds(12), 0)]
        );
    }

    #[test]
    fn a_move_onto_a_frame_with_the_same_numbers_keyframe_is_refused() {
        let (p, sequence, clip) = faded_in();
        let refused = marks(&p, &[3], KeyframesChange::Move(Time::seconds(4))).unwrap_err();
        assert!(
            refused.what().contains("Opacity has a keyframe there"),
            "{}",
            refused.what()
        );
        // Another number's keyframe there: the two marks become one.
        let p = Command::Keyframe {
            sequence,
            clip,
            at: Time::seconds(9),
            fields: vec![LookField::Scale],
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        let joined = marks(&p, &[9], KeyframesChange::Move(Time::seconds(-2))).unwrap();
        assert_eq!(
            the_clip(&joined).keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [3 * 60, 7 * 60]
        );
    }

    #[test]
    fn keyframes_stay_on_their_clip_and_move_by_whole_frames() {
        let (p, _, _) = faded_in();
        // The clip shows 2 s to 12 s of the timeline.
        let early = marks(&p, &[3], KeyframesChange::Move(Time::seconds(-2))).unwrap_err();
        assert!(early.what().contains("stay on their clip"));
        assert!(early.why_text().unwrap_or_default().contains("start"));
        let late = marks(&p, &[7], KeyframesChange::Move(Time::seconds(5))).unwrap_err();
        assert!(late.why_text().unwrap_or_default().contains("end"));
        let to_the_first_frame = marks(&p, &[3], KeyframesChange::Move(Time::seconds(-1))).unwrap();
        assert_eq!(opacity_keys(&to_the_first_frame)[0].0, Time::seconds(5));
        let last = Rate::FPS_60.frame_start(12 * 60 - 1) - Time::seconds(7);
        assert!(
            marks(&p, &[7], KeyframesChange::Move(last)).is_ok(),
            "onto its last frame"
        );
        let half_frame = Rate::FPS_60.frame_start(1) - Time::from_ratio(1, 240).unwrap();
        assert!(marks(&p, &[3], KeyframesChange::Move(half_frame)).is_err());
        assert!(marks(&p, &[3], KeyframesChange::Move(Time::ZERO)).is_err());
        assert!(
            marks(&p, &[5], KeyframesChange::Remove).is_err(),
            "no mark at 5 s"
        );
        assert!(marks(&p, &[], KeyframesChange::Remove).is_err());
    }

    #[test]
    fn several_marks_move_together_at_another_speed() {
        let (p, sequence, clip) = faded_in();
        let p = Command::SetSpeed {
            sequence,
            clip,
            speed: Speed::new(1, 2).unwrap(),
        }
        .apply(&p)
        .unwrap();
        // At half speed the fade runs from 4 s to 12 s of the timeline.
        assert_eq!(
            the_clip(&p).keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [4 * 60, 12 * 60]
        );
        let moved = marks(&p, &[4, 12], KeyframesChange::Move(Time::seconds(2))).unwrap();
        assert_eq!(
            opacity_keys(&moved),
            [(Time::seconds(7), 0), (Time::seconds(11), ONE)],
            "2 s of the timeline is 1 s of the file"
        );
    }

    #[test]
    fn marks_are_removed_and_eased_in_one_step() {
        let (p, _, _) = faded_in();
        let eased = marks(&p, &[3, 7], KeyframesChange::Ease(Ease::Hold)).unwrap();
        let eases: Vec<Ease> = the_clip(&eased)
            .keyframes
            .of(LookField::Opacity)
            .iter()
            .map(|k| k.ease)
            .collect();
        assert_eq!(eases, [Ease::Hold, Ease::Hold]);
        let removed = marks(&p, &[3, 7], KeyframesChange::Remove).unwrap();
        assert!(the_clip(&removed).keyframes.is_empty());
        assert_eq!(the_clip(&removed).look.opacity, ONE, "the last one's value stays");
    }

    #[test]
    fn sound_has_no_keyframes() {
        let (p, sequence, _) = project();
        let track = p.active().unwrap().audio_tracks[0].id;
        let p = Command::PlaceClip {
            sequence,
            track,
            clip: Clip {
                start: Time::seconds(30),
                ..the_clip(&p).clone()
            },
        }
        .apply(&p)
        .unwrap();
        let sound = p.active().unwrap().audio_tracks[0].clips[0].id;
        let refused = Command::Keyframe {
            sequence,
            clip: sound,
            at: Time::seconds(31),
            fields: vec![LookField::Opacity],
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap_err();
        assert!(refused.what().contains("no picture"));
    }
}
