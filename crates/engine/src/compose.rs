//! Puts the decoded pictures of one moment together into the timeline's
//! picture, on the device the window is drawn with (`render::Compositor`).
//!
//! The preview worker does this, so the interface is handed one finished
//! frame to show and never does more than draw a rectangle (the choice and
//! its measurements: docs/research/rendering.md).

use crate::frame::Layer;
use media::{Decoded, GpuFrame, Layout};
use render::{Compositor, Drawn, Frame, Pixels, Working};
use std::sync::Arc;

/// Turns a picture decoded on the window's device into the two textures it
/// is drawn from (brightness, and colour at half size). Nothing when the
/// renderer can't take the picture's images.
pub type Wrap = dyn Fn(&GpuFrame) -> Option<(wgpu::Texture, wgpu::Texture)> + Send + Sync;

/// What the worker composes with: the device and queue the window is drawn with.
#[derive(Clone)]
pub struct Composing {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// For pictures decoded on that device; `None` when none are.
    pub wrap: Option<Arc<Wrap>>,
}

impl std::fmt::Debug for Composing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Composing(pictures decoded on the device: {})",
            self.wrap.is_some()
        )
    }
}

/// A finished picture of the timeline, on the graphics card.
#[derive(Debug, Clone)]
pub struct Composed {
    frame: Arc<Frame>,
    /// Some of it was decoded on the window's device and drawn from there.
    pub on_device: bool,
}

impl Composed {
    pub fn view(&self) -> &wgpu::TextureView {
        self.frame.view()
    }

    pub fn texture(&self) -> &wgpu::Texture {
        self.frame.texture()
    }

    pub fn size(&self) -> (u32, u32) {
        self.frame.size()
    }

    /// What the frame was made in, for whoever shows it.
    pub fn working(&self) -> Working {
        self.frame.working()
    }
}

/// The renderer could not take a picture decoded on the window's device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refused;

/// Frames kept for drawing into again once nobody shows them any more.
const FRAMES_KEPT: usize = 4;

pub(crate) struct Composer {
    composing: Composing,
    compositor: Compositor,
    /// Frames handed out before. One nobody else holds is drawn into again.
    frames: Vec<Arc<Frame>>,
    /// By layer: the textures planes in memory were last written into.
    uploads: Vec<Option<Upload>>,
}

impl Composer {
    /// Makes the pipelines: some tens of milliseconds, once, on this thread.
    pub fn new(composing: Composing) -> Composer {
        Composer {
            compositor: Compositor::new(&composing.device, Working::Linear),
            composing,
            frames: Vec::new(),
            uploads: Vec::new(),
        }
    }

    /// Draws `layers` (bottom first, each with its decoded picture) into a
    /// frame of `size` pixels. An empty list is a black frame.
    pub fn compose(&mut self, size: (u32, u32), layers: Vec<(Layer, Decoded)>) -> Result<Composed, Refused> {
        let (device, queue) = (&self.composing.device.clone(), &self.composing.queue.clone());
        let wrap = self.composing.wrap.clone();
        // Every layer's pixels as textures on the device.
        let mut on_device = false;
        let mut placed = Vec::with_capacity(layers.len());
        self.uploads
            .resize_with(self.uploads.len().max(layers.len()), || None);
        for (index, (layer, decoded)) in layers.into_iter().enumerate() {
            let (views, to_rgb, rotation) = match decoded {
                Decoded::Gpu(frame) => {
                    let (luma, chroma) = wrap.as_ref().and_then(|wrap| wrap(&frame)).ok_or(Refused)?;
                    on_device = true;
                    let view = |texture: &wgpu::Texture| {
                        texture.create_view(&wgpu::TextureViewDescriptor::default())
                    };
                    (
                        Views::Two(view(&luma), view(&chroma)),
                        frame.colour.rgb_rows(),
                        frame.rotation,
                    )
                }
                Decoded::Yuv(frame) => {
                    let shape = Shape::Planes {
                        luma: (frame.width, frame.height),
                        layout: frame.layout,
                    };
                    let (to_rgb, rotation) = (frame.colour.rgb_rows(), frame.rotation);
                    let upload = Upload::of(&mut self.uploads[index], device, shape);
                    upload.write(queue, frame.into_planes().as_ref());
                    (upload.views(), to_rgb, rotation)
                }
                Decoded::Rgba(frame) => {
                    let upload = Upload::of(
                        &mut self.uploads[index],
                        device,
                        Shape::Rgba(frame.width, frame.height),
                    );
                    upload.write(queue, &frame.rgba);
                    // Converted and turned upright already.
                    (upload.views(), [[0.0; 4]; 3], 0)
                }
            };
            placed.push((layer, views, to_rgb, rotation));
        }
        let drawn: Vec<Drawn<'_>> = placed
            .iter()
            .map(|(layer, views, to_rgb, rotation)| Drawn {
                pixels: views.pixels(),
                to_rgb: *to_rgb,
                rotation: *rotation,
                area: layer.area,
                part: layer.part,
                opacity: layer.opacity,
            })
            .collect();

        let mut frame = self.frame_to_draw_into(size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("the timeline's picture"),
        });
        match Arc::get_mut(&mut frame) {
            Some(target) => self.compositor.draw(device, queue, &mut encoder, target, &drawn),
            // Not reached: the frame was just found unshared or made new.
            None => {
                let mut fresh = self.compositor.frame(device, size);
                self.compositor
                    .draw(device, queue, &mut encoder, &mut fresh, &drawn);
                frame = Arc::new(fresh);
            }
        }
        queue.submit([encoder.finish()]);
        self.frames.push(frame.clone());
        Ok(Composed { frame, on_device })
    }

    /// A frame of `size` that nobody shows any more, or a new one. Frames of
    /// other sizes that nobody shows are let go.
    fn frame_to_draw_into(&mut self, size: (u32, u32)) -> Arc<Frame> {
        self.frames
            .retain(|frame| frame.size() == size || Arc::strong_count(frame) > 1);
        if let Some(free) = self.frames.iter().position(|frame| Arc::strong_count(frame) == 1) {
            return self.frames.swap_remove(free);
        }
        // All in use (the interface holds on to what it shows): forget the
        // oldest rather than keep ever more.
        if self.frames.len() >= FRAMES_KEPT {
            self.frames.remove(0);
        }
        Arc::new(self.compositor.frame(&self.composing.device, size))
    }
}

/// What planes in memory are shaped like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Brightness of this size and colour at half of it.
    Planes {
        luma: (u32, u32),
        layout: Layout,
    },
    Rgba(u32, u32),
}

/// Textures a layer's pixels are written into, picture after picture.
struct Upload {
    shape: Shape,
    textures: Vec<wgpu::Texture>,
}

impl Upload {
    /// The textures in `slot` if they have this shape, else new ones there.
    fn of<'a>(slot: &'a mut Option<Upload>, device: &wgpu::Device, shape: Shape) -> &'a Upload {
        if slot.as_ref().is_none_or(|upload| upload.shape != shape) {
            *slot = None;
        }
        slot.get_or_insert_with(|| {
            let texture = |(width, height): (u32, u32), format| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("a layer's pixels"),
                    size: wgpu::Extent3d {
                        width: width.max(1),
                        height: height.max(1),
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
            let textures = match shape {
                Shape::Planes { luma, layout } => {
                    let half = media::yuv::chroma_size(luma.0, luma.1);
                    let one = wgpu::TextureFormat::R8Unorm;
                    match layout {
                        Layout::Interleaved => {
                            vec![texture(luma, one), texture(half, wgpu::TextureFormat::Rg8Unorm)]
                        }
                        Layout::Planar => vec![texture(luma, one), texture(half, one), texture(half, one)],
                    }
                }
                Shape::Rgba(width, height) => vec![texture((width, height), wgpu::TextureFormat::Rgba8Unorm)],
            };
            Upload { shape, textures }
        })
    }

    /// Writes one picture's bytes (plane after plane, rows without padding).
    /// Bytes that don't match the shape leave the textures as they were.
    fn write(&self, queue: &wgpu::Queue, bytes: &[u8]) {
        let mut rest = bytes;
        for texture in &self.textures {
            let samples = texture.format().block_copy_size(None).unwrap_or(1);
            let row = texture.width() * samples;
            let length = (row * texture.height()) as usize;
            let Some((plane, after)) = rest.split_at_checked(length) else {
                tracing::warn!("a decoded picture is smaller than its size says: not drawn");
                return;
            };
            rest = after;
            queue.write_texture(
                texture.as_image_copy(),
                plane,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(texture.height()),
                },
                texture.size(),
            );
        }
    }

    fn views(&self) -> Views {
        let view = |index: usize| self.textures[index].create_view(&wgpu::TextureViewDescriptor::default());
        match self.textures.len() {
            1 => Views::One(view(0)),
            2 => Views::Two(view(0), view(1)),
            _ => Views::Three(view(0), view(1), view(2)),
        }
    }
}

/// One layer's textures, held while it is drawn.
enum Views {
    One(wgpu::TextureView),
    Two(wgpu::TextureView, wgpu::TextureView),
    Three(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView),
}

impl Views {
    fn pixels(&self) -> Pixels<'_> {
        match self {
            Views::One(rgba) => Pixels::Rgba(rgba),
            Views::Two(luma, chroma) => Pixels::TwoPlanes { luma, chroma },
            Views::Three(luma, u, v) => Pixels::ThreePlanes { luma, u, v },
        }
    }
}
