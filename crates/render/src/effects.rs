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
        }
    }

    /// The pass's third row: a box, or a move's turn and shift.
    fn extra(self) -> [f32; 4] {
        match self {
            Pass::Within { spot } | Pass::Pixelate { spot, .. } => spot.numbers(),
            Pass::Move { turn, shift, .. } => [turn, shift.0, shift.1, 0.0],
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

    /// The pass's grade rows: a grade's numbers, plain for every other.
    fn grade(self) -> [[f32; 4]; 7] {
        match self {
            Pass::Grade(numbers) => numbers.rows(),
            _ => GradeNumbers::PLAIN.rows(),
        }
    }
}

const PASS_BYTES: u64 = 48 + 7 * 16 + CURVE_SAMPLES as u64 * 16;

/// The effect passes for pictures made in one working space.
#[derive(Debug)]
pub struct Effects {
    working: Working,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

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
    /// space). Returns the picture holding the result; every other picture
    /// used goes to `pictures.done`.
    pub fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        picture: Frame,
        effects: &[Effect],
        pictures: Pictures<'_>,
    ) -> Frame {
        let Pictures { take, done } = pictures;
        let size = picture.size();
        let mut now = picture;
        let mut pass = |encoder: &mut wgpu::CommandEncoder,
                        from: &Frame,
                        with: Option<&Frame>,
                        pass: Pass,
                        seed: u32| {
            let mut to = take(size);
            self.pass(
                device,
                queue,
                encoder,
                (from, with.unwrap_or(from)),
                &mut to,
                pass,
                seed,
            );
            to
        };
        for effect in effects {
            let next = match *effect {
                Effect::Vignette {
                    amount,
                    size,
                    softness,
                } => pass(
                    encoder,
                    &now,
                    None,
                    Pass::Vignette {
                        amount,
                        size,
                        softness,
                    },
                    0,
                ),
                Effect::Grain { amount, size, seed } => {
                    pass(encoder, &now, None, Pass::Grain { amount, size }, seed)
                }
                Effect::Sharpen { amount, radius } => {
                    pass(encoder, &now, None, Pass::Sharpen { amount, radius }, 0)
                }
                Effect::Glow {
                    amount,
                    threshold,
                    radius,
                } => {
                    let lit = pass(encoder, &now, None, Pass::Bright { threshold }, 0);
                    let across = pass(encoder, &lit, None, Pass::BlurAcross { radius }, 0);
                    let down = pass(encoder, &across, None, Pass::BlurDown { radius }, 0);
                    let glowing = pass(encoder, &now, Some(&down), Pass::Add { amount }, 0);
                    done.extend([lit, across, down]);
                    glowing
                }
                Effect::Blur { radius, spot } => {
                    let across = pass(encoder, &now, None, Pass::BlurAcross { radius }, 0);
                    let down = pass(encoder, &across, None, Pass::BlurDown { radius }, 0);
                    let blurred = pass(encoder, &now, Some(&down), Pass::Within { spot }, 0);
                    done.extend([across, down]);
                    blurred
                }
                Effect::Pixelate { size, spot } => {
                    pass(encoder, &now, None, Pass::Pixelate { size, spot }, 0)
                }
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
                    pass(encoder, &now, None, moving, 0)
                }
                Effect::Grade(numbers) => pass(encoder, &now, None, Pass::Grade(numbers), 0),
            };
            done.push(std::mem::replace(&mut now, next));
        }
        now
    }

    /// One pass from `inputs` (the picture, and a second one or the same
    /// again) into `to`.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        (from, with): (&Frame, &Frame),
        to: &mut Frame,
        pass: Pass,
        seed: u32,
    ) {
        let (w, h) = from.size();
        let linear = if self.working == Working::Linear { 1.0 } else { 0.0 };
        // Each pass its own numbers: they are written when the queue is
        // submitted, after every pass has been recorded. The seed stays
        // below 2^24, where a float holds every whole number.
        let grade = pass.grade();
        let rows = [
            pass.numbers(),
            [linear, w as f32, h as f32, (seed % (1 << 24)) as f32],
            pass.extra(),
            grade[0],
            grade[1],
            grade[2],
            grade[3],
            grade[4],
            grade[5],
            grade[6],
        ];
        let curves = pass.curves();
        let bytes: Vec<u8> = rows
            .as_flattened()
            .iter()
            .chain(curves.as_flattened())
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
