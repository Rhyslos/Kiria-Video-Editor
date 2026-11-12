//! The GPU side of a video picture: textures for the decoder's YUV planes
//! and the pipeline whose shader (`yuv_to_rgb.wgsl`) turns them into RGB
//! while drawing. Plain wgpu, nothing of the interface toolkit, so the same
//! renderer can draw into any target.
//!
//! A picture comes one of two ways. Planes in memory are written into the
//! renderer's own textures, which are made again only when the picture's
//! size or layout changes. Planes that are textures already
//! ([`PlaneTextures`]: a picture decoded on the graphics card) are drawn
//! from as they are, with nothing written at all.

use crate::video_picture::{ChromaLayout, PlaneTextures, YuvImage, texture_axes};
use egui_wgpu::wgpu;
use render::WHOLE;

/// What the textures in use were made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    luma: (u32, u32),
    chroma: (u32, u32),
    layout: ChromaLayout,
}

#[derive(Debug)]
struct Textures {
    shape: Shape,
    /// Made here, to be written into; not someone else's, only to be read.
    own: bool,
    luma: wgpu::Texture,
    /// One two-channel texture when interleaved, else U and V.
    chroma: Vec<wgpu::Texture>,
    bind_group: wgpu::BindGroup,
}

#[derive(Debug)]
pub struct YuvRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// The target stores sRGB itself, so the shader hands it linear values.
    linear_target: bool,
    textures: Option<Textures>,
    /// The picture's colour settings, turn and layout, kept to write the
    /// shader's block again when the part shown changes.
    settings: Option<([[f32; 4]; 3], u16, ChromaLayout)>,
    /// The part of the picture drawn ([`Self::show_part`]).
    part: [f32; 4],
}

/// Five rows of four numbers: see `Picture` in the shader.
const UNIFORM_BYTES: u64 = 5 * 4 * 4;

impl YuvRenderer {
    /// A renderer drawing into targets of `target` format, one sample a
    /// pixel and no depth buffer (what the window's render pass has).
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> YuvRenderer {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video picture"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv_to_rgb.wgsl").into()),
        });
        let plane = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video picture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                plane(2),
                plane(3),
                plane(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video picture"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video picture"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("corner"),
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
                module: &module,
                entry_point: Some("convert"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video picture"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video picture"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        YuvRenderer {
            pipeline,
            layout,
            sampler,
            uniforms,
            linear_target: target.is_srgb(),
            textures: None,
            settings: None,
            part: WHOLE,
        }
    }

    /// Takes in a picture: its planes go into the textures (made new when
    /// the size or layout changed) and its colour settings to the shader.
    /// False, and nothing left to draw, when the picture can't be shown:
    /// planes that don't match the sizes given, or sizes beyond what the
    /// graphics device takes.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &YuvImage) -> bool {
        let shape = Shape {
            luma: image.luma_size,
            chroma: image.chroma_size(),
            layout: image.layout,
        };
        let bytes = image.planes.as_ref().as_ref();
        let largest = device.limits().max_texture_dimension_2d;
        let fits = |(w, h): (u32, u32)| (1..=largest).contains(&w) && (1..=largest).contains(&h);
        if !fits(shape.luma) || !fits(shape.chroma) || bytes.len() != image.bytes_needed() {
            self.textures = None;
            return false;
        }
        if self.textures.as_ref().is_none_or(|t| t.shape != shape || !t.own) {
            self.textures = Some(self.textures_for(device, shape));
        }
        let Some(textures) = &self.textures else {
            return false;
        };
        let (luma, chroma) = bytes.split_at(shape.luma.0 as usize * shape.luma.1 as usize);
        write_plane(queue, &textures.luma, luma, 1);
        match shape.layout {
            ChromaLayout::Interleaved => write_plane(queue, &textures.chroma[0], chroma, 2),
            ChromaLayout::Planar => {
                let (u, v) = chroma.split_at(chroma.len() / 2);
                write_plane(queue, &textures.chroma[0], u, 1);
                write_plane(queue, &textures.chroma[1], v, 1);
            }
        }
        self.settings = Some((image.to_rgb, image.rotation, image.layout));
        self.write_settings(queue);
        true
    }

    /// Takes in a picture whose planes are textures already: they are drawn
    /// from directly and kept until the next picture. False, and nothing
    /// left to draw, when they aren't a one-byte luma texture with a
    /// two-byte colour texture of half its size.
    pub fn show(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, planes: &PlaneTextures) -> bool {
        let luma = (planes.luma.width(), planes.luma.height());
        let shape = Shape {
            luma,
            chroma: (luma.0.div_ceil(2), luma.1.div_ceil(2)),
            layout: ChromaLayout::Interleaved,
        };
        let fits = planes.luma.format() == wgpu::TextureFormat::R8Unorm
            && planes.chroma.format() == wgpu::TextureFormat::Rg8Unorm
            && (planes.chroma.width(), planes.chroma.height()) == shape.chroma;
        if !fits {
            self.textures = None;
            return false;
        }
        let bind_group = self.bind_group(device, &planes.luma, &planes.chroma, &planes.chroma);
        self.textures = Some(Textures {
            shape,
            own: false,
            luma: planes.luma.clone(),
            chroma: vec![planes.chroma.clone()],
            bind_group,
        });
        self.settings = Some((planes.to_rgb, planes.rotation, ChromaLayout::Interleaved));
        self.write_settings(queue);
        true
    }

    /// Draws only `part` of the picture from now on (left, top, right and
    /// bottom of the upright picture, 0 to 1 across and down; [`WHOLE`] for
    /// all of it), stretched over the viewport: for a zoomed-in viewer,
    /// which draws what is seen.
    pub fn show_part(&mut self, queue: &wgpu::Queue, part: [f32; 4]) {
        if self.part != part {
            self.part = part;
            self.write_settings(queue);
        }
    }

    fn write_settings(&self, queue: &wgpu::Queue) {
        if let Some((to_rgb, rotation, layout)) = self.settings {
            let settings = uniform_bytes(to_rgb, rotation, layout, self.linear_target, self.part);
            queue.write_buffer(&self.uniforms, 0, &settings);
        }
    }

    /// Whether a picture was taken in and can be drawn.
    pub fn has_picture(&self) -> bool {
        self.textures.is_some()
    }

    /// Forgets the picture, so nothing is drawn until the next one.
    pub fn clear(&mut self) {
        self.textures = None;
    }

    /// Draws the picture over the whole viewport of `pass`.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(textures) = &self.textures {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &textures.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
    }

    fn textures_for(&self, device: &wgpu::Device, shape: Shape) -> Textures {
        let texture = |(width, height): (u32, u32), format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video picture plane"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let luma = texture(shape.luma, wgpu::TextureFormat::R8Unorm);
        let chroma = match shape.layout {
            ChromaLayout::Interleaved => vec![texture(shape.chroma, wgpu::TextureFormat::Rg8Unorm)],
            ChromaLayout::Planar => vec![
                texture(shape.chroma, wgpu::TextureFormat::R8Unorm),
                texture(shape.chroma, wgpu::TextureFormat::R8Unorm),
            ],
        };
        // Interleaved: the shader reads both channels from the one texture.
        let bind_group = self.bind_group(device, &luma, &chroma[0], chroma.last().unwrap_or(&chroma[0]));
        Textures {
            shape,
            own: true,
            luma,
            chroma,
            bind_group,
        }
    }

    /// The shader's inputs for a picture in these textures.
    fn bind_group(
        &self,
        device: &wgpu::Device,
        luma: &wgpu::Texture,
        u: &wgpu::Texture,
        v: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (luma_view, u_view, v_view) = (view(luma), view(u), view(v));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video picture"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&u_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&v_view),
                },
            ],
        })
    }
}

/// Writes one plane's rows (no padding) into its texture.
fn write_plane(queue: &wgpu::Queue, texture: &wgpu::Texture, bytes: &[u8], bytes_per_sample: u32) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * bytes_per_sample),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}

/// The shader's `Picture` block: the conversion, the turn and the part of
/// the upright picture drawn, how the colour planes are stored and what
/// kind of target is drawn into.
fn uniform_bytes(
    to_rgb: [[f32; 4]; 3],
    rotation: u16,
    layout: ChromaLayout,
    linear_target: bool,
    part: [f32; 4],
) -> Vec<u8> {
    let [origin, right, below] = texture_axes(rotation);
    let [left, top, end, bottom] = part;
    // The part's top left corner, and the steps to its other corners, in
    // the stored picture.
    let origin = [
        origin[0] + left * right[0] + top * below[0],
        origin[1] + left * right[1] + top * below[1],
    ];
    let across = [(end - left) * right[0], (end - left) * right[1]];
    let down = [(bottom - top) * below[0], (bottom - top) * below[1]];
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let rows = [
        to_rgb[0],
        to_rgb[1],
        to_rgb[2],
        [origin[0], origin[1], across[0], across[1]],
        [
            down[0],
            down[1],
            flag(layout == ChromaLayout::Interleaved),
            flag(linear_target),
        ],
    ];
    rows.iter().flatten().flat_map(|n| n.to_ne_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_block_is_five_rows_of_four() {
        let to_rgb = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
        ];
        let bytes = uniform_bytes(to_rgb, 90, ChromaLayout::Interleaved, true, WHOLE);
        assert_eq!(bytes.len() as u64, UNIFORM_BYTES);
        let numbers: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(
            numbers[..12],
            [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]
        );
        // A quarter turn: the top left of the rectangle is the stored
        // picture's bottom left; going right goes up the stored picture.
        assert_eq!(numbers[12..16], [0.0, 1.0, 0.0, -1.0]);
        assert_eq!(numbers[16..], [1.0, 0.0, 1.0, 1.0], "interleaved, linear target");
        let bytes = uniform_bytes(to_rgb, 0, ChromaLayout::Planar, false, WHOLE);
        assert_eq!(bytes[72..], [0u8; 8], "both flags off");
    }

    #[test]
    fn a_part_of_the_picture_is_read_from_its_place_in_the_stored_one() {
        let numbers = |rotation, part| -> Vec<f32> {
            uniform_bytes([[0.0; 4]; 3], rotation, ChromaLayout::Planar, false, part)
                .chunks_exact(4)
                .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect()
        };
        // Upright: the right half, top half only.
        let upright = numbers(0, [0.5, 0.0, 1.0, 0.5]);
        assert_eq!(
            upright[12..16],
            [0.5, 0.0, 0.5, 0.0],
            "starts half way across, half as wide"
        );
        assert_eq!(upright[16..18], [0.0, 0.5], "half as deep");
        // Turned a quarter: the upright left half is the stored bottom half.
        let turned = numbers(90, [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(turned[12..16], [0.0, 1.0, 0.0, -0.5]);
        assert_eq!(turned[16..18], [1.0, 0.0]);
    }
}
