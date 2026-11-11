//! The compositor being measured: decoded pictures' planes laid into a frame,
//! in each of the ways the research note compares.

use crate::gpu::{Gpu, Stamps};
use std::time::{Duration, Instant};

const EIGHT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const FLOAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const LAYER_BYTES: u64 = 96;
const WHOLE_BYTES: u64 = 16;
/// The most layers any measurement lays into one frame.
pub const MOST_LAYERS: usize = 4;

/// How a picture is put together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// Layers straight onto the window, in display colours (approach A).
    Direct,
    /// Layers into an 8-bit frame in display colours, then the frame shown.
    Eight,
    /// Layers into a floating-point frame in linear light, then shown.
    Float,
    /// As `Float`, but every layer first into a target of its own, which is
    /// then laid over the frame: what an effect on every layer would force.
    FloatApart,
}

impl Way {
    pub const ALL: [Way; 4] = [Way::Direct, Way::Eight, Way::Float, Way::FloatApart];

    pub fn name(self) -> &'static str {
        match self {
            Way::Direct => "A straight to the window",
            Way::Eight => "8-bit frame",
            Way::Float => "float frame",
            Way::FloatApart => "float, a target per layer",
        }
    }

    fn linear(self) -> bool {
        matches!(self, Way::Float | Way::FloatApart)
    }

    /// Passes for a picture of `layers` layers, with or without showing it.
    fn passes(self, layers: usize, shown: bool) -> usize {
        let show = usize::from(shown);
        match self {
            Way::Direct => 1,
            Way::Eight | Way::Float => 1 + show,
            Way::FloatApart => layers * 2 + show,
        }
    }
}

/// A decoded picture as the zero-copy path hands it over: brightness at full
/// size, the two colour samples interleaved at half size.
pub struct Source {
    pub luma: wgpu::TextureView,
    pub chroma: wgpu::TextureView,
}

impl Source {
    /// A picture of `size` with a pattern that depends on `seed`, so no two
    /// layers are the same texture.
    pub fn pattern(gpu: &Gpu, (width, height): (u32, u32), seed: u32) -> Source {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let luma = gpu.texture("luma", (width, height), wgpu::TextureFormat::R8Unorm, usage);
        let half = (width.div_ceil(2), height.div_ceil(2));
        let chroma = gpu.texture("chroma", half, wgpu::TextureFormat::Rg8Unorm, usage);
        let mut bytes = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                // Diagonal bands with some grain: detail everywhere, as a recording has.
                let band = ((x + y * 2 + seed * 97) / 24) % 8;
                let grain = (x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ seed) % 13;
                bytes.push((40 + band * 22 + grain) as u8);
            }
        }
        gpu.write(&luma, &bytes, 1);
        bytes.clear();
        for y in 0..half.1 {
            for x in 0..half.0 {
                bytes.push((96 + (x / 16 + seed * 5) % 64) as u8);
                bytes.push((96 + (y / 16 + seed * 11) % 64) as u8);
            }
        }
        gpu.write(&chroma, &bytes, 2);
        Source::of(&luma, &chroma)
    }

    /// A picture of one colour.
    pub fn flat(gpu: &Gpu, (width, height): (u32, u32), yuv: [u8; 3]) -> Source {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let luma = gpu.texture("luma", (width, height), wgpu::TextureFormat::R8Unorm, usage);
        let half = (width.div_ceil(2), height.div_ceil(2));
        let chroma = gpu.texture("chroma", half, wgpu::TextureFormat::Rg8Unorm, usage);
        gpu.write(&luma, &vec![yuv[0]; (width * height) as usize], 1);
        gpu.write(&chroma, &[yuv[1], yuv[2]].repeat((half.0 * half.1) as usize), 2);
        Source::of(&luma, &chroma)
    }

    fn of(luma: &wgpu::Texture, chroma: &wgpu::Texture) -> Source {
        Source {
            luma: luma.create_view(&wgpu::TextureViewDescriptor::default()),
            chroma: chroma.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }
}

/// Where a layer sits in the frame, all in fractions of the frame.
#[derive(Clone, Copy)]
pub struct Place {
    pub centre: (f32, f32),
    pub half: (f32, f32),
    pub degrees: f32,
    pub opacity: f32,
    /// The part of the picture shown: left, top, width, height in 0-1.
    pub crop: [f32; 4],
}

impl Place {
    pub const FULL: Place = Place {
        centre: (0.5, 0.5),
        half: (0.5, 0.5),
        degrees: 0.0,
        opacity: 1.0,
        crop: [0.0, 0.0, 1.0, 1.0],
    };
    /// A facecam in a corner: scaled down and cropped.
    pub const FACECAM: Place = Place {
        centre: (0.82, 0.78),
        half: (0.15, 0.15),
        degrees: 0.0,
        opacity: 1.0,
        crop: [0.2, 0.1, 0.6, 0.8],
    };
    /// A second full picture mixed in at half strength: every pixel drawn twice.
    pub const MIXED: Place = Place {
        opacity: 0.5,
        ..Place::FULL
    };
    /// A smaller picture, turned a little.
    pub const TURNED: Place = Place {
        centre: (0.27, 0.3),
        half: (0.2, 0.2),
        degrees: 8.0,
        opacity: 0.9,
        crop: [0.0, 0.0, 1.0, 1.0],
    };

    /// The places of the first `count` layers of the standard picture:
    /// a recording; with a facecam; with a mixed-in picture and a turned one.
    pub fn standard(count: usize) -> Vec<Place> {
        let all = match count {
            0..=2 => vec![Place::FULL, Place::FACECAM],
            _ => vec![Place::FULL, Place::MIXED, Place::FACECAM, Place::TURNED],
        };
        all.into_iter().take(count.max(1)).collect()
    }

    /// The shader's `Layer` block for this place in a frame of `frame` pixels.
    fn uniform(self, (width, height): (u32, u32), linear: bool) -> Vec<u8> {
        let (w, h) = (width as f32, height as f32);
        let (sin, cos) = self.degrees.to_radians().sin_cos();
        // In frame pixels, so a turn keeps its shape, then to clip space.
        let turn = |x: f32, y: f32| (x * cos - y * sin, x * sin + y * cos);
        let (hw, hh) = (self.half.0 * w, self.half.1 * h);
        let corner = turn(-hw, -hh);
        let origin = (self.centre.0 * w + corner.0, self.centre.1 * h + corner.1);
        let across = turn(2.0 * hw, 0.0);
        let down = turn(0.0, 2.0 * hh);
        let clip = |(x, y): (f32, f32)| (x / w * 2.0, -y / h * 2.0);
        let origin = (origin.0 / w * 2.0 - 1.0, 1.0 - origin.1 / h * 2.0);
        let (across, down) = (clip(across), clip(down));
        // BT.709, limited range.
        let (y, black, none) = (255.0 / 219.0, 16.0 / 255.0, 128.0 / 255.0);
        let (rv, gu, gv, bu) = (1.792_74, -0.213_25, -0.532_91, 2.112_40);
        let rows = [
            [y, 0.0, rv, -y * black - rv * none],
            [y, gu, gv, -y * black - (gu + gv) * none],
            [y, bu, 0.0, -y * black - bu * none],
            [origin.0, origin.1, across.0, across.1],
            [down.0, down.1, self.opacity, if linear { 1.0 } else { 0.0 }],
            self.crop,
        ];
        rows.iter()
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }
}

/// The pipelines, made once.
pub struct Compositor {
    layer_layout: wgpu::BindGroupLayout,
    whole_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    layer_to_eight: wgpu::RenderPipeline,
    layer_to_float: wgpu::RenderPipeline,
    over_float: wgpu::RenderPipeline,
    show: wgpu::RenderPipeline,
    /// How long making the shaders and pipelines took.
    pub made_in: Duration,
}

impl Compositor {
    pub fn new(gpu: &Gpu) -> Compositor {
        let start = Instant::now();
        let device = &gpu.device;
        let module = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let layer_module = module("layer", include_str!("compose.wgsl"));
        let whole_module = module("whole", include_str!("whole.wgsl"));
        let uniform = |bytes| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(bytes),
            },
            count: None,
        };
        let sampler_entry = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[uniform(LAYER_BYTES), sampler_entry, texture(2), texture(3)],
        });
        let whole_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("whole"),
            entries: &[uniform(WHOLE_BYTES), sampler_entry, texture(2)],
        });
        let pipeline = |label, layout: &wgpu::BindGroupLayout, module, vertex, fragment, target, blend| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let layer_to_eight = pipeline(
            "layer to 8-bit",
            &layer_layout,
            &layer_module,
            "place",
            "layer",
            EIGHT,
            over,
        );
        let layer_to_float = pipeline(
            "layer to float",
            &layer_layout,
            &layer_module,
            "place",
            "layer",
            FLOAT,
            over,
        );
        let over_float = pipeline("over", &whole_layout, &whole_module, "whole", "over", FLOAT, over);
        let show = pipeline("show", &whole_layout, &whole_module, "whole", "over", EIGHT, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("smooth"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Compositor {
            layer_layout,
            whole_layout,
            sampler,
            layer_to_eight,
            layer_to_float,
            over_float,
            show,
            made_in: start.elapsed(),
        }
    }

    /// Adds the passes that put `layers` together in `way` to `encoder`:
    /// into `targets`' frame, and from there onto `window` when one is given.
    /// (`Way::Direct` has no frame: it needs the window.)
    pub fn draw(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        way: Way,
        targets: &Targets,
        layers: &[(&Source, Place)],
        window: Option<&wgpu::TextureView>,
        stamps: Option<(&Stamps, u32)>,
    ) {
        let passes = way.passes(layers.len(), window.is_some());
        let mut pass_number = 0;
        let mut next_stamps = || {
            let writes = stamps.and_then(|(stamps, picture)| stamps.writes(picture, pass_number, passes));
            pass_number += 1;
            writes
        };
        // The decoder hands over new textures for every picture, so the
        // bind groups are made new every time, as the real thing must.
        let groups: Vec<wgpu::BindGroup> = layers
            .iter()
            .zip(&targets.layer_uniforms)
            .map(|((source, place), uniforms)| {
                gpu.queue
                    .write_buffer(uniforms, 0, &place.uniform(targets.size, way.linear()));
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("layer"),
                    layout: &self.layer_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniforms.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&source.luma),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&source.chroma),
                        },
                    ],
                })
            })
            .collect();
        let layer_pipeline = if way.linear() {
            &self.layer_to_float
        } else {
            &self.layer_to_eight
        };
        match way {
            Way::Direct | Way::Eight | Way::Float => {
                let into = match (way, window) {
                    (Way::Direct, Some(window)) => window,
                    _ => &targets.frame,
                };
                let mut pass = begin(encoder, into, Some(wgpu::Color::BLACK), next_stamps());
                pass.set_pipeline(layer_pipeline);
                for group in &groups {
                    pass.set_bind_group(0, group, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
            Way::FloatApart => {
                for (index, group) in groups.iter().enumerate() {
                    {
                        let mut pass = begin(
                            encoder,
                            &targets.apart,
                            Some(wgpu::Color::TRANSPARENT),
                            next_stamps(),
                        );
                        pass.set_pipeline(layer_pipeline);
                        pass.set_bind_group(0, group, &[]);
                        pass.draw(0..4, 0..1);
                    }
                    let clear = (index == 0).then_some(wgpu::Color::BLACK);
                    let mut pass = begin(encoder, &targets.frame, clear, next_stamps());
                    pass.set_pipeline(&self.over_float);
                    pass.set_bind_group(0, &targets.lay_apart, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
        }
        if let (Some(window), true) = (window, way != Way::Direct) {
            let mut pass = begin(encoder, window, Some(wgpu::Color::BLACK), next_stamps());
            self.show_in(&mut pass, targets);
        }
    }

    /// Draws `targets`' frame over the whole of a pass onto the window.
    pub fn show_in(&self, pass: &mut wgpu::RenderPass<'_>, targets: &Targets) {
        pass.set_pipeline(&self.show);
        pass.set_bind_group(0, &targets.show_frame, &[]);
        pass.draw(0..4, 0..1);
    }
}

pub fn begin<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        timestamp_writes,
        ..Default::default()
    })
}

/// What a compositor keeps between pictures for one frame size and one way:
/// the frame, a spare target, and the small buffers the shaders read.
pub struct Targets {
    pub size: (u32, u32),
    pub frame: wgpu::TextureView,
    apart: wgpu::TextureView,
    layer_uniforms: Vec<wgpu::Buffer>,
    lay_apart: wgpu::BindGroup,
    show_frame: wgpu::BindGroup,
}

impl Targets {
    pub fn new(gpu: &Gpu, compositor: &Compositor, way: Way, size: (u32, u32)) -> Targets {
        let format = if way.linear() { FLOAT } else { EIGHT };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let view = |label, size| {
            gpu.texture(label, size, format, usage)
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let frame = view("frame", size);
        // Only one way uses the spare target: the others get a token one.
        let apart = view("apart", if way == Way::FloatApart { size } else { (4, 4) });
        let buffer = |bytes| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("uniforms"),
                size: bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let whole = |picture: &wgpu::TextureView, to_video_values: bool| {
            let uniforms = buffer(WHOLE_BYTES);
            let flags = [if to_video_values { 1.0f32 } else { 0.0 }, 0.0, 0.0, 0.0];
            let bytes: Vec<u8> = flags.iter().flat_map(|value| value.to_le_bytes()).collect();
            gpu.queue.write_buffer(&uniforms, 0, &bytes);
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("whole"),
                layout: &compositor.whole_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&compositor.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(picture),
                    },
                ],
            })
        };
        Targets {
            size,
            lay_apart: whole(&apart, false),
            show_frame: whole(&frame, way.linear()),
            frame,
            apart,
            layer_uniforms: (0..MOST_LAYERS).map(|_| buffer(LAYER_BYTES)).collect(),
        }
    }
}

/// A stand-in for the window: an 8-bit target of `size`.
pub fn window(gpu: &Gpu, size: (u32, u32)) -> wgpu::Texture {
    gpu.texture(
        "window",
        size,
        EIGHT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    )
}

/// The colour of the pixel at `at` of an 8-bit target.
pub fn pixel(gpu: &Gpu, target: &wgpu::Texture, at: (u32, u32)) -> Result<[u8; 4], String> {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pixel"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: at.0,
                y: at.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let bytes = gpu.read(&buffer)?;
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}
