//! A clip's picture effects (format 19): an ordered list, each applied to
//! the picture the one before it gave, after the clip's quick colour and
//! before it is placed in the frame. Numbers are whole millionths, like a
//! look's, so preview and export work them out the same.
//!
//! Any setting can change over the clip (format 24): its keyframes are kept
//! at moments of the clip's file, like a look's, and worked out between with
//! the same whole-number steps (`crate::keyframes::value_at`).
//!
//! Effects are set on pictures: clips on video tracks that play a file or
//! a nested timeline. An adjustment layer's effects (on everything below
//! it) come later.

use crate::keyframes::{Ease, Keyframe, value_at};
use crate::look::ONE;
use crate::time::Time;
use diag::AppError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which effect of a clip: its own number, unique among that clip's
/// effects (a clip split in two keeps the same numbers on both halves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub u32);

/// One effect on a clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub id: EffectId,
    /// Switched off: kept, with its settings, but not applied.
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub off: bool,
    pub kind: EffectKind,
    /// Settings that change over the clip (format 24): for each, by its
    /// place in [`EffectKind::numbers`], keyframes at moments of the file.
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub keys: EffectKeys,
}

/// An effect's keyframes: per setting (its place in
/// [`EffectKind::numbers`]), in time order, never two at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectKeys(pub BTreeMap<u8, Vec<Keyframe>>);

impl EffectKeys {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Setting `index`'s keyframes; empty when it has none.
    pub fn of(&self, index: usize) -> &[Keyframe] {
        u8::try_from(index)
            .ok()
            .and_then(|i| self.0.get(&i))
            .map_or(&[], Vec::as_slice)
    }

    /// Sets setting `index`'s keyframe in `near` (a stretch of the file,
    /// end excluded) to `value`, or adds one at `at`; a new one takes the
    /// ease of the stretch it lands in.
    pub fn set(&mut self, index: usize, at: Time, near: (Time, Time), value: i32) {
        let Ok(index) = u8::try_from(index) else { return };
        let keys = self.0.entry(index).or_default();
        if let Some(key) = keys.iter_mut().find(|k| near.0 <= k.at && k.at < near.1) {
            key.value = value;
            return;
        }
        let place = keys.partition_point(|k| k.at < at);
        let ease = place
            .checked_sub(1)
            .and_then(|i| keys.get(i))
            .map_or(Ease::Linear, |before| before.ease);
        keys.insert(place, Keyframe { at, value, ease });
    }

    /// Removes setting `index`'s keyframes in `near` (end excluded); the
    /// setting stops changing when its last goes.
    pub fn remove(&mut self, index: usize, near: (Time, Time)) {
        let Ok(index) = u8::try_from(index) else { return };
        if let Some(keys) = self.0.get_mut(&index) {
            keys.retain(|k| !(near.0 <= k.at && k.at < near.1));
            if keys.is_empty() {
                self.0.remove(&index);
            }
        }
    }

    /// Sets the ease of setting `index`'s keyframes in `near`.
    pub fn set_ease(&mut self, index: usize, near: (Time, Time), ease: Ease) {
        let Ok(index) = u8::try_from(index) else { return };
        for key in self.0.get_mut(&index).into_iter().flatten() {
            if near.0 <= key.at && key.at < near.1 {
                key.ease = ease;
            }
        }
    }

    /// Takes away every keyframe of the settings in `indices`.
    pub fn clear(&mut self, indices: std::ops::Range<usize>) {
        self.0.retain(|i, _| !indices.contains(&usize::from(*i)));
    }
}

impl Effect {
    /// Its settings at `at` (a moment of the clip's file): the keyframes'
    /// values where a setting has them, kept within the effect's limits.
    pub fn kind_at(&self, at: Time) -> EffectKind {
        if self.keys.is_empty() {
            return self.kind;
        }
        let mut numbers = self.kind.numbers();
        for (index, number) in numbers.iter_mut().enumerate() {
            if let Some(value) = value_at(self.keys.of(index), at) {
                *number = value;
            }
        }
        self.kind.with_numbers(&numbers)
    }
}

/// What an effect does, with its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectKind {
    Vignette(Vignette),
    Grain(Grain),
    Sharpen(Sharpen),
    Glow(Glow),
    Blur(Blur),
    Pixelate(Pixelate),
    Zoom(Zoom),
    Shake(Shake),
    Pan(Pan),
}

impl EffectKind {
    /// Every kind there is, with its usual settings: what is added.
    pub const ALL: [EffectKind; 9] = [
        EffectKind::Vignette(Vignette::USUAL),
        EffectKind::Grain(Grain::USUAL),
        EffectKind::Sharpen(Sharpen::USUAL),
        EffectKind::Glow(Glow::USUAL),
        EffectKind::Blur(Blur::USUAL),
        EffectKind::Pixelate(Pixelate::USUAL),
        EffectKind::Zoom(Zoom::USUAL),
        EffectKind::Shake(Shake::USUAL),
        EffectKind::Pan(Pan::USUAL),
    ];

    /// Its name, as the interface shows it.
    pub fn name(&self) -> &'static str {
        match self {
            EffectKind::Vignette(_) => "Vignette",
            EffectKind::Grain(_) => "Film grain",
            EffectKind::Sharpen(_) => "Sharpen",
            EffectKind::Glow(_) => "Glow",
            EffectKind::Blur(_) => "Blur",
            EffectKind::Pixelate(_) => "Pixelate",
            EffectKind::Zoom(_) => "Zoom to a point",
            EffectKind::Shake(_) => "Shake",
            EffectKind::Pan(_) => "Smooth pan",
        }
    }

    /// Its settings as numbers, in the order the inspector shows them
    /// (the place a keyframe is kept under). A box's are its left, top,
    /// width and height, after the effect's own.
    pub fn numbers(&self) -> Vec<i32> {
        match *self {
            EffectKind::Vignette(v) => vec![v.amount, v.size, v.softness],
            EffectKind::Grain(g) => vec![g.amount, g.size],
            EffectKind::Sharpen(s) => vec![s.amount, s.radius],
            EffectKind::Glow(g) => vec![g.amount, g.threshold, g.radius],
            EffectKind::Blur(b) => vec![b.radius, b.spot.left, b.spot.top, b.spot.width, b.spot.height],
            EffectKind::Pixelate(p) => vec![p.size, p.spot.left, p.spot.top, p.spot.width, p.spot.height],
            EffectKind::Zoom(z) => vec![z.amount, z.x, z.y],
            EffectKind::Shake(s) => vec![s.amount, s.speed],
            EffectKind::Pan(p) => vec![p.zoom, p.from_x, p.from_y, p.to_x, p.to_y],
        }
    }

    /// The same effect with its settings set from `numbers` (as
    /// [`EffectKind::numbers`] gives them; missing ones kept), each kept
    /// within its limits and a box inside the picture.
    pub fn with_numbers(&self, numbers: &[i32]) -> EffectKind {
        let mut n = self.numbers();
        for (into, from) in n.iter_mut().zip(numbers) {
            *into = (*from).clamp(0, ONE);
        }
        let spot = |at: usize| Spot::fitted(n[at], n[at + 1], n[at + 2], n[at + 3]);
        match *self {
            EffectKind::Vignette(_) => EffectKind::Vignette(Vignette {
                amount: n[0],
                size: n[1],
                softness: n[2],
            }),
            EffectKind::Grain(_) => EffectKind::Grain(Grain {
                amount: n[0],
                size: n[1],
            }),
            EffectKind::Sharpen(_) => EffectKind::Sharpen(Sharpen {
                amount: n[0],
                radius: n[1],
            }),
            EffectKind::Glow(_) => EffectKind::Glow(Glow {
                amount: n[0],
                threshold: n[1],
                radius: n[2],
            }),
            EffectKind::Blur(_) => EffectKind::Blur(Blur {
                radius: n[0],
                spot: spot(1),
            }),
            EffectKind::Pixelate(_) => EffectKind::Pixelate(Pixelate {
                size: n[0],
                spot: spot(1),
            }),
            EffectKind::Zoom(_) => EffectKind::Zoom(Zoom {
                amount: n[0],
                x: n[1],
                y: n[2],
            }),
            EffectKind::Shake(_) => EffectKind::Shake(Shake {
                amount: n[0],
                speed: n[1],
            }),
            EffectKind::Pan(_) => EffectKind::Pan(Pan {
                zoom: n[0],
                from_x: n[1],
                from_y: n[2],
                to_x: n[3],
                to_y: n[4],
            }),
        }
    }

    /// Where its box's settings are among its numbers, if it has a box.
    pub fn spot_numbers(&self) -> Option<std::ops::Range<usize>> {
        matches!(self, EffectKind::Blur(_) | EffectKind::Pixelate(_)).then_some(1..5)
    }

    /// Whether its settings are within what the interface offers.
    fn check(&self) -> diag::Result<()> {
        match self {
            EffectKind::Vignette(v) => shares("Vignette", &[v.amount, v.size, v.softness]),
            EffectKind::Grain(g) => shares("Film grain", &[g.amount, g.size]),
            EffectKind::Sharpen(s) => shares("Sharpen", &[s.amount, s.radius]),
            EffectKind::Glow(g) => shares("Glow", &[g.amount, g.threshold, g.radius]),
            EffectKind::Blur(b) => shares("Blur", &[b.radius]).and_then(|()| b.spot.check("Blur")),
            EffectKind::Pixelate(p) => shares("Pixelate", &[p.size]).and_then(|()| p.spot.check("Pixelate")),
            EffectKind::Zoom(z) => shares("Zoom to a point", &[z.amount, z.x, z.y]),
            EffectKind::Shake(s) => shares("Shake", &[s.amount, s.speed]),
            EffectKind::Pan(p) => shares("Smooth pan", &[p.from_x, p.from_y, p.to_x, p.to_y, p.zoom]),
        }
    }
}

/// Darkens the picture towards its edges, in an ellipse of its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vignette {
    /// How dark the edges go: 0 (not at all) to [`ONE`] (black).
    pub amount: i32,
    /// Where darkening starts, from the middle: 0 (at once) to [`ONE`]
    /// (at the edges' middles), as a share of the way to an edge.
    pub size: i32,
    /// How far past `size` it takes to reach full darkness: from a hard
    /// edge (0) to [`ONE`].
    pub softness: i32,
}

impl Vignette {
    pub const USUAL: Vignette = Vignette {
        amount: ONE / 2,
        size: ONE * 6 / 10,
        softness: ONE / 2,
    };
}

/// Brightness noise like film's, new every frame, strongest in the middle
/// tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grain {
    /// How strong: 0 to [`ONE`].
    pub amount: i32,
    /// How coarse: 0 (fine, a pixel of 1080 lines) to [`ONE`] (four).
    pub size: i32,
}

impl Grain {
    pub const USUAL: Grain = Grain {
        amount: ONE * 3 / 10,
        size: ONE * 3 / 10,
    };
}

/// Crisper detail: the picture pushed away from a slight blur of itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sharpen {
    /// How much: 0 to [`ONE`].
    pub amount: i32,
    /// How fine the detail it works on: 0 (half a pixel of 1080 lines) to
    /// [`ONE`] (three).
    pub radius: i32,
}

impl Sharpen {
    pub const USUAL: Sharpen = Sharpen {
        amount: ONE * 4 / 10,
        radius: ONE * 3 / 10,
    };
}

/// Bright parts spill a soft light around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glow {
    /// How strong: 0 to [`ONE`].
    pub amount: i32,
    /// How bright a part must be to glow: 0 (everything) to [`ONE`]
    /// (white only).
    pub threshold: i32,
    /// How far the light spreads: 0 (two pixels of 1080 lines) to [`ONE`]
    /// (sixty).
    pub radius: i32,
}

impl Glow {
    pub const USUAL: Glow = Glow {
        amount: ONE / 2,
        threshold: ONE * 6 / 10,
        radius: ONE * 4 / 10,
    };
}

/// Softens the picture, or a box in it (names and chat to hide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blur {
    /// How soft: 0 (two pixels of 1080 lines) to [`ONE`] (a hundred and
    /// twenty).
    pub radius: i32,
    pub spot: Spot,
}

impl Blur {
    pub const USUAL: Blur = Blur {
        radius: ONE * 4 / 10,
        spot: Spot::WHOLE,
    };
}

/// Big square blocks over the picture, or a box in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pixelate {
    /// How big the blocks: 0 (four pixels of 1080 lines) to [`ONE`] (a
    /// hundred).
    pub size: i32,
    pub spot: Spot,
}

impl Pixelate {
    pub const USUAL: Pixelate = Pixelate {
        size: ONE * 3 / 10,
        spot: Spot::WHOLE,
    };
}

/// Moves in on a point of the picture over the clip's length, easing in
/// and out, inside the clip's own frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zoom {
    /// How far in by the clip's end: 0 (not at all) to [`ONE`] (three times).
    pub amount: i32,
    /// The point moved in on, which stays where it is: shares of the
    /// picture's width and height.
    pub x: i32,
    pub y: i32,
}

impl Zoom {
    pub const USUAL: Zoom = Zoom {
        amount: ONE / 4,
        x: ONE / 2,
        y: ONE / 2,
    };
}

/// A camera shake: the picture wanders and tilts a little, smoothly,
/// enlarged just enough that its edges stay out of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shake {
    /// How much: 0 to [`ONE`].
    pub amount: i32,
    /// How fast: 0 (slow drift) to [`ONE`] (jitter).
    pub speed: i32,
}

impl Shake {
    pub const USUAL: Shake = Shake {
        amount: ONE * 3 / 10,
        speed: ONE / 2,
    };
}

/// The view slides from one part of the picture to another over the
/// clip's length, easing in and out, enlarged so there is room to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pan {
    /// Where the view's middle starts and ends: shares of the picture's
    /// width and height.
    pub from_x: i32,
    pub from_y: i32,
    pub to_x: i32,
    pub to_y: i32,
    /// How far in: 0 (not at all: nothing to move over) to [`ONE`] (twice).
    pub zoom: i32,
}

impl Pan {
    pub const USUAL: Pan = Pan {
        from_x: ONE * 3 / 10,
        from_y: ONE / 2,
        to_x: ONE * 7 / 10,
        to_y: ONE / 2,
        zoom: ONE * 3 / 10,
    };
}

/// Where in the picture an effect applies: a box, its left and top edge and
/// its size, each a share of the picture's width or height in millionths.
/// The whole picture is (0, 0, [`ONE`], [`ONE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spot {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Spot {
    pub const WHOLE: Spot = Spot {
        left: 0,
        top: 0,
        width: ONE,
        height: ONE,
    };

    /// A box in the middle, a third of the picture each way: where a box
    /// starts when one is asked for.
    pub const MIDDLE: Spot = Spot {
        left: ONE / 3,
        top: ONE / 3,
        width: ONE / 3,
        height: ONE / 3,
    };

    pub fn is_whole(&self) -> bool {
        *self == Spot::WHOLE
    }

    /// A box from its edges and size, kept inside the picture: pushed back
    /// in past an edge, never less than 1 % across.
    pub fn fitted(left: i32, top: i32, width: i32, height: i32) -> Spot {
        let least = ONE / 100;
        let width = width.clamp(least, ONE);
        let height = height.clamp(least, ONE);
        Spot {
            left: left.clamp(0, ONE - width),
            top: top.clamp(0, ONE - height),
            width,
            height,
        }
    }

    fn check(&self, effect: &str) -> diag::Result<()> {
        shares(effect, &[self.left, self.top, self.width, self.height])?;
        match self.left + self.width <= ONE && self.top + self.height <= ONE {
            true => Ok(()),
            false => Err(out_of_range(effect)),
        }
    }
}

/// Refuses any of `values` outside 0 to [`ONE`].
fn shares(effect: &str, values: &[i32]) -> diag::Result<()> {
    match values.iter().all(|v| (0..=ONE).contains(v)) {
        true => Ok(()),
        false => Err(out_of_range(effect)),
    }
}

/// A clip's effects, in the order they are applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Effects(pub Vec<Effect>);

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Effect> {
        self.0.iter()
    }

    /// The effects that are applied: those not switched off, in order.
    pub fn applied(&self) -> impl Iterator<Item = &Effect> {
        self.0.iter().filter(|e| !e.off)
    }

    pub fn get(&self, id: EffectId) -> Option<&Effect> {
        self.0.iter().find(|e| e.id == id)
    }

    /// These effects with `kind` added at the end, and its number.
    pub fn with_added(&self, kind: EffectKind) -> (Effects, EffectId) {
        let id = EffectId(self.0.iter().map(|e| e.id.0 + 1).max().unwrap_or(1));
        let mut more = self.0.clone();
        more.push(Effect {
            id,
            off: false,
            kind,
            keys: EffectKeys::default(),
        });
        (Effects(more), id)
    }

    /// Refuses numbers used twice and settings out of range.
    pub fn check(&self) -> diag::Result<()> {
        let mut seen = std::collections::HashSet::new();
        for effect in &self.0 {
            if !seen.insert(effect.id) {
                return Err(AppError::new("These effects can't be used")
                    .why("Two of them have the same number.")
                    .fix("This is a bug in the app; please report it."));
            }
            effect.kind.check()?;
            let count = effect.kind.numbers().len();
            for (index, keys) in &effect.keys.0 {
                let in_order = keys.windows(2).all(|pair| pair[0].at < pair[1].at);
                let in_range = keys.iter().all(|k| (0..=ONE).contains(&k.value));
                if usize::from(*index) >= count || keys.is_empty() || !in_order || !in_range {
                    return Err(out_of_range(effect.kind.name()));
                }
            }
        }
        Ok(())
    }

    /// Whether `other` only takes away, reorders, switches or changes the
    /// settings of these effects: every number in it is one of these.
    pub fn has_every_id_of(&self, other: &Effects) -> bool {
        other.0.iter().all(|e| self.get(e.id).is_some())
    }
}

fn out_of_range(effect: &str) -> AppError {
    AppError::new(format!("That {effect} can't be used"))
        .why("One of its settings is outside what the effect allows.")
        .fix("Choose a value the slider offers.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_get_numbers_of_their_own_and_are_checked() {
        let (one, first) = Effects::default().with_added(EffectKind::ALL[0]);
        let (two, second) = one.with_added(EffectKind::ALL[0]);
        assert_eq!((first, second), (EffectId(1), EffectId(2)));
        assert!(two.check().is_ok());
        assert!(two.has_every_id_of(&one));
        assert!(!one.has_every_id_of(&two), "a new number is not a change");
        let mut twice = two.clone();
        twice.0[1].id = first;
        assert!(twice.check().is_err());
        let mut far = one.clone();
        far.0[0].kind = EffectKind::Vignette(Vignette {
            amount: ONE + 1,
            ..Vignette::USUAL
        });
        assert!(far.check().is_err());
    }

    #[test]
    fn switched_off_effects_are_kept_and_not_applied() {
        let (mut effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        assert_eq!(effects.applied().count(), 1);
        effects.0[0].off = true;
        assert_eq!(effects.applied().count(), 0);
        assert_eq!(effects.iter().count(), 1);
    }

    #[test]
    fn a_setting_with_keyframes_changes_over_the_clip_in_whole_steps() {
        let blur = EffectKind::Blur(Blur {
            radius: 100_000,
            spot: Spot::MIDDLE,
        });
        let (mut effects, _) = Effects::default().with_added(blur);
        let s = Time::seconds;
        let frame = |at: Time| (at, at + Time::from_ratio(1, 60).unwrap());
        // The box's left edge goes from 10 % to 50 % over the first two
        // seconds of the file.
        let keys = &mut effects.0[0].keys;
        keys.set(1, s(0), frame(s(0)), 100_000);
        keys.set(1, s(2), frame(s(2)), 500_000);
        assert!(effects.check().is_ok());
        let left = |at| match effects.0[0].kind_at(at) {
            EffectKind::Blur(b) => b.spot.left,
            other => panic!("{other:?}"),
        };
        assert_eq!(left(s(0)), 100_000);
        assert_eq!(left(s(1)), 300_000, "halfway");
        assert_eq!(left(s(5)), 500_000, "the last keyframe's after it");
        // Settings without keyframes keep their own value.
        let radius = match effects.0[0].kind_at(s(1)) {
            EffectKind::Blur(b) => b.radius,
            other => panic!("{other:?}"),
        };
        assert_eq!(radius, 100_000);
        // A box pushed past the edge by its keyframes stays inside.
        let keys = &mut effects.0[0].keys;
        keys.set(1, s(2), frame(s(2)), ONE);
        match effects.0[0].kind_at(s(3)) {
            EffectKind::Blur(b) => assert_eq!(b.spot.left + b.spot.width, ONE),
            other => panic!("{other:?}"),
        }
        // Removing the last keyframe of a setting stops it changing.
        let keys = &mut effects.0[0].keys;
        keys.remove(1, frame(s(0)));
        keys.remove(1, frame(s(2)));
        assert!(keys.is_empty());
    }

    #[test]
    fn keyframes_out_of_range_or_for_no_setting_are_refused() {
        let (mut effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        let frame = (Time::ZERO, Time::from_ratio(1, 60).unwrap());
        effects.0[0].keys.set(7, Time::ZERO, frame, 0);
        assert!(effects.check().is_err(), "a vignette has three settings");
        effects.0[0].keys = EffectKeys::default();
        effects.0[0].keys.set(0, Time::ZERO, frame, ONE + 1);
        assert!(effects.check().is_err());
    }

    #[test]
    fn numbers_go_out_and_back_unchanged() {
        for kind in EffectKind::ALL {
            assert_eq!(kind.with_numbers(&kind.numbers()), kind, "{}", kind.name());
        }
    }

    #[test]
    fn written_short() {
        let (effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        assert_eq!(
            ron::to_string(&effects).unwrap(),
            "[(id:1,kind:Vignette((amount:500000,size:600000,softness:500000)))]"
        );
    }
}
