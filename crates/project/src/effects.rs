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
    /// The effect with its box following `places` (moments of the file,
    /// with the box's left and top in millionths of the picture): Left
    /// and Top keyframes at each, in place of the ones between the first
    /// and the last. Unchanged for an effect without a box, or no places.
    pub fn following(&self, places: &[(Time, (i32, i32))]) -> Effect {
        match self.kind.spot_numbers() {
            Some(range) => self.keyed_pair(range.start, places),
            None => self.clone(),
        }
    }

    /// The effect with settings `first` and the one after it keyed at
    /// each of `places` (moments of the file, with the two values), in
    /// place of their keyframes between the first and the last place.
    pub fn keyed_pair(&self, first: usize, places: &[(Time, (i32, i32))]) -> Effect {
        let (Some(start), Some(end)) = (places.first(), places.last()) else {
            return self.clone();
        };
        let mut next = self.clone();
        // Past the last place by a hair, so a keyframe there goes too.
        let through = (start.0, end.0 + Time::from_i128(1, 1_000_000));
        for (index, pick) in [(first, 0usize), (first + 1, 1)] {
            next.keys.remove(index, through);
            for (at, (a, b)) in places {
                let value = if pick == 0 { *a } else { *b };
                next.keys.set(index, *at, (*at, *at), value.clamp(0, ONE));
            }
        }
        next
    }

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
    Stabilise(Stabilise),
    Mirror(Mirror),
    Corners(Corners),
    Outline(Outline),
    Shadow(Shadow),
    Key(Key),
    Letterbox(Letterbox),
    Mono(Amount),
    Sepia(Amount),
    Duotone(Duotone),
    Invert(Amount),
    Split(Split),
}

impl EffectKind {
    /// Every kind there is, with its usual settings: what is added.
    pub const ALL: [EffectKind; 21] = [
        EffectKind::Vignette(Vignette::USUAL),
        EffectKind::Grain(Grain::USUAL),
        EffectKind::Sharpen(Sharpen::USUAL),
        EffectKind::Glow(Glow::USUAL),
        EffectKind::Blur(Blur::USUAL),
        EffectKind::Pixelate(Pixelate::USUAL),
        EffectKind::Zoom(Zoom::USUAL),
        EffectKind::Shake(Shake::USUAL),
        EffectKind::Pan(Pan::USUAL),
        EffectKind::Stabilise(Stabilise::USUAL),
        EffectKind::Mirror(Mirror::USUAL),
        EffectKind::Corners(Corners::USUAL),
        EffectKind::Outline(Outline::USUAL),
        EffectKind::Shadow(Shadow::USUAL),
        EffectKind::Key(Key::USUAL),
        EffectKind::Letterbox(Letterbox::USUAL),
        EffectKind::Mono(Amount::WHOLLY),
        EffectKind::Sepia(Amount::WHOLLY),
        EffectKind::Duotone(Duotone::USUAL),
        EffectKind::Invert(Amount::WHOLLY),
        EffectKind::Split(Split::USUAL),
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
            EffectKind::Stabilise(_) => "Stabilise",
            EffectKind::Mirror(_) => "Mirror",
            EffectKind::Corners(_) => "Rounded corners",
            EffectKind::Outline(_) => "Outline",
            EffectKind::Shadow(_) => "Drop shadow",
            EffectKind::Key(_) => "Chroma key",
            EffectKind::Letterbox(_) => "Letterbox",
            EffectKind::Mono(_) => "Black and white",
            EffectKind::Sepia(_) => "Sepia",
            EffectKind::Duotone(_) => "Duotone",
            EffectKind::Invert(_) => "Invert",
            EffectKind::Split(_) => "Chromatic aberration",
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
            EffectKind::Stabilise(s) => vec![s.smoothing, s.zoom, s.x, s.y],
            EffectKind::Mirror(m) => vec![m.across, m.down],
            EffectKind::Corners(c) => vec![c.radius],
            EffectKind::Outline(o) => vec![o.width, o.colour.red, o.colour.green, o.colour.blue],
            EffectKind::Shadow(s) => vec![
                s.amount,
                s.softness,
                s.distance,
                s.direction,
                s.colour.red,
                s.colour.green,
                s.colour.blue,
            ],
            EffectKind::Key(k) => vec![
                k.tolerance,
                k.softness,
                k.spill,
                k.colour.red,
                k.colour.green,
                k.colour.blue,
            ],
            EffectKind::Letterbox(l) => vec![l.shape],
            EffectKind::Mono(a) | EffectKind::Sepia(a) | EffectKind::Invert(a) => vec![a.amount],
            EffectKind::Duotone(d) => vec![
                d.amount,
                d.shadows.red,
                d.shadows.green,
                d.shadows.blue,
                d.highlights.red,
                d.highlights.green,
                d.highlights.blue,
            ],
            EffectKind::Split(s) => vec![s.amount, s.direction],
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
            EffectKind::Stabilise(_) => EffectKind::Stabilise(Stabilise {
                smoothing: n[0],
                zoom: n[1],
                x: n[2],
                y: n[3],
            }),
            EffectKind::Mirror(_) => EffectKind::Mirror(Mirror {
                across: n[0],
                down: n[1],
            }),
            EffectKind::Corners(_) => EffectKind::Corners(Corners { radius: n[0] }),
            EffectKind::Outline(_) => EffectKind::Outline(Outline {
                width: n[0],
                colour: Paint::from_numbers(&n[1..4]),
            }),
            EffectKind::Shadow(_) => EffectKind::Shadow(Shadow {
                amount: n[0],
                softness: n[1],
                distance: n[2],
                direction: n[3],
                colour: Paint::from_numbers(&n[4..7]),
            }),
            EffectKind::Key(_) => EffectKind::Key(Key {
                tolerance: n[0],
                softness: n[1],
                spill: n[2],
                colour: Paint::from_numbers(&n[3..6]),
            }),
            EffectKind::Letterbox(_) => EffectKind::Letterbox(Letterbox { shape: n[0] }),
            EffectKind::Mono(_) => EffectKind::Mono(Amount { amount: n[0] }),
            EffectKind::Sepia(_) => EffectKind::Sepia(Amount { amount: n[0] }),
            EffectKind::Invert(_) => EffectKind::Invert(Amount { amount: n[0] }),
            EffectKind::Duotone(_) => EffectKind::Duotone(Duotone {
                amount: n[0],
                shadows: Paint::from_numbers(&n[1..4]),
                highlights: Paint::from_numbers(&n[4..7]),
            }),
            EffectKind::Split(_) => EffectKind::Split(Split {
                amount: n[0],
                direction: n[1],
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
            EffectKind::Stabilise(s) => shares("Stabilise", &[s.smoothing, s.zoom, s.x, s.y]),
            EffectKind::Mirror(m) => shares("Mirror", &[m.across, m.down]),
            EffectKind::Corners(c) => shares("Rounded corners", &[c.radius]),
            EffectKind::Outline(o) => shares("Outline", &[o.width]).and_then(|()| o.colour.check("Outline")),
            EffectKind::Shadow(s) => shares("Drop shadow", &[s.amount, s.softness, s.distance, s.direction])
                .and_then(|()| s.colour.check("Drop shadow")),
            EffectKind::Key(k) => shares("Chroma key", &[k.tolerance, k.softness, k.spill])
                .and_then(|()| k.colour.check("Chroma key")),
            EffectKind::Letterbox(l) => shares("Letterbox", &[l.shape]),
            EffectKind::Mono(a) | EffectKind::Sepia(a) | EffectKind::Invert(a) => {
                shares(self.name(), &[a.amount])
            }
            EffectKind::Duotone(d) => shares("Duotone", &[d.amount])
                .and_then(|()| d.shadows.check("Duotone"))
                .and_then(|()| d.highlights.check("Duotone")),
            EffectKind::Split(s) => shares("Chromatic aberration", &[s.amount, s.direction]),
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

/// A steadier picture: the view follows the camera's movement smoothed
/// out, enlarged so its edges stay out of sight. The correction comes from
/// analysing the clip (format 27): keyframes on `x` and `y`, the view's
/// middle at each frame, and the `zoom` that leaves room for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stabilise {
    /// How smooth the camera's path is made: 0 (only jitter taken out) to
    /// [`ONE`] (nearly still). Used when the clip is analysed.
    pub smoothing: i32,
    /// How far in: 0 (not at all) to [`ONE`] (half as large again).
    pub zoom: i32,
    /// Where the view's middle is: shares of the picture's width and
    /// height, the middle at a half. Keyed by the analysis.
    pub x: i32,
    pub y: i32,
}

impl Stabilise {
    pub const USUAL: Stabilise = Stabilise {
        smoothing: ONE / 2,
        zoom: ONE / 10,
        x: ONE / 2,
        y: ONE / 2,
    };

    /// Settings `x` and `y`'s place among its numbers.
    pub const MIDDLE: usize = 2;
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

/// The picture turned over, as in a mirror: left for right, top for
/// bottom, or both. Each is on from a half up, so a keyframe flips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirror {
    /// Left and right swapped: on at [`ONE`], off at 0.
    pub across: i32,
    /// Top and bottom swapped.
    pub down: i32,
}

impl Mirror {
    pub const USUAL: Mirror = Mirror { across: ONE, down: 0 };

    pub fn is_across(&self) -> bool {
        self.across >= ONE / 2
    }

    pub fn is_down(&self) -> bool {
        self.down >= ONE / 2
    }
}

/// The picture's corners rounded off, see-through outside the curve: a
/// facecam in a soft frame, or a circle when the picture is square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corners {
    /// How round: 0 (square) to [`ONE`] (half the shorter side: the ends
    /// fully round).
    pub radius: i32,
}

impl Corners {
    pub const USUAL: Corners = Corners { radius: ONE * 2 / 10 };
}

/// A colour an effect paints with: red, green and blue as shown on screen,
/// each 0 to [`ONE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paint {
    pub red: i32,
    pub green: i32,
    pub blue: i32,
}

impl Paint {
    pub const BLACK: Paint = Paint {
        red: 0,
        green: 0,
        blue: 0,
    };
    pub const WHITE: Paint = Paint {
        red: ONE,
        green: ONE,
        blue: ONE,
    };

    /// From 8-bit red, green and blue.
    pub fn from_rgb(rgb: [u8; 3]) -> Paint {
        let share = |v: u8| ((i64::from(v) * i64::from(ONE) + 127) / 255) as i32;
        Paint {
            red: share(rgb[0]),
            green: share(rgb[1]),
            blue: share(rgb[2]),
        }
    }

    /// As 8-bit red, green and blue.
    pub fn rgb(&self) -> [u8; 3] {
        let byte = |v: i32| ((i64::from(v.clamp(0, ONE)) * 255 + i64::from(ONE / 2)) / i64::from(ONE)) as u8;
        [byte(self.red), byte(self.green), byte(self.blue)]
    }

    fn from_numbers(n: &[i32]) -> Paint {
        let at = |i: usize| n.get(i).copied().unwrap_or(0);
        Paint {
            red: at(0),
            green: at(1),
            blue: at(2),
        }
    }

    fn check(&self, effect: &str) -> diag::Result<()> {
        shares(effect, &[self.red, self.green, self.blue])
    }
}

/// A line of colour around the picture's edge, outside it: it follows the
/// rounded corners and anything else see-through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outline {
    /// How thick: 0 (one pixel of 1080 lines) to [`ONE`] (forty).
    pub width: i32,
    pub colour: Paint,
}

impl Outline {
    pub const USUAL: Outline = Outline {
        width: ONE * 15 / 100,
        colour: Paint::WHITE,
    };
}

/// A soft shadow cast behind the picture, onto what is below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shadow {
    /// How dark: 0 (not at all) to [`ONE`] (the colour itself).
    pub amount: i32,
    /// How soft its edge: 0 (sharp) to [`ONE`] (sixty pixels of 1080 lines).
    pub softness: i32,
    /// How far it falls from the picture: 0 to [`ONE`] (a hundred pixels).
    pub distance: i32,
    /// Which way it falls: 0 to [`ONE`] is once round clockwise from the
    /// right (a quarter is straight down).
    pub direction: i32,
    pub colour: Paint,
}

impl Shadow {
    pub const USUAL: Shadow = Shadow {
        amount: ONE * 7 / 10,
        softness: ONE / 4,
        distance: ONE / 5,
        direction: ONE / 8,
        colour: Paint::BLACK,
    };
}

/// A colour (a green or blue screen) made see-through, and its colour
/// taken off what spills onto the rest. Colours are compared by hue and
/// strength, not brightness, so the screen's shadows go too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// How far from the screen's colour still goes: 0 (only the colour
    /// itself) to [`ONE`] (much of its side of the colour wheel).
    pub tolerance: i32,
    /// How gradually the edge goes from see-through to solid: 0 (hard) to
    /// [`ONE`].
    pub softness: i32,
    /// How much of the screen's colour is taken off what stays (green on
    /// hair and edges): 0 to [`ONE`] (all of it).
    pub spill: i32,
    /// The screen's colour.
    pub colour: Paint,
}

impl Key {
    pub const USUAL: Key = Key {
        tolerance: ONE * 4 / 10,
        softness: ONE * 3 / 10,
        spill: ONE / 2,
        // A lit green screen as a camera records it: 40, 180, 70.
        colour: Paint {
            red: 156_863,
            green: 705_882,
            blue: 274_510,
        },
    };
}

/// Black bars that give the picture another shape: above and below for a
/// wider one, at the sides for a narrower one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Letterbox {
    /// The shape left between the bars: 0 (square, 1 : 1) to [`ONE`]
    /// (three times as wide as tall).
    pub shape: i32,
}

impl Letterbox {
    /// 2.39 : 1, the wide cinema shape.
    pub const USUAL: Letterbox = Letterbox { shape: 695_000 };

    /// How many times as wide as tall the shape is.
    pub fn width_per_height(&self) -> f64 {
        1.0 + 2.0 * f64::from(self.shape.clamp(0, ONE)) / f64::from(ONE)
    }
}

/// An effect with one setting, how much of it: black and white, sepia,
/// invert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    /// 0 (the picture as it was) to [`ONE`] (all the way).
    pub amount: i32,
}

impl Amount {
    pub const WHOLLY: Amount = Amount { amount: ONE };
}

/// The picture's brightness in two colours: its shadows in one, its
/// highlights in the other, and the tones between mixed from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Duotone {
    /// 0 (the picture as it was) to [`ONE`] (only the two colours).
    pub amount: i32,
    pub shadows: Paint,
    pub highlights: Paint,
}

impl Duotone {
    /// Deep purple shadows, warm orange highlights.
    pub const USUAL: Duotone = Duotone {
        amount: ONE,
        shadows: Paint {
            red: 156_863,
            green: 78_431,
            blue: 431_373,
        },
        highlights: Paint {
            red: ONE,
            green: 666_667,
            blue: 235_294,
        },
    };
}

/// Red and blue drawn apart from green, as a cheap lens does at its edges:
/// a chromatic aberration, or an RGB split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    /// How far apart: 0 to [`ONE`] (twenty pixels of 1080 lines each way).
    pub amount: i32,
    /// Which way red goes (blue the other): 0 to [`ONE`] once round
    /// clockwise from the right.
    pub direction: i32,
}

impl Split {
    pub const USUAL: Split = Split {
        amount: ONE / 4,
        direction: 0,
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
    fn a_followed_box_gets_left_and_top_keyframes_over_its_stretch() {
        let (effects, id) = Effects::default().with_added(EffectKind::Blur(Blur {
            radius: 500_000,
            spot: Spot::MIDDLE,
        }));
        let mut blur = effects.get(id).expect("added").clone();
        // A keyframe inside the stretch is replaced; one after it stays.
        blur.keys
            .set(1, Time::seconds(1), (Time::seconds(1), Time::seconds(1)), 1);
        blur.keys
            .set(1, Time::seconds(9), (Time::seconds(9), Time::seconds(9)), 2);
        let places = [
            (Time::ZERO, (100_000, 200_000)),
            (Time::seconds(2), (300_000, 250_000)),
        ];
        let followed = blur.following(&places);
        let left: Vec<(Time, i32)> = followed.keys.of(1).iter().map(|k| (k.at, k.value)).collect();
        assert_eq!(
            left,
            [
                (Time::ZERO, 100_000),
                (Time::seconds(2), 300_000),
                (Time::seconds(9), 2)
            ]
        );
        assert_eq!(followed.keys.of(2).len(), 2);
        assert_eq!(followed.kind_at(Time::seconds(1)).spot_numbers(), Some(1..5));
        // No box, nothing to follow.
        let (plain, id) = Effects::default().with_added(EffectKind::ALL[0]);
        let vignette = plain.get(id).expect("added");
        assert_eq!(&vignette.following(&places), vignette);
    }

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
