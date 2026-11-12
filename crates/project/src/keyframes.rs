//! A clip's look changing over time: keyframes.
//!
//! Any of a look's numbers ([`LookField`]) can be given keyframes: values at
//! moments of the clip, with the value in between worked out from the two
//! around it ([`Ease`]). A number without keyframes keeps the clip's
//! [`Look`] value, as it always did.
//!
//! A keyframe's moment is a time in the clip's file (source time), not on the
//! timeline: the keyframes stay on the picture they were set on. Moving a
//! clip, trimming either end, sliding or rolling it, or changing its speed
//! leaves every keyframe on the same frame of the file; slipping a clip moves
//! them with what it shows. Keyframes outside the stretch a clip shows are
//! kept (trimming back brings them back, as it brings back the picture)
//! and still shape the values inside it. A split ([`Keyframes::cut_to`])
//! gives each half only its own keyframes, plus one at the cut holding the
//! value there, so neither half's picture changes.
//!
//! Values in between are worked out in whole numbers, never floats, so the
//! viewer and an export compute exactly the same picture.

use crate::look::Look;
use crate::model::Clip;
use crate::time::{Rate, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One of a look's numbers, as keyframes name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LookField {
    X,
    Y,
    Scale,
    Turn,
    CropLeft,
    CropTop,
    CropRight,
    CropBottom,
    Opacity,
    Exposure,
    Contrast,
    Saturation,
    Temperature,
    Tint,
}

impl LookField {
    pub const ALL: [LookField; 14] = [
        LookField::X,
        LookField::Y,
        LookField::Scale,
        LookField::Turn,
        LookField::CropLeft,
        LookField::CropTop,
        LookField::CropRight,
        LookField::CropBottom,
        LookField::Opacity,
        LookField::Exposure,
        LookField::Contrast,
        LookField::Saturation,
        LookField::Temperature,
        LookField::Tint,
    ];

    /// The numbers that are keyed together: a place is one thing, so its X
    /// and Y always get their keyframes at the same moments. Every other
    /// number stands alone.
    pub const GROUPS: [&'static [LookField]; 13] = [
        &[LookField::X, LookField::Y],
        &[LookField::Scale],
        &[LookField::Turn],
        &[LookField::Opacity],
        &[LookField::CropLeft],
        &[LookField::CropTop],
        &[LookField::CropRight],
        &[LookField::CropBottom],
        &[LookField::Exposure],
        &[LookField::Contrast],
        &[LookField::Saturation],
        &[LookField::Temperature],
        &[LookField::Tint],
    ];

    /// The fields keyed together with this one (see [`LookField::GROUPS`]).
    pub fn group(self) -> &'static [LookField] {
        LookField::GROUPS
            .into_iter()
            .find(|group| group.contains(&self))
            .unwrap_or(&[])
    }

    /// Its value in `look`.
    pub fn of(self, look: &Look) -> i32 {
        match self {
            LookField::X => look.x,
            LookField::Y => look.y,
            LookField::Scale => look.scale,
            LookField::Turn => look.turn,
            LookField::CropLeft => look.crop_left,
            LookField::CropTop => look.crop_top,
            LookField::CropRight => look.crop_right,
            LookField::CropBottom => look.crop_bottom,
            LookField::Opacity => look.opacity,
            LookField::Exposure => look.exposure,
            LookField::Contrast => look.contrast,
            LookField::Saturation => look.saturation,
            LookField::Temperature => look.temperature,
            LookField::Tint => look.tint,
        }
    }

    /// Sets its value in `look`.
    pub fn set(self, look: &mut Look, value: i32) {
        let slot = match self {
            LookField::X => &mut look.x,
            LookField::Y => &mut look.y,
            LookField::Scale => &mut look.scale,
            LookField::Turn => &mut look.turn,
            LookField::CropLeft => &mut look.crop_left,
            LookField::CropTop => &mut look.crop_top,
            LookField::CropRight => &mut look.crop_right,
            LookField::CropBottom => &mut look.crop_bottom,
            LookField::Opacity => &mut look.opacity,
            LookField::Exposure => &mut look.exposure,
            LookField::Contrast => &mut look.contrast,
            LookField::Saturation => &mut look.saturation,
            LookField::Temperature => &mut look.temperature,
            LookField::Tint => &mut look.tint,
        };
        *slot = value;
    }

    /// Its name in plain words, for messages.
    pub fn name(self) -> &'static str {
        match self {
            LookField::X | LookField::Y => "position",
            LookField::Scale => "size",
            LookField::Turn => "turn",
            LookField::CropLeft => "crop left",
            LookField::CropTop => "crop top",
            LookField::CropRight => "crop right",
            LookField::CropBottom => "crop bottom",
            LookField::Opacity => "opacity",
            LookField::Exposure => "exposure",
            LookField::Contrast => "contrast",
            LookField::Saturation => "saturation",
            LookField::Temperature => "temperature",
            LookField::Tint => "tint",
        }
    }

    /// Whether `value` is one this number can hold (the look's own limits).
    pub fn holds(self, value: i32) -> bool {
        let mut look = Look::PLAIN;
        self.set(&mut look, value);
        self.of(&look.within_limits()) == value
    }
}

/// How the value goes from one keyframe to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Ease {
    /// Evenly, in a straight line.
    #[default]
    Linear,
    /// It stays until the next keyframe, then jumps.
    Hold,
    /// Starting and arriving gently (slow, faster, slow).
    Smooth,
}

impl Ease {
    pub const ALL: [Ease; 3] = [Ease::Linear, Ease::Hold, Ease::Smooth];

    /// Its name in the interface.
    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::Hold => "Hold",
            Ease::Smooth => "Smooth",
        }
    }
}

/// A value at a moment of the clip's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Keyframe {
    /// Where in the clip's file, from its start (source time).
    pub at: Time,
    /// In the millionths the look's number is counted in.
    pub value: i32,
    /// How the value goes from here to the next keyframe.
    #[serde(default)]
    pub ease: Ease,
}

/// Every keyframe of one clip: for each number that has any, its keyframes
/// in time order, never two at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keyframes(BTreeMap<LookField, Vec<Keyframe>>);

/// One whole in the fixed-point shares used in between keyframes.
const SHARE_ONE: u128 = 1 << 32;

impl Keyframes {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The numbers that have keyframes, with them.
    pub fn fields(&self) -> impl Iterator<Item = (LookField, &[Keyframe])> {
        self.0.iter().map(|(field, keys)| (*field, keys.as_slice()))
    }

    /// `field`'s keyframes, in time order; empty when it has none.
    pub fn of(&self, field: LookField) -> &[Keyframe] {
        self.0.get(&field).map_or(&[], Vec::as_slice)
    }

    /// Whether `field` changes over time.
    pub fn animates(&self, field: LookField) -> bool {
        self.0.contains_key(&field)
    }

    /// `base` with every number that has keyframes set to its value at
    /// `at` (in the file), within the look's limits. Without keyframes,
    /// `base` itself. Allocates nothing: this runs for every layer of every
    /// frame.
    pub fn apply(&self, base: Look, at: Time) -> Look {
        if self.0.is_empty() {
            return base;
        }
        let mut look = base;
        for (field, keys) in &self.0 {
            if let Some(value) = value_at(keys, at) {
                field.set(&mut look, value);
            }
        }
        look.within_limits()
    }

    /// Sets `field`'s keyframe in `near` (a stretch of the file, end
    /// excluded) to `value`, or adds one at `at` when there is none there.
    /// A keyframe that is there keeps its ease; a new one takes the ease of
    /// the stretch it lands in, so the curve keeps its kind.
    pub fn set(&mut self, field: LookField, at: Time, near: (Time, Time), value: i32) {
        let keys = self.0.entry(field).or_default();
        if let Some(key) = keys.iter_mut().find(|k| near.0 <= k.at && k.at < near.1) {
            key.value = value;
            return;
        }
        let index = keys.partition_point(|k| k.at < at);
        let ease = index
            .checked_sub(1)
            .and_then(|i| keys.get(i))
            .map_or(Ease::Linear, |before| before.ease);
        keys.insert(index, Keyframe { at, value, ease });
    }

    /// Removes `field`'s keyframes in `near` (end excluded). Returns the
    /// value the last one removed held, when any was.
    pub fn remove(&mut self, field: LookField, near: (Time, Time)) -> Option<i32> {
        let keys = self.0.get_mut(&field)?;
        let removed = keys
            .iter()
            .rev()
            .find(|k| near.0 <= k.at && k.at < near.1)
            .map(|k| k.value);
        keys.retain(|k| !(near.0 <= k.at && k.at < near.1));
        if keys.is_empty() {
            self.0.remove(&field);
        }
        removed
    }

    /// Sets the ease of `field`'s keyframes in `near` (end excluded).
    /// Returns whether there was one.
    pub fn set_ease(&mut self, field: LookField, near: (Time, Time), ease: Ease) -> bool {
        let Some(keys) = self.0.get_mut(&field) else {
            return false;
        };
        let mut found = false;
        for key in keys.iter_mut().filter(|k| near.0 <= k.at && k.at < near.1) {
            key.ease = ease;
            found = true;
        }
        found
    }

    /// Takes every keyframe of `field` away.
    pub fn clear(&mut self, field: LookField) {
        self.0.remove(&field);
    }

    /// Only what matters between `from` and `to` in the file: keyframes
    /// outside that stretch go, and where one went that shaped the values
    /// inside it, a keyframe at that end holds the value there, so the
    /// picture between `from` and `to` stays as it was (exactly, apart from
    /// a smooth stretch cut in two, which eases on each side of the cut).
    pub fn cut_to(&self, from: Time, to: Time) -> Keyframes {
        let mut cut = BTreeMap::new();
        for (field, keys) in &self.0 {
            let mut kept: Vec<Keyframe> = Vec::with_capacity(keys.len() + 2);
            let before = keys.iter().rev().find(|k| k.at < from);
            let starts_inside = keys.iter().any(|k| k.at == from);
            if let Some(before) = before
                && !starts_inside
                && let Some(value) = value_at(keys, from)
            {
                kept.push(Keyframe {
                    at: from,
                    value,
                    ease: before.ease,
                });
            }
            kept.extend(keys.iter().filter(|k| from <= k.at && k.at <= to));
            let after = keys.iter().any(|k| k.at > to);
            let ends_inside = keys.iter().any(|k| k.at == to);
            if after
                && !ends_inside
                && from < to
                && let Some(value) = value_at(keys, to)
            {
                kept.push(Keyframe {
                    at: to,
                    value,
                    ease: Ease::Linear,
                });
            }
            if !kept.is_empty() {
                cut.insert(*field, kept);
            }
        }
        Keyframes(cut)
    }

    /// What is wrong with these keyframes, in plain words, each starting
    /// with `place`.
    pub fn problems(&self, place: &str) -> Vec<String> {
        let mut problems = Vec::new();
        for (field, keys) in &self.0 {
            let name = field.name();
            if keys.is_empty() {
                problems.push(format!("{place} has an empty list of {name} keyframes"));
            }
            if keys.windows(2).any(|pair| pair[0].at >= pair[1].at) {
                problems.push(format!("{place} has {name} keyframes out of order"));
            }
            if keys.iter().any(|k| k.at.is_negative()) {
                problems.push(format!("{place} has a {name} keyframe before its file starts"));
            }
            if keys.iter().any(|k| !field.holds(k.value)) {
                problems.push(format!("{place} has a {name} keyframe out of range"));
            }
        }
        problems
    }
}

/// The value `keys` (in time order) give at `at`: the first keyframe's
/// before it, the last one's after it, and in between as the earlier
/// keyframe's ease says. `None` without keyframes.
pub fn value_at(keys: &[Keyframe], at: Time) -> Option<i32> {
    let next = keys.partition_point(|k| k.at <= at);
    let Some(before) = next.checked_sub(1).and_then(|i| keys.get(i)) else {
        return keys.first().map(|k| k.value);
    };
    let Some(after) = keys.get(next) else {
        return Some(before.value);
    };
    let share = share(at - before.at, after.at - before.at);
    let share = match before.ease {
        Ease::Hold => 0,
        Ease::Linear => share,
        Ease::Smooth => smooth(share),
    };
    Some(between(before.value, after.value, share))
}

/// How far `done` is through `whole`, in [`SHARE_ONE`]ths: exact fractions
/// brought to a fixed point, rounding down. 0 for a `whole` of no length.
fn share(done: Time, whole: Time) -> u128 {
    if whole <= Time::ZERO || done <= Time::ZERO {
        return 0;
    }
    if done >= whole {
        return SHARE_ONE;
    }
    // done / whole = (done.num * whole.den) / (done.den * whole.num), with
    // both below 2^127. Halve both until the denominator fits in 64 bits,
    // so the shift by 32 below can't overflow (the share is below one).
    let mut n = done.num().unsigned_abs() as u128 * u128::from(whole.den());
    let mut d = u128::from(done.den()) * whole.num().unsigned_abs() as u128;
    let excess = (128 - d.leading_zeros()).saturating_sub(64);
    n >>= excess;
    d >>= excess;
    if d == 0 {
        return 0;
    }
    ((n << 32) / d).min(SHARE_ONE)
}

/// Slow, faster, slow: 3s² - 2s³, in [`SHARE_ONE`]ths.
fn smooth(s: u128) -> u128 {
    let squared = s * s / SHARE_ONE;
    squared * (3 * SHARE_ONE - 2 * s) / SHARE_ONE
}

/// From `a` to `b`, `share` of the way ([`SHARE_ONE`]ths), to the nearest
/// whole number.
fn between(a: i32, b: i32, share: u128) -> i32 {
    let span = i128::from(b) - i128::from(a);
    let moved = (span * share as i128 + (SHARE_ONE / 2) as i128).div_euclid(SHARE_ONE as i128);
    (i128::from(a) + moved).clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

impl Clip {
    /// The look its picture has at `at` on the timeline: its keyframes'
    /// values there, its look's for numbers without keyframes.
    pub fn look_at(&self, at: Time) -> Look {
        self.keyframes.apply(self.look, self.source_at(at))
    }

    /// Where on the timeline the moment `source` of its file is shown.
    pub fn timeline_at(&self, source: Time) -> Time {
        self.start + self.speed.unscale(source - self.source_in)
    }

    /// `at` brought onto the clip: its first frame's start when `at` is
    /// before the clip, its last frame's when it is after. Keyframes set
    /// "at the playhead" with the playhead off the clip go there.
    pub fn moment(&self, at: Time, rate: Rate) -> Time {
        if at < self.start {
            return self.start;
        }
        let last = rate.frame_start(rate.frame_at(self.end()) - 1).max(self.start);
        let last = if last >= self.end() { self.start } else { last };
        at.min(last)
    }

    /// The stretch of the file shown in the timeline frame that `at` is in:
    /// what "a keyframe here" means. End excluded.
    pub fn frame_in_file(&self, at: Time, rate: Rate) -> (Time, Time) {
        let frame = rate.frame_at(at);
        (
            self.source_at(rate.frame_start(frame)),
            self.source_at(rate.frame_start(frame + 1)),
        )
    }

    /// The timeline frames (numbers at `rate`) that hold a keyframe of any
    /// of `fields`, inside the clip, in order, each once.
    pub fn keyframe_frames(&self, fields: &[LookField], rate: Rate) -> Vec<i64> {
        let (first, end) = (rate.frame_at(self.start), rate.frame_at(self.end()));
        let mut frames: Vec<i64> = fields
            .iter()
            .flat_map(|field| self.keyframes.of(*field))
            .map(|key| rate.frame_at(self.timeline_at(key.at)))
            .filter(|frame| (first..end).contains(frame))
            .collect();
        frames.sort_unstable();
        frames.dedup();
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::ONE;

    fn key(seconds: i64, value: i32, ease: Ease) -> Keyframe {
        Keyframe {
            at: Time::seconds(seconds),
            value,
            ease,
        }
    }

    fn half() -> Time {
        Time::from_ratio(1, 2).unwrap()
    }

    #[test]
    fn values_before_the_first_and_after_the_last_keyframe_hold() {
        let keys = [key(2, 100, Ease::Linear), key(4, 300, Ease::Linear)];
        assert_eq!(value_at(&keys, Time::ZERO), Some(100));
        assert_eq!(value_at(&keys, Time::seconds(2)), Some(100));
        assert_eq!(value_at(&keys, Time::seconds(4)), Some(300));
        assert_eq!(value_at(&keys, Time::seconds(9)), Some(300));
        assert_eq!(value_at(&[], Time::seconds(1)), None);
    }

    #[test]
    fn linear_goes_evenly_and_rounds_to_the_nearest() {
        let keys = [key(0, 0, Ease::Linear), key(3, ONE, Ease::Linear)];
        assert_eq!(value_at(&keys, Time::seconds(1)), Some(333_333));
        assert_eq!(value_at(&keys, Time::seconds(2)), Some(666_667));
        let down = [key(0, 10, Ease::Linear), key(1, -10, Ease::Linear)];
        assert_eq!(value_at(&down, half()), Some(0));
        assert_eq!(value_at(&down, Time::from_ratio(1, 4).unwrap()), Some(5));
    }

    #[test]
    fn hold_stays_and_then_jumps() {
        let keys = [key(0, 7, Ease::Hold), key(2, 99, Ease::Linear)];
        assert_eq!(value_at(&keys, Time::seconds(1)), Some(7));
        assert_eq!(value_at(&keys, Time::from_ratio(3999, 2000).unwrap()), Some(7));
        assert_eq!(value_at(&keys, Time::seconds(2)), Some(99));
    }

    #[test]
    fn smooth_starts_and_ends_gently_and_passes_the_middle() {
        let keys = [key(0, 0, Ease::Smooth), key(1, ONE, Ease::Linear)];
        let linear = [key(0, 0, Ease::Linear), key(1, ONE, Ease::Linear)];
        let tenth = Time::from_ratio(1, 10).unwrap();
        assert_eq!(value_at(&keys, half()), Some(ONE / 2));
        assert!(value_at(&keys, tenth) < value_at(&linear, tenth));
        assert_eq!(value_at(&keys, tenth), Some(28_000));
    }

    #[test]
    fn awkward_fractions_and_huge_spans_stay_exact_enough() {
        // NTSC frames, extreme values: no overflow, no float.
        let rate = Rate::NTSC_60;
        let keys = [
            Keyframe {
                at: rate.frame_start(1),
                value: i32::MIN + 1,
                ease: Ease::Linear,
            },
            Keyframe {
                at: rate.frame_start(1_000_001),
                value: i32::MAX,
                ease: Ease::Linear,
            },
        ];
        assert_eq!(value_at(&keys, rate.frame_start(500_001)), Some(0));
    }

    #[test]
    fn a_look_takes_its_animated_numbers_at_the_moment_and_keeps_the_rest() {
        let mut keys = Keyframes::default();
        let whole = (Time::ZERO, Time::seconds(100));
        keys.set(LookField::Opacity, Time::ZERO, (Time::ZERO, Time::ZERO), 0);
        keys.set(
            LookField::Opacity,
            Time::seconds(2),
            (Time::ZERO, Time::ZERO),
            ONE,
        );
        let base = Look {
            x: 5,
            opacity: 300_000,
            ..Look::PLAIN
        };
        let at = keys.apply(base, Time::seconds(1));
        assert_eq!((at.x, at.opacity), (5, ONE / 2));
        assert_eq!(Keyframes::default().apply(base, Time::seconds(1)), base);
        assert!(keys.animates(LookField::Opacity) && !keys.animates(LookField::X));
        assert_eq!(
            keys.remove(LookField::Opacity, whole),
            Some(ONE),
            "the last of those removed"
        );
        assert!(keys.is_empty(), "a number left without keyframes is not animated");
    }

    #[test]
    fn setting_near_an_existing_keyframe_changes_it_and_a_new_one_keeps_the_curve() {
        let mut keys = Keyframes::default();
        let none = (Time::ZERO, Time::ZERO);
        keys.set(LookField::X, Time::ZERO, none, 0);
        keys.set(LookField::X, Time::seconds(4), none, 40);
        assert!(keys.set_ease(LookField::X, (Time::ZERO, half()), Ease::Hold));
        // Inside the frame of the keyframe at 4 s: that one changes.
        let frame = (Time::from_ratio(119, 30).unwrap(), Time::seconds(4) + half());
        keys.set(LookField::X, Time::from_ratio(119, 30).unwrap(), frame, 50);
        assert_eq!(keys.of(LookField::X).len(), 2);
        assert_eq!(keys.of(LookField::X)[1].value, 50);
        // A new one between: it holds too, as the stretch it landed in did.
        keys.set(LookField::X, Time::seconds(2), none, 20);
        let eases: Vec<Ease> = keys.of(LookField::X).iter().map(|k| k.ease).collect();
        assert_eq!(eases, [Ease::Hold, Ease::Hold, Ease::Linear]);
    }

    #[test]
    fn cutting_keeps_the_values_inside_and_drops_the_rest() {
        let mut keys = Keyframes::default();
        let none = (Time::ZERO, Time::ZERO);
        for (s, v) in [(0, 0), (10, 1000), (20, 0)] {
            keys.set(LookField::Scale, Time::seconds(s), none, ONE + v);
        }
        keys.set(LookField::Tint, Time::seconds(15), none, 5);
        let cut = keys.cut_to(Time::seconds(5), Time::seconds(12));
        let scale: Vec<(Time, i32)> = cut.of(LookField::Scale).iter().map(|k| (k.at, k.value)).collect();
        assert_eq!(
            scale,
            [
                (Time::seconds(5), ONE + 500),
                (Time::seconds(10), ONE + 1000),
                (Time::seconds(12), ONE + 800),
            ]
        );
        assert_eq!(
            cut.of(LookField::Tint),
            [key(12, 5, Ease::Linear)],
            "a lone keyframe after the stretch still holds its value in it"
        );
        for s in 5..=12 {
            let at = Time::seconds(s);
            assert_eq!(
                cut.apply(Look::PLAIN, at),
                keys.apply(Look::PLAIN, at),
                "at {s} s"
            );
        }
        assert!(
            keys.cut_to(Time::seconds(30), Time::seconds(40))
                .of(LookField::X)
                .is_empty()
        );
    }

    #[test]
    fn problems_are_found() {
        let mut keys = Keyframes::default();
        keys.0.insert(
            LookField::Opacity,
            vec![key(2, ONE, Ease::Linear), key(1, 2 * ONE, Ease::Linear)],
        );
        keys.0.insert(LookField::Tint, Vec::new());
        let problems = keys.problems("clip").join("\n");
        assert!(problems.contains("out of order"), "{problems}");
        assert!(problems.contains("out of range"), "{problems}");
        assert!(problems.contains("empty"), "{problems}");
        assert!(LookField::CropLeft.holds(Look::MOST_CROP) && !LookField::CropLeft.holds(-1));
    }

    #[test]
    fn every_field_reads_back_what_it_set_and_has_one_group() {
        for field in LookField::ALL {
            let mut look = Look::PLAIN;
            field.set(&mut look, 12_345);
            assert_eq!(field.of(&look), 12_345);
            let groups = LookField::GROUPS.iter().filter(|g| g.contains(&field)).count();
            assert_eq!(groups, 1, "{field:?}");
            assert!(field.group().contains(&field));
        }
    }

    #[test]
    fn keyframes_are_written_and_read_by_field() {
        let mut keys = Keyframes::default();
        keys.set(LookField::Opacity, half(), (Time::ZERO, Time::ZERO), 7);
        let text = ron::to_string(&keys).unwrap();
        assert!(text.contains("Opacity"), "{text}");
        assert_eq!(ron::from_str::<Keyframes>(&text).unwrap(), keys);
        let short: Keyframes = ron::from_str("{Tint: [(at: (1, 1), value: 3)]}").unwrap();
        assert_eq!(short.of(LookField::Tint), [key(1, 3, Ease::Linear)]);
    }
}
