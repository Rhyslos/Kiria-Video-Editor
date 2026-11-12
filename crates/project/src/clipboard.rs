//! Keyframes copied from a clip's keyframe marks and pasted onto a picture
//! clip: the same clip at another moment, or another clip, in this
//! timeline or another.
//!
//! A copy holds, for each copied mark, every keyframe of its frame (the
//! number, its value and how it goes on to the next) and how long after
//! the first copied mark it comes **on the timeline**: what is copied is
//! what was seen, so a fade that took a second on the timeline takes a
//! second where it is pasted, whatever speed either clip plays at.
//!
//! Pasting puts the first copied mark on the frame at the playhead
//! (brought onto the clip, as every "at the playhead" is) and the others
//! as far after it as they were, in whole frames of the timeline pasted
//! into. Where the clip already has a keyframe of a pasted number in one of
//! those frames, the pasted one replaces it (as pasting over a selection
//! replaces it); keyframes of numbers that are not pasted stay, so marks
//! join as they do when moved. Moving refuses a clash instead, because a
//! drag that lands on a keyframe is more often a slip of the hand than a
//! wish to lose it; a paste is asked for at a chosen place. A paste that
//! would put a mark past either end of the clip is refused whole, with
//! how far, so a paste never leaves keyframes where they can't be seen.

use crate::animate::picture_clip;
use crate::ids::{ClipId, SequenceId};
use crate::keyframes::{Ease, Keyframe, LookField};
use crate::model::{Clip, Project};
use crate::time::{Rate, Time};
use diag::AppError;
use serde::{Deserialize, Serialize};

/// One keyframe of a copied mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CopiedKey {
    pub field: LookField,
    /// In the millionths the look's number is counted in.
    pub value: i32,
    pub ease: Ease,
}

/// One copied mark: the keyframes of one frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CopiedMark {
    /// How long after the first copied mark it comes, on the timeline
    /// (whole frames of the timeline it was copied from).
    pub after: Time,
    /// Its keyframes, one per number, in [`LookField::ALL`] order.
    pub keys: Vec<CopiedKey>,
}

/// Keyframe marks copied from a clip, ready to paste.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CopiedKeyframes {
    /// In time order, the first `after` zero.
    pub marks: Vec<CopiedMark>,
}

impl CopiedKeyframes {
    /// The keyframes of `clip` (timeline at `rate`) in the timeline frames
    /// `frames` start in. Refused when one of those frames has none.
    pub fn copy(clip: &Clip, rate: Rate, frames: &[Time]) -> diag::Result<CopiedKeyframes> {
        let mut wanted: Vec<i64> = frames.iter().map(|at| rate.frame_at(*at)).collect();
        wanted.sort_unstable();
        wanted.dedup();
        let Some(first) = wanted.first().copied() else {
            return Err(AppError::new("No keyframes to copy")
                .why("No keyframe mark is chosen.")
                .fix("Click a keyframe mark on the selected clip, Shift-click more, or drag a box around them along the clip's bottom edge."));
        };
        let mut marks = Vec::with_capacity(wanted.len());
        for frame in wanted {
            let near = clip.frame_in_file(rate.frame_start(frame), rate);
            let keys: Vec<CopiedKey> = clip
                .keyframes
                .fields()
                .filter_map(|(field, keys)| {
                    let key = keys.iter().find(|k| near.0 <= k.at && k.at < near.1)?;
                    Some(CopiedKey {
                        field,
                        value: key.value,
                        ease: key.ease,
                    })
                })
                .collect();
            if keys.is_empty() {
                return Err(AppError::new("There is no keyframe there")
                    .why("The clip has no keyframe on one of the frames that were chosen.")
                    .fix("Choose keyframe marks along the bottom of the selected clip."));
            }
            marks.push(CopiedMark {
                after: rate.frame_start(frame) - rate.frame_start(first),
                keys,
            });
        }
        Ok(CopiedKeyframes { marks })
    }

    /// How many marks it holds.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The numbers it has keyframes for, in plain words, each once, in
    /// the order the inspector shows them ("position, opacity").
    pub fn says(&self) -> String {
        let keyed = |field: &LookField| self.marks.iter().flat_map(|m| &m.keys).any(|k| k.field == *field);
        LookField::GROUPS
            .iter()
            .filter(|group| group.iter().any(keyed))
            .filter_map(|group| group.first().map(|field| field.name()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// See [`crate::Command::PasteKeyframes`].
pub(crate) fn paste_keyframes(
    p: &mut Project,
    sequence: SequenceId,
    clip: ClipId,
    at: Time,
    copied: &CopiedKeyframes,
) -> diag::Result<()> {
    if copied.marks.is_empty() || copied.marks.iter().any(|m| m.keys.is_empty()) {
        return Err(AppError::new("Nothing to paste")
            .why("No keyframes were copied.")
            .fix("Choose keyframe marks on a clip and copy them first."));
    }
    if let Some(key) = copied
        .marks
        .iter()
        .flat_map(|m| &m.keys)
        .find(|k| !k.field.holds(k.value))
    {
        return Err(AppError::new("The copied keyframes can't be pasted")
            .why(format!("A {} value is out of its range.", key.field.name())));
    }
    let (clip, rate) = picture_clip(p, sequence, clip)?;
    let base = rate.frame_at(clip.moment(at, rate));
    let (first, end) = (rate.frame_at(clip.start), rate.frame_at(clip.end()));
    let frames: Vec<i64> = copied
        .marks
        .iter()
        .map(|m| rate.frame_at(rate.frame_start(base) + rate.snap(m.after)))
        .collect();
    if let Some(past) = frames
        .iter()
        .map(|f| f - (end - 1))
        .filter(|over| *over > 0)
        .max()
    {
        let frames = if past == 1 { "frame" } else { "frames" };
        return Err(AppError::new("The keyframes don't fit on the clip")
            .why(format!(
                "Pasted at the playhead, the last of them would land {past} {frames} past the clip's end."
            ))
            .fix("Move the playhead earlier, or trim the clip longer first."));
    }
    if frames.iter().any(|f| *f < first) {
        return Err(AppError::new("The keyframes don't fit on the clip")
            .why("Pasted at the playhead, the first of them would land before the clip's start.")
            .fix("Move the playhead onto the clip."));
    }
    for (mark, frame) in copied.marks.iter().zip(frames) {
        let moment = rate.frame_start(frame).max(clip.start);
        let near = clip.frame_in_file(moment, rate);
        let source = clip.source_at(moment);
        for key in &mark.keys {
            clip.keyframes.put(
                key.field,
                near,
                Keyframe {
                    at: source,
                    value: key.value,
                    ease: key.ease,
                },
            );
        }
    }
    clip.look = clip.look.within_limits();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Command;
    use crate::animate::KeyframeChange;
    use crate::look::{Look, ONE};
    use crate::model::{Fingerprint, MediaInfo, MediaKind, MediaRef, SequenceSettings, VideoStream};
    use crate::time::Speed;

    /// Two 10-second clips of one recording on V1: at 0 s and at 20 s
    /// (the second at half speed when `slow`).
    fn project(slow: bool) -> (Project, SequenceId, ClipId, ClipId) {
        let mut p = Project::new("Paste", SequenceSettings::GAME_CLIP);
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
        for start in [0, 20] {
            p = Command::PlaceClip {
                sequence,
                track,
                clip: Clip {
                    id: ClipId(0),
                    media,
                    stream: 0,
                    start: Time::seconds(start),
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
        }
        let clips = &p.active().unwrap().video_tracks[0].clips;
        let (a, b) = (clips[0].id, clips[1].id);
        if slow {
            p = Command::SetSpeed {
                sequence,
                clip: b,
                speed: Speed::new(1, 2).unwrap(),
            }
            .apply(&p)
            .unwrap();
        }
        (p, sequence, a, b)
    }

    fn clip(p: &Project, id: ClipId) -> &Clip {
        p.active()
            .unwrap()
            .video_tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .find(|c| c.id == id)
            .unwrap()
    }

    /// The first clip fades in from 1 s to 3 s (timeline), with a turn
    /// keyframe at 1 s too, holding.
    fn faded(slow: bool) -> (Project, SequenceId, ClipId, ClipId) {
        let (mut p, sequence, a, b) = project(slow);
        for (at, fields) in [
            (1, vec![LookField::Opacity, LookField::Turn]),
            (3, vec![LookField::Opacity]),
        ] {
            p = Command::Keyframe {
                sequence,
                clip: a,
                at: Time::seconds(at),
                fields,
                change: KeyframeChange::Add,
            }
            .apply(&p)
            .unwrap();
        }
        p = Command::SetLookAt {
            sequence,
            clip: a,
            at: Time::seconds(1),
            look: Look {
                opacity: 0,
                turn: 15 * ONE,
                ..clip(&p, a).look_at(Time::seconds(1))
            },
        }
        .apply(&p)
        .unwrap();
        p = Command::Keyframe {
            sequence,
            clip: a,
            at: Time::seconds(1),
            fields: vec![LookField::Turn],
            change: KeyframeChange::Ease(Ease::Hold),
        }
        .apply(&p)
        .unwrap();
        (p, sequence, a, b)
    }

    fn copy_of(p: &Project, id: ClipId, seconds: &[i64]) -> CopiedKeyframes {
        let frames: Vec<Time> = seconds.iter().map(|s| Time::seconds(*s)).collect();
        CopiedKeyframes::copy(clip(p, id), Rate::FPS_60, &frames).unwrap()
    }

    fn paste(p: &Project, id: ClipId, at: Time, copied: &CopiedKeyframes) -> diag::Result<Project> {
        Command::PasteKeyframes {
            sequence: p.active_sequence,
            clip: id,
            at,
            keyframes: copied.clone(),
        }
        .apply(p)
    }

    #[test]
    fn a_copy_holds_every_number_of_each_mark_relative_to_the_first() {
        let (p, _, a, _) = faded(false);
        let copied = copy_of(&p, a, &[3, 1]);
        assert_eq!(copied.len(), 2);
        assert_eq!(copied.marks[0].after, Time::ZERO);
        assert_eq!(copied.marks[1].after, Time::seconds(2));
        let first: Vec<LookField> = copied.marks[0].keys.iter().map(|k| k.field).collect();
        assert_eq!(first, [LookField::Turn, LookField::Opacity]);
        assert_eq!(copied.marks[0].keys[0].ease, Ease::Hold);
        assert_eq!(copied.marks[1].keys[0].value, ONE);
        assert_eq!(copied.says(), "turn, opacity");
        let none = CopiedKeyframes::copy(clip(&p, a), Rate::FPS_60, &[Time::seconds(2)]).unwrap_err();
        assert!(none.what().contains("no keyframe"));
        assert!(CopiedKeyframes::copy(clip(&p, a), Rate::FPS_60, &[]).is_err());
    }

    #[test]
    fn pasted_on_another_clip_it_looks_the_same_from_the_playhead() {
        let (p, _, a, b) = faded(false);
        let copied = copy_of(&p, a, &[1, 3]);
        let pasted = paste(&p, b, Time::seconds(22), &copied).unwrap();
        let (from, to) = (clip(&pasted, a), clip(&pasted, b));
        for offset in [0, 1, 2, 3] {
            let seen = |c: &Clip, at: i64| c.look_at(Time::seconds(at));
            assert_eq!(seen(from, 1 + offset), seen(to, 22 + offset), "{offset}");
        }
        assert_eq!(
            to.keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [22 * 60, 24 * 60],
            "from the playhead on"
        );
        assert!(pasted.problems().is_empty(), "{:?}", pasted.problems());
        // The first clip kept its own.
        assert_eq!(clip(&pasted, a).keyframes, clip(&p, a).keyframes);
    }

    #[test]
    fn spacing_is_kept_on_the_timeline_at_another_speed() {
        let (p, _, a, b) = faded(true);
        let copied = copy_of(&p, a, &[1, 3]);
        let pasted = paste(&p, b, Time::seconds(24), &copied).unwrap();
        let to = clip(&pasted, b);
        assert_eq!(
            to.keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [24 * 60, 26 * 60]
        );
        // Half speed: 2 s of the timeline is 1 s of its file.
        let opacity: Vec<Time> = to.keyframes.of(LookField::Opacity).iter().map(|k| k.at).collect();
        assert_eq!(opacity, [Time::seconds(7), Time::seconds(8)]);
    }

    #[test]
    fn a_pasted_number_replaces_its_keyframe_there_and_others_join() {
        let (p, sequence, a, _) = faded(false);
        let copied = copy_of(&p, a, &[1]);
        // A size keyframe at 6 s, and an opacity one at 6 s too.
        let mut p = p;
        p = Command::Keyframe {
            sequence,
            clip: a,
            at: Time::seconds(6),
            fields: vec![LookField::Scale, LookField::Opacity],
            change: KeyframeChange::Add,
        }
        .apply(&p)
        .unwrap();
        let pasted = paste(&p, a, Time::seconds(6), &copied).unwrap();
        let c = clip(&pasted, a);
        assert_eq!(
            c.keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [60, 3 * 60, 6 * 60]
        );
        assert_eq!(c.keyframes.of(LookField::Opacity).len(), 3, "replaced, not added");
        assert_eq!(c.look_at(Time::seconds(6)).opacity, 0, "the pasted value");
        assert_eq!(c.keyframes.of(LookField::Scale).len(), 1, "size stays");
        assert_eq!(c.keyframes.of(LookField::Turn).len(), 2);
        assert!(pasted.problems().is_empty(), "{:?}", pasted.problems());
    }

    #[test]
    fn a_paste_that_does_not_fit_is_refused_with_how_far() {
        let (p, _, a, b) = faded(false);
        let copied = copy_of(&p, a, &[1, 3]);
        // The second clip ends at 30 s: from 29 s the second mark is at 31 s.
        let refused = paste(&p, b, Time::seconds(29), &copied).unwrap_err();
        assert!(refused.what().contains("don't fit"));
        assert!(refused.why_text().unwrap_or_default().contains("61 frames past"));
        // On its last frame exactly: fits.
        let last = Rate::FPS_60.frame_start(30 * 60 - 1) - Time::seconds(2);
        assert!(paste(&p, b, last, &copied).is_ok());
        // With the playhead before the clip, from its first frame.
        let early = paste(&p, b, Time::seconds(15), &copied).unwrap();
        assert_eq!(
            clip(&early, b).keyframe_frames(&LookField::ALL, Rate::FPS_60),
            [20 * 60, 22 * 60]
        );
        let empty = CopiedKeyframes { marks: Vec::new() };
        assert!(paste(&p, b, Time::seconds(22), &empty).is_err());
        let wild = CopiedKeyframes {
            marks: vec![CopiedMark {
                after: Time::ZERO,
                keys: vec![CopiedKey {
                    field: LookField::Opacity,
                    value: 7 * ONE,
                    ease: Ease::Linear,
                }],
            }],
        };
        assert!(paste(&p, b, Time::seconds(22), &wild).is_err());
    }

    #[test]
    fn sound_takes_no_keyframes() {
        let (p, sequence, a, _) = faded(false);
        let copied = copy_of(&p, a, &[1]);
        let track = p.active().unwrap().audio_tracks[0].id;
        let p = Command::PlaceClip {
            sequence,
            track,
            clip: Clip {
                start: Time::seconds(40),
                keyframes: Default::default(),
                ..clip(&p, a).clone()
            },
        }
        .apply(&p)
        .unwrap();
        let sound = p.active().unwrap().audio_tracks[0].clips[0].id;
        let refused = paste(&p, sound, Time::seconds(41), &copied).unwrap_err();
        assert!(refused.what().contains("no picture"));
    }
}
