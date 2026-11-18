//! Where the selected clip's look keyframes stand at the playhead, for the
//! picture sections' diamonds: per group of numbers ([`LookField::GROUPS`])
//! whether one is at the playhead, between others or not there at all, the
//! nearest before and after, and the ease of the one at the playhead. Also
//! the "Over time: …" start of a section's summary.

use project::{Ease, LookField, Rate, Time};
use ui_kit::keyframe::{KeyState, Keys};

/// Where one group of numbers' keyframes ([`LookField::GROUPS`]) stand at
/// the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct GroupKeys {
    pub(super) state: KeyState,
    /// The nearest timeline frames with a keyframe before and after the
    /// playhead's, inside the clip.
    pub(super) before: Option<i64>,
    pub(super) after: Option<i64>,
    /// The ease of the keyframe at the playhead, as an index of `Ease::ALL`.
    pub(super) ease: Option<usize>,
}

/// Every group's keyframes at the playhead, for the diamonds.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct KeyTable {
    groups: [GroupKeys; LookField::GROUPS.len()],
    /// The playhead is on the clip, so keyframes can be added and removed.
    can: bool,
}

impl KeyTable {
    pub(super) fn at(clip: &project::Clip, playhead: Time, rate: Rate, can: bool) -> KeyTable {
        let here = rate.frame_at(playhead);
        let groups = LookField::GROUPS.map(|group| {
            if !group.iter().any(|field| clip.keyframes.animates(*field)) {
                return GroupKeys::default();
            }
            let frames = clip.keyframe_frames(group, rate);
            let at = frames.contains(&here);
            let near = clip.frame_in_file(playhead, rate);
            let ease = group
                .iter()
                .flat_map(|field| clip.keyframes.of(*field))
                .find(|key| at && near.0 <= key.at && key.at < near.1)
                .and_then(|key| Ease::ALL.iter().position(|ease| *ease == key.ease));
            GroupKeys {
                state: if at { KeyState::At } else { KeyState::Between },
                before: frames.iter().rev().find(|frame| **frame < here).copied(),
                after: frames.iter().find(|frame| **frame > here).copied(),
                ease,
            }
        });
        KeyTable { groups, can }
    }

    /// The group `fields` is (or starts with).
    pub(super) fn of(&self, fields: &[LookField]) -> Option<&GroupKeys> {
        let first = fields.first()?;
        LookField::GROUPS
            .iter()
            .position(|group| group.contains(first))
            .and_then(|i| self.groups.get(i))
    }

    /// The diamond's state for the group of `field`.
    pub(super) fn keys<'a>(&self, field: LookField, eases: &'a [&'a str]) -> Keys<'a> {
        let group = self.of(&[field]).copied().unwrap_or_default();
        Keys {
            state: group.state,
            before: group.before.is_some(),
            after: group.after.is_some(),
            can: self.can,
            eases,
            ease: group.ease,
        }
    }

    /// Whether any of `fields` changes over time.
    pub(super) fn animates(&self, fields: &[LookField]) -> bool {
        fields
            .iter()
            .any(|field| self.of(&[*field]).is_some_and(|g| g.state != KeyState::Still))
    }
}

/// The names of `fields` that change over time, put first in a section's
/// summary so a narrow header still shows them: "Over time: opacity,
/// size · ", or nothing.
pub(super) fn over_time(keys: &KeyTable, fields: &[LookField]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for field in fields {
        if keys.animates(&[*field]) && !names.contains(&field.name()) {
            names.push(field.name());
        }
    }
    match names.is_empty() {
        true => String::new(),
        false => format!("Over time: {} · ", names.join(", ")),
    }
}
