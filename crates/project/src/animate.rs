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
    if !changed {
        return Err(match change {
            KeyframeChange::Add => AppError::new("There is a keyframe here already")
                .why("The playhead is on a frame that has one.")
                .fix("Change the value to change that keyframe, or move the playhead."),
            _ => AppError::new("There is no keyframe here")
                .why("The playhead is not on a frame that has one.")
                .fix("Move the playhead onto a keyframe first (the arrows beside the diamond jump to one)."),
        });
    }
    // The look stays valid whatever the keyframes did to it.
    clip.look = clip.look.within_limits();
    Ok(())
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
