//! Reads finished frames back from the graphics card as NV12, the layout
//! video encoders take: brightness at full size, then the two colour
//! samples interleaved at half size (BT.709, limited range).
//!
//! The conversion is drawn on the card (two small passes), so only the
//! twelve bits a pixel the encoder needs cross over to memory.

use crate::compositor::{
    Working, pipeline, sampler_entry, smooth_sampler, texture_binding, texture_entry, uniform_buffer,
    uniform_entry,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// Why a frame could not be read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// The graphics card did not hand the pixels over.
    Refused,
    /// It did not answer in time.
    TimedOut,
}

/// How long one frame may take to come back.
const PATIENCE: Duration = Duration::from_secs(20);

const WAITING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// One plane: where it is drawn and what it is read back through.
#[derive(Debug)]
struct Plane {
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    /// Bytes a row takes in the buffer (rows start at multiples of 256).
    stride: u32,
    /// Bytes of a row that are picture.
    row: u32,
}

#[derive(Debug)]
pub struct Reader {
    size: (u32, u32),
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    luma: Plane,
    chroma: Plane,
}

impl Reader {
    /// For pictures of `size` pixels; odd sides lose their last row or column.
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Reader {
        let size = ((size.0 & !1).max(2), (size.1 & !1).max(2));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("planes"),
            source: wgpu::ShaderSource::Wgsl(include_str!("planes.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planes"),
            entries: &[uniform_entry(16), sampler_entry(), texture_entry(2)],
        });
        let plane =
            |label, fragment, format: wgpu::TextureFormat, (width, height): (u32, u32), samples: u32| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let row = width * samples;
                let stride =
                    row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                Plane {
                    pipeline: pipeline(device, label, &layout, &module, ("whole", fragment), format, None),
                    view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    texture,
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: u64::from(stride) * u64::from(height),
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    stride,
                    row,
                }
            };
        let luma = plane("brightness", "luma", wgpu::TextureFormat::R8Unorm, size, 1);
        let chroma = plane(
            "colour",
            "chroma",
            wgpu::TextureFormat::Rg8Unorm,
            (size.0 / 2, size.1 / 2),
            2,
        );
        Reader {
            size,
            sampler: smooth_sampler(device),
            uniforms: uniform_buffer(device, 16),
            layout,
            luma,
            chroma,
        }
    }

    /// The size of the pictures read: even on both sides.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Bytes in one picture.
    pub fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 3 / 2
    }

    /// Reads `frame` (made in `working`) into `out`: brightness rows, then
    /// colour rows, without padding. Waits until the card has drawn and
    /// handed over, without keeping the device from anyone else meanwhile.
    pub fn read(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &wgpu::TextureView,
        working: Working,
        out: &mut Vec<u8>,
    ) -> Result<(), ReadError> {
        let linear = if working == Working::Linear { 1.0f32 } else { 0.0 };
        let flags: Vec<u8> = [linear, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|n| n.to_ne_bytes())
            .collect();
        queue.write_buffer(&self.uniforms, 0, &flags);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planes"),
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
                texture_binding(2, frame),
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("planes"),
        });
        for plane in [&self.luma, &self.chroma] {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("plane"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &plane.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&plane.pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..4, 0..1);
            }
            encoder.copy_texture_to_buffer(
                plane.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &plane.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(plane.stride),
                        rows_per_image: Some(plane.texture.height()),
                    },
                },
                plane.texture.size(),
            );
        }
        queue.submit([encoder.finish()]);

        // Both planes are asked for, then waited for together.
        let states = [&self.luma, &self.chroma].map(|plane| {
            let state = Arc::new(AtomicU8::new(WAITING));
            let told = state.clone();
            plane.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
                told.store(if result.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
            state
        });
        let give_up = Instant::now() + PATIENCE;
        loop {
            // Looked at again and again rather than waited on: a wait would
            // hold the device while the window wants to draw.
            let _ = device.poll(wgpu::PollType::Poll);
            let now = states.each_ref().map(|state| state.load(Ordering::Acquire));
            if now.contains(&FAILED) {
                self.unmap(&now);
                return Err(ReadError::Refused);
            }
            if now == [READY, READY] {
                break;
            }
            if Instant::now() >= give_up {
                return Err(ReadError::TimedOut);
            }
            std::thread::sleep(Duration::from_micros(150));
        }
        out.clear();
        out.reserve(self.bytes());
        let mut result = Ok(());
        for plane in [&self.luma, &self.chroma] {
            match plane.buffer.get_mapped_range(..) {
                Ok(mapped) => {
                    for row in mapped.chunks(plane.stride as usize) {
                        out.extend_from_slice(&row[..plane.row as usize]);
                    }
                }
                Err(_) => result = Err(ReadError::Refused),
            }
            plane.buffer.unmap();
        }
        result
    }

    /// Lets go of the planes that did come back when another failed.
    fn unmap(&self, states: &[u8; 2]) {
        for (plane, state) in [&self.luma, &self.chroma].into_iter().zip(states) {
            if *state == READY {
                plane.buffer.unmap();
            }
        }
    }
}

/// A black picture of `size` in the same layout: for moments with nothing
/// on the timeline.
pub fn black((width, height): (u32, u32)) -> Vec<u8> {
    let luma = width as usize * height as usize;
    let mut picture = vec![16u8; luma];
    picture.resize(luma * 3 / 2, 128);
    picture
}
