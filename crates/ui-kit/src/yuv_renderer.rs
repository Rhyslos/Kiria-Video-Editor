//! The GPU side of a video picture: textures for the decoder's YUV planes
//! and the pipeline whose shader (`yuv_to_rgb.wgsl`) turns them into RGB
//! while drawing. Plain wgpu, nothing of the interface toolkit, so the same
//! renderer can draw into any target.
//!
//! Textures are made again only when the picture's size or layout changes;
//! otherwise a new picture is written into the ones there.

use crate::video_picture::{ChromaLayout, YuvImage, texture_axes};
use egui_wgpu::wgpu;

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
        if self.textures.as_ref().is_none_or(|t| t.shape != shape) {
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
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes(image, self.linear_target));
        true
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
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (luma_view, u_view) = (view(&luma), view(&chroma[0]));
        // Interleaved: the shader reads both channels from the one texture.
        let v_view = view(chroma.last().unwrap_or(&chroma[0]));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        });
        Textures {
            shape,
            luma,
            chroma,
            bind_group,
        }
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

/// The shader's `Picture` block for an image.
fn uniform_bytes(image: &YuvImage, linear_target: bool) -> Vec<u8> {
    let [origin, across, down] = texture_axes(image.rotation);
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let rows = [
        image.to_rgb[0],
        image.to_rgb[1],
        image.to_rgb[2],
        [origin[0], origin[1], across[0], across[1]],
        [
            down[0],
            down[1],
            flag(image.layout == ChromaLayout::Interleaved),
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
        let image = YuvImage {
            luma_size: (4, 2),
            layout: ChromaLayout::Interleaved,
            planes: Box::new(vec![0u8; 12]),
            to_rgb: [
                [1.0, 2.0, 3.0, 4.0],
                [5.0, 6.0, 7.0, 8.0],
                [9.0, 10.0, 11.0, 12.0],
            ],
            rotation: 90,
        };
        let bytes = uniform_bytes(&image, true);
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
        let planar = YuvImage {
            layout: ChromaLayout::Planar,
            rotation: 0,
            ..image
        };
        let bytes = uniform_bytes(&planar, false);
        assert_eq!(bytes[72..], [0u8; 8], "both flags off");
    }
}
