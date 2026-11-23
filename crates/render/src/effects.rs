//! Picture effects: passes that read pictures the compositor made and write
//! one of the same size. A clip's effects run one after another, after its
//! picture is drawn upright at the size it takes in the frame and before
//! it is placed (`engine::compose`). Most effects are one pass; glow is
//! four (what is bright, blurred across, blurred down, added back). Numbers
//! arrive worked out (`Effect`); nothing here knows of projects. Sizes are
//! in reference pixels (one of 1080 lines of the picture), so a preview
//! drawn smaller looks like the export.

use crate::compositor::{
    Frame, Working, pipeline, sampler_entry, smooth_sampler, texture_binding, texture_entry, uniform_buffer,
    uniform_entry,
};

/// One effect with its numbers, as the shaders take them.
// A grade with its sampled curves is the largest by far; a frame has a few
// effects, made once a frame, so the size costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// Darker towards the edges: `amount` 0-1, from `size` (0-1 of the way
    /// to an edge's middle), reaching it `softness` further on.
    Vignette { amount: f32, size: f32, softness: f32 },
    /// Brightness noise in cells `size` reference pixels across, `amount`
    /// 0-1, new every frame: `seed` picks the frame's pattern.
    Grain { amount: f32, size: f32, seed: u32 },
    /// Detail pushed out from a blur `radius` reference pixels wide, by
    /// `amount` 0-1.
    Sharpen { amount: f32, radius: f32 },
    /// What is brighter than `threshold` (0-1 of light) blurred `radius`
    /// reference pixels wide and added back by `amount`.
    Glow {
        amount: f32,
        threshold: f32,
        radius: f32,
    },
    /// A gaussian blur `radius` reference pixels wide inside `spot`.
    Blur { radius: f32, spot: Spot },
    /// Blocks `size` reference pixels across inside `spot`.
    Pixelate { size: f32, spot: Spot },
    /// The picture moved inside its own frame: enlarged `scale` times
    /// about `fixed` (a point, 0-1, that stays where it is), turned by
    /// `turn` radians and shifted by `shift` (0-1 of its width and height).
    /// What would come from outside it is see-through.
    Move {
        scale: f32,
        fixed: (f32, f32),
        turn: f32,
        shift: (f32, f32),
    },
    /// A grade's primary corrections (`GradeNumbers`), on light.
    Grade(GradeNumbers),
    /// The picture turned over: left for right (`across`), top for bottom
    /// (`down`).
    Mirror { across: bool, down: bool },
    /// The corners rounded off, see-through outside the curve: `radius`
    /// 0-1 of half the shorter side.
    Corners { radius: f32 },
    /// A line `width` reference pixels thick around the picture's edge,
    /// outside it, of `colour` (light, 0-1 each).
    Outline { width: f32, colour: [f32; 3] },
    /// A shadow of `colour` (light) behind the picture, `amount` 0-1 dark,
    /// its edge blurred `softness` reference pixels wide, falling `offset`
    /// reference pixels across and down.
    Shadow {
        amount: f32,
        softness: f32,
        offset: (f32, f32),
        colour: [f32; 3],
    },
    /// A screen's colour made see-through: `colour` as shown (0-1 each,
    /// not light), what is within `tolerance` (0-1) of it by hue and
    /// strength gone, solid again `softness` (0-1) further, and `spill`
    /// (0-1) of its colour taken off what stays.
    Key {
        tolerance: f32,
        softness: f32,
        spill: f32,
        colour: [f32; 3],
    },
    /// Black bars leaving a shape `ratio` times as wide as tall.
    Letterbox { ratio: f32 },
    /// Towards grey of the same brightness, by `amount` 0-1.
    Mono { amount: f32 },
    /// Towards a warm brown of the same brightness, by `amount` 0-1.
    Sepia { amount: f32 },
    /// Towards its brightness in two colours (light), `shadows` for the
    /// dark end and `highlights` for the bright, by `amount` 0-1.
    Duotone {
        amount: f32,
        shadows: [f32; 3],
        highlights: [f32; 3],
    },
    /// Towards its negative (as shown), by `amount` 0-1.
    Invert { amount: f32 },
    /// Red moved `offset` reference pixels across and down, blue the other
    /// way, green left where it is.
    Split { offset: (f32, f32) },
    /// Smeared `length` reference pixels along `angle` (radians, clockwise
    /// from the right), half each way.
    Smear { length: f32, angle: f32 },
    /// Smeared towards `centre` (0-1), `amount` (0-1) of the way there.
    Radial { amount: f32, centre: (f32, f32) },
    /// Bands torn sideways with their colours apart, `amount` 0-1 hard,
    /// `often` 0-1; `seed` picks the frame's pattern.
    Glitch { amount: f32, often: f32, seed: u32 },
}

impl Effect {
    /// Whether it may make part of an opaque picture see-through, so what
    /// is below it shows.
    pub fn leaves_holes(&self) -> bool {
        matches!(self, Effect::Corners { .. } | Effect::Key { .. })
    }

    /// How far past the picture's edges it draws, in reference pixels: the
    /// picture is given that much room on every side first.
    fn reach(&self) -> f32 {
        match *self {
            Effect::Outline { width, .. } => width + 2.0,
            Effect::Shadow { softness, offset, .. } => offset.0.hypot(offset.1) + 2.0 * softness + 2.0,
            _ => 0.0,
        }
    }
}

/// A grade layer's numbers as the shader takes them: each wheel's red,
/// green, blue and brightness (-1 to 1); exposure in stops; contrast (0
/// leaves it, 1 twice as hard) around `pivot` (a light level); saturation
/// (1 leaves it); the white balance as gains on red, green and blue; and
/// highlights and shadows in stops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradeNumbers {
    pub shadows_wheel: [f32; 4],
    pub midtones_wheel: [f32; 4],
    pub highlights_wheel: [f32; 4],
    pub overall_wheel: [f32; 4],
    pub exposure: f32,
    pub contrast: f32,
    pub pivot: f32,
    pub saturation: f32,
    pub balance: [f32; 3],
    pub highlights: f32,
    pub shadows: f32,
    /// Its curves sampled at [`CURVE_SAMPLES`] even places from 0 to 1,
    /// master, red, green and blue at each; none when they are straight.
    pub curves: Option<[[f32; 4]; CURVE_SAMPLES]>,
    /// Where it applies: a colour range and a mask, as the shader's
    /// `limits` rows; all zero applies everywhere.
    pub limits: [[f32; 4]; 4],
}

/// How many places a grade's curves are sampled at.
pub const CURVE_SAMPLES: usize = 33;

/// Straight curves.
const STRAIGHT: [[f32; 4]; CURVE_SAMPLES] = {
    let mut rows = [[0.0; 4]; CURVE_SAMPLES];
    let mut i = 0;
    while i < CURVE_SAMPLES {
        let x = i as f32 / (CURVE_SAMPLES - 1) as f32;
        rows[i] = [x, x, x, x];
        i += 1;
    }
    rows
};

impl GradeNumbers {
    /// One that changes nothing.
    pub const PLAIN: GradeNumbers = GradeNumbers {
        shadows_wheel: [0.0; 4],
        midtones_wheel: [0.0; 4],
        highlights_wheel: [0.0; 4],
        overall_wheel: [0.0; 4],
        exposure: 0.0,
        contrast: 0.0,
        pivot: 0.18,
        saturation: 1.0,
        balance: [1.0; 3],
        highlights: 0.0,
        shadows: 0.0,
        curves: None,
        limits: [[0.0; 4]; 4],
    };

    fn rows(&self) -> [[f32; 4]; 7] {
        [
            self.shadows_wheel,
            self.midtones_wheel,
            self.highlights_wheel,
            self.overall_wheel,
            [self.exposure, self.contrast, self.pivot, self.saturation],
            [self.balance[0], self.balance[1], self.balance[2], self.highlights],
            [
                self.shadows,
                if self.curves.is_some() { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        ]
    }

    fn curve_rows(&self) -> [[f32; 4]; CURVE_SAMPLES] {
        self.curves.unwrap_or(STRAIGHT)
    }
}

/// A box in a picture: its left, top, right and bottom edges, each 0-1 of
/// the way across or down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Spot {
    pub const WHOLE: Spot = Spot {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    fn numbers(self) -> [f32; 4] {
        [self.left, self.top, self.right, self.bottom]
    }
}

/// One pass of a shader: which, and its numbers.
// As `Effect`: a grade's pass is the largest, and passes are few.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy)]
enum Pass {
    Vignette {
        amount: f32,
        size: f32,
        softness: f32,
    },
    Grain {
        amount: f32,
        size: f32,
    },
    Sharpen {
        amount: f32,
        radius: f32,
    },
    Bright {
        threshold: f32,
    },
    BlurAcross {
        radius: f32,
    },
    BlurDown {
        radius: f32,
    },
    Add {
        amount: f32,
    },
    Within {
        spot: Spot,
    },
    Pixelate {
        size: f32,
        spot: Spot,
    },
    Move {
        scale: f32,
        fixed: (f32, f32),
        turn: f32,
        shift: (f32, f32),
    },
    Grade(GradeNumbers),
    Mirror {
        across: bool,
        down: bool,
        within: Spot,
    },
    Corners {
        radius: f32,
        within: Spot,
    },
    /// The picture copied into a larger one, into `within`.
    Room {
        within: Spot,
    },
    Outline {
        width: f32,
        colour: [f32; 3],
    },
    /// What the picture covers, filled with a colour at `amount`, moved by
    /// `offset` (reference pixels): a shadow before it is blurred.
    Silhouette {
        amount: f32,
        offset: (f32, f32),
        colour: [f32; 3],
    },
    /// The picture over the second one (its shadow).
    Over,
    Key {
        tolerance: f32,
        softness: f32,
        spill: f32,
        colour: [f32; 3],
    },
    Letterbox {
        ratio: f32,
        within: Spot,
    },
    Mono {
        amount: f32,
    },
    Sepia {
        amount: f32,
    },
    Duotone {
        amount: f32,
        shadows: [f32; 3],
        highlights: [f32; 3],
    },
    Invert {
        amount: f32,
    },
    Split {
        offset: (f32, f32),
    },
    Smear {
        length: f32,
        angle: f32,
    },
    Radial {
        amount: f32,
        centre: (f32, f32),
    },
    Glitch {
        amount: f32,
        often: f32,
    },
}

impl Pass {
    fn numbers(self) -> [f32; 4] {
        match self {
            Pass::Vignette {
                amount,
                size,
                softness,
            } => [0.0, amount, size, softness],
            Pass::Grain { amount, size } => [1.0, amount, size, 0.0],
            Pass::Sharpen { amount, radius } => [2.0, amount, radius, 0.0],
            Pass::Bright { threshold } => [3.0, threshold, 0.0, 0.0],
            Pass::BlurAcross { radius } => [4.0, radius, 0.0, 0.0],
            Pass::BlurDown { radius } => [5.0, radius, 0.0, 0.0],
            Pass::Add { amount } => [6.0, amount, 0.0, 0.0],
            Pass::Within { .. } => [7.0, 0.0, 0.0, 0.0],
            Pass::Pixelate { size, .. } => [8.0, size, 0.0, 0.0],
            Pass::Move { scale, fixed, .. } => [9.0, scale, fixed.0, fixed.1],
            Pass::Grade(_) => [10.0, 0.0, 0.0, 0.0],
            Pass::Mirror { across, down, .. } => [
                11.0,
                if across { 1.0 } else { 0.0 },
                if down { 1.0 } else { 0.0 },
                0.0,
            ],
            Pass::Corners { radius, .. } => [12.0, radius, 0.0, 0.0],
            Pass::Room { .. } => [13.0, 0.0, 0.0, 0.0],
            Pass::Outline { width, .. } => [14.0, width, 0.0, 0.0],
            Pass::Silhouette { amount, offset, .. } => [15.0, amount, offset.0, offset.1],
            Pass::Over => [16.0, 0.0, 0.0, 0.0],
            Pass::Key {
                tolerance,
                softness,
                spill,
                ..
            } => [17.0, tolerance, softness, spill],
            Pass::Letterbox { ratio, .. } => [18.0, ratio, 0.0, 0.0],
            Pass::Mono { amount } => [19.0, amount, 0.0, 0.0],
            Pass::Sepia { amount } => [20.0, amount, 0.0, 0.0],
            Pass::Duotone { amount, .. } => [21.0, amount, 0.0, 0.0],
            Pass::Invert { amount } => [22.0, amount, 0.0, 0.0],
            Pass::Split { offset } => [23.0, 0.0, offset.0, offset.1],
            Pass::Smear { length, angle } => [24.0, length, angle, 0.0],
            Pass::Radial { amount, centre } => [25.0, amount, centre.0, centre.1],
            Pass::Glitch { amount, often } => [26.0, amount, often, 0.0],
        }
    }

    /// The colour it paints with, as light.
    fn paint(self) -> [f32; 4] {
        match self {
            Pass::Outline { colour, .. }
            | Pass::Silhouette { colour, .. }
            | Pass::Key { colour, .. }
            | Pass::Duotone { shadows: colour, .. } => [colour[0], colour[1], colour[2], 1.0],
            _ => [0.0; 4],
        }
    }

    /// The pass's third row: a box, or a move's turn and shift.
    fn extra(self) -> [f32; 4] {
        match self {
            Pass::Within { spot } | Pass::Pixelate { spot, .. } => spot.numbers(),
            Pass::Mirror { within, .. }
            | Pass::Corners { within, .. }
            | Pass::Room { within }
            | Pass::Letterbox { within, .. } => within.numbers(),
            Pass::Move { turn, shift, .. } => [turn, shift.0, shift.1, 0.0],
            // A duotone's second colour.
            Pass::Duotone { highlights, .. } => [highlights[0], highlights[1], highlights[2], 0.0],
            _ => Spot::WHOLE.numbers(),
        }
    }

    /// The pass's curve rows: a grade's curves, straight for every other.
    fn curves(self) -> [[f32; 4]; CURVE_SAMPLES] {
        match self {
            Pass::Grade(numbers) => numbers.curve_rows(),
            _ => STRAIGHT,
        }
    }

    /// The pass's limit rows: where a grade applies, everywhere for others.
    fn limits(self) -> [[f32; 4]; 4] {
        match self {
            Pass::Grade(numbers) => numbers.limits,
            _ => [[0.0; 4]; 4],
        }
    }

    /// The pass's grade rows: a grade's numbers, plain for every other.
    fn grade(self) -> [[f32; 4]; 7] {
        match self {
            Pass::Grade(numbers) => numbers.rows(),
            _ => GradeNumbers::PLAIN.rows(),
        }
    }
}

const PASS_BYTES: u64 = 80 + 7 * 16 + CURVE_SAMPLES as u64 * 16 + 4 * 16;

/// The effect passes for pictures made in one working space.
#[derive(Debug)]
pub struct Effects {
    working: Working,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// What a clip's effects made: the picture, and how much larger it is than
/// the one they started from on each side, as shares of that one's width
/// and height (0 when nothing draws past its edges).
#[derive(Debug)]
pub struct Applied {
    pub picture: Frame,
    pub grown: (f32, f32),
}

/// The largest side a picture is given when room is made around it.
const LARGEST_PICTURE: u32 = 8192;

/// Where the pictures the passes write come from, and where those no
/// longer needed go (to be drawn into again after the queue is submitted).
pub struct Pictures<'a> {
    pub take: &'a mut dyn FnMut((u32, u32)) -> Frame,
    pub done: &'a mut Vec<Frame>,
}

impl std::fmt::Debug for Pictures<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pictures({} done)", self.done.len())
    }
}

impl Effects {
    pub fn new(device: &wgpu::Device, working: Working) -> Effects {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect"),
            entries: &[
                uniform_entry(PASS_BYTES),
                sampler_entry(),
                texture_entry(2),
                texture_entry(3),
            ],
        });
        let pipeline = pipeline(
            device,
            "effect",
            &layout,
            &module,
            ("whole", "effect"),
            working.format(),
            // Every pixel is written as worked out: nothing is blended.
            None,
        );
        Effects {
            working,
            pipeline,
            layout,
            sampler: smooth_sampler(device),
        }
    }

    /// Adds to `encoder` the passes of `effects`, one after another,
    /// starting from `picture` (made by a compositor in this working
    /// space). Returns the picture holding the result, which is larger
    /// than `picture` when an effect draws past its edges; every other
    /// picture used goes to `pictures.done`.
    pub fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        picture: Frame,
        effects: &[Effect],
        pictures: Pictures<'_>,
    ) -> Applied {
        let Pictures { take, done } = pictures;
        let first = picture.size();
        // Sizes stay those of the picture as it came, whatever room is
        // made around it.
        let reference = first.1.max(1) as f32 / 1080.0;
        let mut now = picture;
        // Where the picture as it came is in `now`, and how much room was
        // made on each side, in pixels.
        let mut within = Spot::WHOLE;
        let mut room = (0u32, 0u32);
        let mut pass = |encoder: &mut wgpu::CommandEncoder,
                        (from, with): (&Frame, Option<&Frame>),
                        size: (u32, u32),
                        pass: Pass,
                        seed: u32| {
            let mut to = take(size);
            let inputs = (from, with.unwrap_or(from));
            self.pass(device, queue, encoder, inputs, &mut to, (pass, seed, reference));
            to
        };
        for effect in effects {
            let reach = effect.reach();
            if reach > 0.0 {
                let (w, h) = now.size();
                let wanted = (reach * reference).ceil() as u32;
                let most = |side: u32| (LARGEST_PICTURE.saturating_sub(side) / 2).min(wanted);
                let more = (most(w), most(h));
                if more != (0, 0) {
                    let size = (w + 2 * more.0, h + 2 * more.1);
                    let to_new = |at: f32, side: u32, more: u32, new: u32| {
                        (at * side as f32 + more as f32) / new as f32
                    };
                    within = Spot {
                        left: to_new(within.left, w, more.0, size.0),
                        top: to_new(within.top, h, more.1, size.1),
                        right: to_new(within.right, w, more.0, size.0),
                        bottom: to_new(within.bottom, h, more.1, size.1),
                    };
                    let whole_before = Spot {
                        left: more.0 as f32 / size.0 as f32,
                        top: more.1 as f32 / size.1 as f32,
                        right: (more.0 + w) as f32 / size.0 as f32,
                        bottom: (more.1 + h) as f32 / size.1 as f32,
                    };
                    let roomy = pass(
                        encoder,
                        (&now, None),
                        size,
                        Pass::Room { within: whole_before },
                        0,
                    );
                    done.push(std::mem::replace(&mut now, roomy));
                    room = (room.0 + more.0, room.1 + more.1);
                }
            }
            let size = now.size();
            let next = match *effect {
                Effect::Vignette {
                    amount,
                    size: reach,
                    softness,
                } => pass(
                    encoder,
                    (&now, None),
                    size,
                    Pass::Vignette {
                        amount,
                        size: reach,
                        softness,
                    },
                    0,
                ),
                Effect::Grain {
                    amount,
                    size: cell,
                    seed,
                } => pass(
                    encoder,
                    (&now, None),
                    size,
                    Pass::Grain { amount, size: cell },
                    seed,
                ),
                Effect::Sharpen { amount, radius } => {
                    pass(encoder, (&now, None), size, Pass::Sharpen { amount, radius }, 0)
                }
                Effect::Glow {
                    amount,
                    threshold,
                    radius,
                } => {
                    let lit = pass(encoder, (&now, None), size, Pass::Bright { threshold }, 0);
                    let across = pass(encoder, (&lit, None), size, Pass::BlurAcross { radius }, 0);
                    let down = pass(encoder, (&across, None), size, Pass::BlurDown { radius }, 0);
                    let glowing = pass(encoder, (&now, Some(&down)), size, Pass::Add { amount }, 0);
                    done.extend([lit, across, down]);
                    glowing
                }
                Effect::Blur { radius, spot } => {
                    let across = pass(encoder, (&now, None), size, Pass::BlurAcross { radius }, 0);
                    let down = pass(encoder, (&across, None), size, Pass::BlurDown { radius }, 0);
                    let blurred = pass(encoder, (&now, Some(&down)), size, Pass::Within { spot }, 0);
                    done.extend([across, down]);
                    blurred
                }
                Effect::Pixelate { size: cell, spot } => pass(
                    encoder,
                    (&now, None),
                    size,
                    Pass::Pixelate { size: cell, spot },
                    0,
                ),
                Effect::Move {
                    scale,
                    fixed,
                    turn,
                    shift,
                } => {
                    let moving = Pass::Move {
                        scale,
                        fixed,
                        turn,
                        shift,
                    };
                    pass(encoder, (&now, None), size, moving, 0)
                }
                Effect::Grade(numbers) => pass(encoder, (&now, None), size, Pass::Grade(numbers), 0),
                Effect::Mirror { across, down } => {
                    let mirror = Pass::Mirror { across, down, within };
                    pass(encoder, (&now, None), size, mirror, 0)
                }
                Effect::Corners { radius } => {
                    pass(encoder, (&now, None), size, Pass::Corners { radius, within }, 0)
                }
                Effect::Outline { width, colour } => {
                    pass(encoder, (&now, None), size, Pass::Outline { width, colour }, 0)
                }
                Effect::Shadow {
                    amount,
                    softness,
                    offset,
                    colour,
                } => {
                    let cast = Pass::Silhouette {
                        amount,
                        offset,
                        colour,
                    };
                    let shape = pass(encoder, (&now, None), size, cast, 0);
                    let radius = softness;
                    let across = pass(encoder, (&shape, None), size, Pass::BlurAcross { radius }, 0);
                    let down = pass(encoder, (&across, None), size, Pass::BlurDown { radius }, 0);
                    let over = pass(encoder, (&now, Some(&down)), size, Pass::Over, 0);
                    done.extend([shape, across, down]);
                    over
                }
                Effect::Key {
                    tolerance,
                    softness,
                    spill,
                    colour,
                } => {
                    let key = Pass::Key {
                        tolerance,
                        softness,
                        spill,
                        colour,
                    };
                    pass(encoder, (&now, None), size, key, 0)
                }
                Effect::Letterbox { ratio } => {
                    pass(encoder, (&now, None), size, Pass::Letterbox { ratio, within }, 0)
                }
                Effect::Mono { amount } => pass(encoder, (&now, None), size, Pass::Mono { amount }, 0),
                Effect::Sepia { amount } => pass(encoder, (&now, None), size, Pass::Sepia { amount }, 0),
                Effect::Duotone {
                    amount,
                    shadows,
                    highlights,
                } => {
                    let two = Pass::Duotone {
                        amount,
                        shadows,
                        highlights,
                    };
                    pass(encoder, (&now, None), size, two, 0)
                }
                Effect::Invert { amount } => pass(encoder, (&now, None), size, Pass::Invert { amount }, 0),
                Effect::Split { offset } => pass(encoder, (&now, None), size, Pass::Split { offset }, 0),
                Effect::Smear { length, angle } => {
                    pass(encoder, (&now, None), size, Pass::Smear { length, angle }, 0)
                }
                Effect::Radial { amount, centre } => {
                    pass(encoder, (&now, None), size, Pass::Radial { amount, centre }, 0)
                }
                Effect::Glitch { amount, often, seed } => {
                    pass(encoder, (&now, None), size, Pass::Glitch { amount, often }, seed)
                }
            };
            done.push(std::mem::replace(&mut now, next));
        }
        let grown = (
            room.0 as f32 / first.0.max(1) as f32,
            room.1 as f32 / first.1.max(1) as f32,
        );
        Applied { picture: now, grown }
    }

    /// One pass from `inputs` (the picture, and a second one or the same
    /// again) into `to`.
    fn pass(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        (from, with): (&Frame, &Frame),
        to: &mut Frame,
        (pass, seed, reference): (Pass, u32, f32),
    ) {
        let (w, h) = to.size();
        let linear = if self.working == Working::Linear { 1.0 } else { 0.0 };
        // Each pass its own numbers: they are written when the queue is
        // submitted, after every pass has been recorded. The seed stays
        // below 2^24, where a float holds every whole number.
        let grade = pass.grade();
        let rows = [
            pass.numbers(),
            [linear, w as f32, h as f32, (seed % (1 << 24)) as f32],
            pass.extra(),
            [reference, 0.0, 0.0, 0.0],
            pass.paint(),
            grade[0],
            grade[1],
            grade[2],
            grade[3],
            grade[4],
            grade[5],
            grade[6],
        ];
        let curves = pass.curves();
        let limits = pass.limits();
        let bytes: Vec<u8> = rows
            .as_flattened()
            .iter()
            .chain(curves.as_flattened())
            .chain(limits.as_flattened())
            .flat_map(|n| n.to_ne_bytes())
            .collect();
        let uniforms = uniform_buffer(device, PASS_BYTES);
        queue.write_buffer(&uniforms, 0, &bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                texture_binding(2, from.view()),
                texture_binding(3, with.view()),
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("effect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: to.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..1);
    }
}
