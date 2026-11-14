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
}

impl EffectKind {
    /// Every kind there is, with its usual settings: what is added.
    pub const ALL: [EffectKind; 1] = [EffectKind::Vignette(Vignette::USUAL)];

    /// Its name, as the interface shows it.
    pub fn name(&self) -> &'static str {
        match self {
            EffectKind::Vignette(_) => "Vignette",
        }
    }

    /// Whether its settings are within what the interface offers.
    fn check(&self) -> diag::Result<()> {
        match self {
            EffectKind::Vignette(v) => v.check(),
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

    fn check(&self) -> diag::Result<()> {
        for (name, value) in [
            ("amount", self.amount),
            ("size", self.size),
            ("softness", self.softness),
        ] {
            if !(0..=ONE).contains(&value) {
                return Err(out_of_range("Vignette", name));
            }
        }
        Ok(())
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

fn out_of_range(effect: &str, setting: &str) -> AppError {
    AppError::new(format!("That {effect} can't be used"))
        .why(format!("Its {setting} is outside what the effect allows."))
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
