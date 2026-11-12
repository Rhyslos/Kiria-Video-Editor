//! Puts a frame together on the graphics card: every layer's pixels drawn
//! into one picture of the timeline's shape, bottom layer first, and that
//! picture shown. Plain wgpu, nothing of the interface toolkit, so it draws
//! the same for the viewer, for export and in tests.
//!
//! [`Compositor`] holds the pipelines (made once), [`Frame`] a picture being
//! put together and what is kept between pictures of that size, and
//! [`Shower`] draws a finished frame into whatever target shows it.

use crate::geometry::{Area, Colour, Part, WEIGHTS};

/// Ten rows of four numbers: see `Layer` in the shader.
const LAYER_BYTES: u64 = 10 * 4 * 4;
/// One row of four: see `Show` in the shader.
const SHOW_BYTES: u64 = 4 * 4;

/// What a frame is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Working {
    /// Linear light in 16-bit floating point: mixing and scaling are right.
    Linear,
    /// The layers' own display values in 8 bits: half the memory.
    Display,
}

impl Working {
    pub fn format(self) -> wgpu::TextureFormat {
        match self {
            Working::Linear => wgpu::TextureFormat::Rgba16Float,
            Working::Display => wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    fn flag(self) -> f32 {
        match self {
            Working::Linear => 1.0,
            Working::Display => 0.0,
        }
    }
}

/// A decoded picture's pixels, as textures on the compositor's device.
#[derive(Debug, Clone, Copy)]
pub enum Pixels<'a> {
    /// Brightness, and the two colour samples together in one texture of
    /// half the size (how pictures decoded on the graphics card arrive).
    TwoPlanes {
        luma: &'a wgpu::TextureView,
        chroma: &'a wgpu::TextureView,
    },
    /// Brightness and one texture for each colour sample.
    ThreePlanes {
        luma: &'a wgpu::TextureView,
        u: &'a wgpu::TextureView,
        v: &'a wgpu::TextureView,
    },
    /// Red, green, blue and alpha in display values.
    Rgba(&'a wgpu::TextureView),
}

/// One layer to draw: its pixels and where they go.
#[derive(Debug, Clone, Copy)]
pub struct Drawn<'a> {
    pub pixels: Pixels<'a>,
    /// Red, green and blue as weights of (Y, U, V, 1), samples in 0-1.
    /// Not used for RGBA.
    pub to_rgb: [[f32; 4]; 3],
    /// Clockwise turn that shows the stored picture upright: 0, 90, 180, 270.
    pub rotation: u16,
    /// Where the shown part lands in the frame, in frame pixels.
    pub area: Area,
    /// The part of the upright picture shown.
    pub part: Part,
    pub opacity: f32,
    /// Quick colour; [`Colour::NEUTRAL`] leaves the pixels untouched.
    pub colour: Colour,
    /// A second picture mixed into the first, in linear light, and its
    /// share (0 to 1): the next recorded frame, for frame blending. It
    /// must be stored the same way as `pixels` (the same kind of planes,
    /// colour and turn), or it is left out.
    pub blend: Option<(Pixels<'a>, f32)>,
}

/// The pipelines that draw layers into frames of one working format.
#[derive(Debug)]
pub struct Compositor {
    working: Working,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, working: Working) -> Compositor {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("layer.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                uniform_entry(LAYER_BYTES),
                sampler_entry(),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                texture_entry(6),
                texture_entry(7),
            ],
        });
        let pipeline = pipeline(
            device,
            "layer",
            &layout,
            &module,
            ("place", "layer"),
            working.format(),
            // Layers come premultiplied by their opacity.
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        Compositor {
            working,
            pipeline,
            layout,
            sampler: smooth_sampler(device),
        }
    }

    pub fn working(&self) -> Working {
        self.working
    }

    /// A frame of `size` pixels to draw into, again and again.
    pub fn frame(&self, device: &wgpu::Device, size: (u32, u32)) -> Frame {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.working.format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Frame {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            texture,
            working: self.working,
            uniforms: Vec::new(),
        }
    }

    /// Adds to `encoder` the pass that draws `layers` into `frame`, bottom
    /// layer first, over black.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &mut Frame,
        layers: &[Drawn<'_>],
    ) {
        let size = (frame.texture.width(), frame.texture.height());
        while frame.uniforms.len() < layers.len() {
            frame.uniforms.push(uniform_buffer(device, LAYER_BYTES));
        }
        // Decoded pictures are new textures every time, so the groups are too.
        let groups: Vec<wgpu::BindGroup> = layers
            .iter()
            .zip(&frame.uniforms)
            .map(|(layer, uniforms)| {
                queue.write_buffer(uniforms, 0, &layer_bytes(layer, size, self.working));
                let (first, second, third) = views(layer.pixels);
                // Without a picture to blend, the first one stands in (unused).
                let (next_first, next_second, next_third) =
                    blended(layer).map_or((first, second, third), |(next, _)| views(next));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("layer"),
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
                        texture_binding(2, first),
                        texture_binding(3, second),
                        texture_binding(4, third),
                        texture_binding(5, next_first),
                        texture_binding(6, next_second),
                        texture_binding(7, next_third),
                    ],
                })
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &frame.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        for group in &groups {
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}

/// A picture of the timeline being put together, and what is kept between
/// pictures of the same size.
#[derive(Debug)]
pub struct Frame {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    working: Working,
    /// One small buffer a layer, reused from picture to picture.
    uniforms: Vec<wgpu::Buffer>,
}

impl Frame {
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }

    pub fn working(&self) -> Working {
        self.working
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

/// Draws finished frames into targets of one format: the window, or an
/// 8-bit picture to save or to test against.
#[derive(Debug)]
pub struct Shower {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The target stores sRGB itself, so the shader hands it linear values.
    linear_target: bool,
}

/// A frame made ready to show: bind it once, draw it in as many passes as needed.
#[derive(Debug)]
pub struct Showing {
    group: wgpu::BindGroup,
}

impl Shower {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Shower {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("show"),
            source: wgpu::ShaderSource::Wgsl(include_str!("show.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("show"),
            entries: &[uniform_entry(SHOW_BYTES), sampler_entry(), texture_entry(2)],
        });
        Shower {
            pipeline: pipeline(device, "show", &layout, &module, ("whole", "show"), target, None),
            layout,
            sampler: smooth_sampler(device),
            linear_target: target.is_srgb(),
        }
    }

    /// Makes a frame (its view, and what it was made in) ready to draw.
    pub fn showing(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &wgpu::TextureView,
        working: Working,
    ) -> Showing {
        let uniforms = uniform_buffer(device, SHOW_BYTES);
        let flags = [
            working.flag(),
            if self.linear_target { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ];
        queue.write_buffer(&uniforms, 0, &bytes(&flags));
        Showing {
            group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("show"),
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
                    texture_binding(2, frame),
                ],
            }),
        }
    }

    /// Draws the frame over the whole viewport of `pass`.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, showing: &Showing) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &showing.group, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// Where the corners of an upright rectangle fall in the stored picture, for
/// a picture shown turned clockwise by `rotation`: the texture position of
/// the rectangle's top left corner, then the step to its top right, then the
/// step to its bottom left. Positions run 0 to 1 across and down.
pub fn texture_axes(rotation: u16) -> [[f32; 2]; 3] {
    match rotation % 360 {
        90 => [[0.0, 1.0], [0.0, -1.0], [1.0, 0.0]],
        180 => [[1.0, 1.0], [-1.0, 0.0], [0.0, -1.0]],
        270 => [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]],
        _ => [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
    }
}

/// The textures a picture's pixels are sampled from, in binding order.
fn views(pixels: Pixels<'_>) -> (&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView) {
    match pixels {
        Pixels::TwoPlanes { luma, chroma } => (luma, chroma, chroma),
        Pixels::ThreePlanes { luma, u, v } => (luma, u, v),
        Pixels::Rgba(rgba) => (rgba, rgba, rgba),
    }
}

/// The layer's second picture and its share, when it has one stored the
/// same way as the first and a share that changes anything.
fn blended<'a>(layer: &Drawn<'a>) -> Option<(Pixels<'a>, f32)> {
    let (next, share) = layer.blend?;
    let same_kind = std::mem::discriminant(&next) == std::mem::discriminant(&layer.pixels);
    (same_kind && share > 0.0).then_some((next, share.min(1.0)))
}

/// The shader's `Layer` block for a layer in a frame of `frame` pixels.
fn layer_bytes(layer: &Drawn<'_>, frame: (u32, u32), working: Working) -> Vec<u8> {
    // The shown part of the upright picture, then turned into the stored one.
    let [origin, across, down] = texture_axes(layer.rotation);
    let part = layer.part;
    let from_origin = [
        origin[0] + part.left * across[0] + part.top * down[0],
        origin[1] + part.left * across[1] + part.top * down[1],
    ];
    let from_across = [part.width * across[0], part.width * across[1]];
    let from_down = [part.height * down[0], part.height * down[1]];
    // Frame pixels (y down) to clip space (y up, -1 to 1).
    let (w, h) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let step = |(x, y): (f32, f32)| [x / w * 2.0, -y / h * 2.0];
    let area = layer.area;
    let to_origin = [area.origin.0 / w * 2.0 - 1.0, 1.0 - area.origin.1 / h * 2.0];
    let (to_across, to_down) = (step(area.across), step(area.down));
    let kind = match layer.pixels {
        Pixels::TwoPlanes { .. } => 0.0,
        Pixels::ThreePlanes { .. } => 1.0,
        Pixels::Rgba(_) => 2.0,
    };
    let rows = [
        layer.to_rgb[0],
        layer.to_rgb[1],
        layer.to_rgb[2],
        [from_origin[0], from_origin[1], from_across[0], from_across[1]],
        [from_down[0], from_down[1], kind, working.flag()],
        [to_origin[0], to_origin[1], to_across[0], to_across[1]],
        [
            to_down[0],
            to_down[1],
            layer.opacity.clamp(0.0, 1.0),
            if layer.colour.is_neutral() { 0.0 } else { 1.0 },
        ],
        [
            layer.colour.gains[0],
            layer.colour.gains[1],
            layer.colour.gains[2],
            layer.colour.saturation,
        ],
        [WEIGHTS[0], WEIGHTS[1], WEIGHTS[2], layer.colour.contrast],
        [blended(layer).map_or(0.0, |(_, share)| share), 0.0, 0.0, 0.0],
    ];
    bytes(rows.as_flattened())
}

fn bytes(numbers: &[f32]) -> Vec<u8> {
    numbers.iter().flat_map(|n| n.to_ne_bytes()).collect()
}

pub(crate) fn uniform_entry(size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(size),
        },
        count: None,
    }
}

pub(crate) fn sampler_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 1,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

pub(crate) fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(crate) fn uniform_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn smooth_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("smooth"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// A pipeline drawing one rectangle (a strip of two triangles) a draw, into
/// targets of `target` format with one sample a pixel and no depth buffer.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    (vertex, fragment): (&str, &str),
    target: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
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
}

#[cfg(test)]
mod tests;
