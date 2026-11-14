//! A clip's picture effects (format 19): an ordered list, each applied to
//! the picture the one before it gave, after the clip's quick colour and
//! before it is placed in the frame. Numbers are whole millionths, like a
//! look's, so preview and export work them out the same.
//!
//! Effects are set on pictures: clips on video tracks that play a file or
//! a nested timeline. An adjustment layer's effects (on everything below
//! it) come later.

use crate::look::ONE;
use diag::AppError;
use serde::{Deserialize, Serialize};

/// Which effect of a clip: its own number, unique among that clip's
/// effects (a clip split in two keeps the same numbers on both halves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub u32);

/// One effect on a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub id: EffectId,
    /// Switched off: kept, with its settings, but not applied.
    #[serde(default, skip_serializing_if = "crate::defaults::is_default")]
    pub off: bool,
    pub kind: EffectKind,
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
}

impl EffectKind {
    /// Every kind there is, with its usual settings: what is added.
    pub const ALL: [EffectKind; 6] = [
        EffectKind::Vignette(Vignette::USUAL),
        EffectKind::Grain(Grain::USUAL),
        EffectKind::Sharpen(Sharpen::USUAL),
        EffectKind::Glow(Glow::USUAL),
        EffectKind::Blur(Blur::USUAL),
        EffectKind::Pixelate(Pixelate::USUAL),
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
        }
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
    pub fn applied(&self) -> impl Iterator<Item = &EffectKind> {
        self.0.iter().filter(|e| !e.off).map(|e| &e.kind)
    }

    pub fn get(&self, id: EffectId) -> Option<&Effect> {
        self.0.iter().find(|e| e.id == id)
    }

    /// These effects with `kind` added at the end, and its number.
    pub fn with_added(&self, kind: EffectKind) -> (Effects, EffectId) {
        let id = EffectId(self.0.iter().map(|e| e.id.0 + 1).max().unwrap_or(1));
        let mut more = self.0.clone();
        more.push(Effect { id, off: false, kind });
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
    fn written_short() {
        let (effects, _) = Effects::default().with_added(EffectKind::ALL[0]);
        assert_eq!(
            ron::to_string(&effects).unwrap(),
            "[(id:1,kind:Vignette((amount:500000,size:600000,softness:500000)))]"
        );
    }
}
