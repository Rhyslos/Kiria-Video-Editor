//! Puts the decoded pictures of one moment together into the timeline's
//! picture, on the device the window is drawn with (`render::Compositor`).
//!
//! The preview worker does this, so the interface is handed one finished
//! frame to show and never does more than draw a rectangle (the choice and
//! its measurements: docs/research/rendering.md).

use crate::frame::{Adjust, FrameDescription, Layer, Step};
use media::{Decoded, GpuFrame, Layout};
use render::{Area, Background, Colour, Compositor, Drawn, Frame, Part, Pixels, Working};
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

/// Why a frame was not put together.
#[derive(Debug)]
pub(crate) enum NotComposed {
    /// The renderer could not take a picture decoded on the window's device.
    Refused,
    /// The graphics library refused the work (a size over the card's limit,
    /// the card out of memory): told for this frame; the composer starts
    /// afresh with the next.
    Graphics(diag::AppError),
}

/// Frames kept for drawing into again once nobody shows them any more.
const FRAMES_KEPT: usize = 4;

/// Frames in a row that need fewer upload slots than are kept before the
/// slots past what they need are let go (about a second of playback).
const UPLOADS_IDLE_FOR: u32 = 30;

/// How long the small copy for the scopes may take to come back.
const SAMPLE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// A graphics error as an explained error for one frame.
fn graphics_error(what: &str, error: &wgpu::Error) -> diag::AppError {
    diag::AppError::new(what)
        .why(format!(
            "The graphics library refused the work: {}.",
            render::caught::describe(error)
        ))
        .fix("Try again. If it keeps happening, lower the timeline's picture size or update the graphics driver.")
}

pub(crate) struct Composer {
    effects: render::Effects,
    composing: Composing,
    compositor: Compositor,
    /// Frames handed out before. One nobody else holds is drawn into again.
    frames: Vec<Arc<Frame>>,
    /// By layer: the textures planes in memory were last written into.
    uploads: Vec<Option<Upload>>,
    /// Frames in a row that needed fewer upload slots than `uploads` has.
    uploads_idle: u32,
    /// The small copy for the scopes and the buffer it is read back through,
    /// kept while the size stays the same.
    scope_copy: Option<ScopeCopy>,
    /// Pictures of nested timelines, and of what is below adjustment
    /// layers, from the last frame: drawn into again for the next.
    spare: Vec<Frame>,
}

impl Composer {
    /// Makes the pipelines: some tens of milliseconds, once, on this thread.
    pub fn new(composing: Composing) -> Composer {
        Composer {
            compositor: Compositor::new(&composing.device, Working::Linear),
            effects: render::Effects::new(&composing.device, Working::Linear),
            composing,
            frames: Vec::new(),
            uploads: Vec::new(),
            uploads_idle: 0,
            scope_copy: None,
            spare: Vec::new(),
        }
    }

    /// Lets go of everything kept between frames, after a graphics error
    /// that may have left some of it broken: the next frame makes it afresh.
    fn start_afresh(&mut self) {
        self.frames.clear();
        self.uploads.clear();
        self.uploads_idle = 0;
        self.scope_copy = None;
        self.spare.clear();
    }

    /// Keeps upload slots for the layers of a frame of `layers` layers: the
    /// slots past those are let go once frames have needed fewer for a while
    /// (each holds textures the size of a decoded picture).
    fn keep_uploads_for(&mut self, layers: usize) {
        let needed = layers * 2;
        if self.uploads.len() > needed {
            self.uploads_idle += 1;
            if self.uploads_idle >= UPLOADS_IDLE_FOR {
                self.uploads.truncate(needed);
                self.uploads_idle = 0;
            }
        } else {
            self.uploads_idle = 0;
        }
        self.uploads.resize_with(self.uploads.len().max(needed), || None);
    }

    /// Draws the picture `description` describes into a frame of its size,
    /// with `pictures`: for some of its layers (by number) the decoded
    /// picture and, with frame blending, the next recorded frame too. A
    /// layer without its picture is left out; nothing at all is a black frame.
    ///
    /// A nested timeline is put together first into a picture of its own
    /// (see-through where nothing is) and drawn as one layer; an adjustment
    /// layer has what is below it put together first and drawn again,
    /// coloured, over itself (see `Drawing`).
    ///
    /// A graphics error on the way is returned for this frame
    /// (`NotComposed::Graphics`) rather than ending the thread.
    pub fn compose(
        &mut self,
        description: &FrameDescription,
        pictures: Vec<(usize, Decoded, Option<Decoded>)>,
    ) -> Result<Composed, NotComposed> {
        let device = self.composing.device.clone();
        match render::caught(&device, || self.compose_checked(description, pictures)) {
            Ok(Ok(composed)) => Ok(composed),
            Ok(Err(Refused)) => Err(NotComposed::Refused),
            Err(error) => {
                tracing::warn!("a frame could not be put together: {error}");
                self.start_afresh();
                Err(NotComposed::Graphics(graphics_error(
                    "A frame of the timeline could not be put together on the graphics card.",
                    &error,
                )))
            }
        }
    }

    /// `compose`, with graphics errors left to the caller to catch.
    fn compose_checked(
        &mut self,
        description: &FrameDescription,
        pictures: Vec<(usize, Decoded, Option<Decoded>)>,
    ) -> Result<Composed, Refused> {
        let (device, queue) = (&self.composing.device.clone(), &self.composing.queue.clone());
        // Every layer's pixels as textures on the device: two slots a
        // layer, the second for the frame blended in.
        let mut on_device = false;
        let mut placed: Vec<Option<Placed>> = Vec::new();
        placed.resize_with(description.layers.len(), || None);
        self.keep_uploads_for(description.layers.len());
        for (index, decoded, next) in pictures {
            if index >= placed.len() {
                continue;
            }
            let (views, to_rgb, rotation) = self.textures(decoded, index * 2, &mut on_device)?;
            // The next frame counts only when it is stored as this one is.
            let next = match next {
                Some(next) => Some(self.textures(next, index * 2 + 1, &mut on_device)?),
                None => None,
            }
            .filter(|(_, next_rgb, next_rotation)| *next_rgb == to_rgb && *next_rotation == rotation)
            .map(|(views, ..)| views);
            placed[index] = Some(Placed {
                views,
                next,
                to_rgb,
                rotation,
            });
        }

        let mut frame = self.frame_to_draw_into(description.size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("the timeline's picture"),
        });
        let mut drawing = Drawing {
            compositor: &self.compositor,
            effects: &self.effects,
            device,
            queue,
            layers: &description.layers,
            placed: &placed,
            spare: std::mem::take(&mut self.spare),
            used: Vec::new(),
        };
        match Arc::get_mut(&mut frame) {
            Some(target) => drawing.steps(&mut encoder, &description.steps, target, Background::Black),
            // Not reached: the frame was just found unshared or made new.
            None => {
                let mut fresh = self.compositor.frame(device, description.size);
                drawing.steps(&mut encoder, &description.steps, &mut fresh, Background::Black);
                frame = Arc::new(fresh);
            }
        }
        // The pictures put together on the way are kept for the next frame.
        let used = std::mem::take(&mut drawing.used);
        queue.submit([encoder.finish()]);
        self.spare = used;
        self.frames.push(frame.clone());
        Ok(Composed { frame, on_device })
    }

    /// `composed` drawn at about `width` pixels across and read back as
    /// display values, for the scopes. Waits for the graphics card (a few
    /// seconds at most, without keeping the device from the window): on the
    /// preview's worker only. `None` when it couldn't be read; a graphics
    /// error is logged and the next sample starts afresh.
    pub fn sample(&mut self, composed: &Composed, width: u32) -> Option<crate::scope::Sampled> {
        let device = self.composing.device.clone();
        match render::caught(&device, || self.sample_checked(composed, width)) {
            Ok(sampled) => sampled,
            Err(error) => {
                tracing::warn!("the picture for the scopes could not be read back: {error}");
                self.scope_copy = None;
                None
            }
        }
    }

    /// `sample`, with graphics errors left to the caller to catch.
    fn sample_checked(&mut self, composed: &Composed, width: u32) -> Option<crate::scope::Sampled> {
        let (device, queue) = (&self.composing.device, &self.composing.queue);
        let (fw, fh) = composed.size();
        let w = width.min(fw).max(1);
        let h = ((u64::from(fh) * u64::from(w)) / u64::from(fw.max(1))).max(1) as u32;
        // The copy and its buffer from last time when the size is the same.
        let mut copy = match self.scope_copy.take() {
            Some(kept) if kept.small.size() == (w, h) => kept,
            _ => ScopeCopy::new(&self.compositor, device, (w, h)),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("a small copy for the scopes"),
        });
        let whole = Area::fitted((w, h), (w, h));
        let drawn = [made(&composed.frame, whole, Part::WHOLE, 1.0, Colour::NEUTRAL)];
        self.compositor
            .draw(device, queue, &mut encoder, &mut copy.small, &drawn);
        let (linear, bytes, row) = (copy.linear(), copy.bytes(), copy.row());
        encoder.copy_texture_to_buffer(
            copy.small.texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &copy.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        // On an error the buffer is let go of by `read_back` and can be used again.
        let read = render::read_back(device, &[&copy.buffer], SAMPLE_PATIENCE);
        let sampled = match read {
            Ok(()) => {
                let sampled = copy.buffer.get_mapped_range(..).ok().map(|mapped| {
                    let mut rgb = Vec::with_capacity((w * h) as usize);
                    for line in mapped.chunks(row as usize) {
                        for p in line[..(w * bytes) as usize].chunks(bytes as usize) {
                            rgb.push(match linear {
                                true => {
                                    let channel =
                                        |k: usize| half(u16::from_le_bytes([p[2 * k], p[2 * k + 1]]));
                                    let alpha = channel(3).max(1e-5);
                                    [0, 1, 2].map(|k| (channel(k) / alpha).clamp(0.0, 1.0).powf(1.0 / 2.4))
                                }
                                false => [0, 1, 2].map(|k| f32::from(p[k]) / 255.0),
                            });
                        }
                    }
                    crate::scope::Sampled {
                        width: w as usize,
                        height: h as usize,
                        rgb,
                    }
                });
                copy.buffer.unmap();
                sampled
            }
            Err(error) => {
                tracing::debug!("the picture for the scopes did not come back: {error:?}");
                None
            }
        };
        self.scope_copy = Some(copy);
        sampled
    }

    /// One decoded picture as textures on the device, written into upload
    /// slot `slot` when it is in memory: the textures, the rows that turn
    /// its samples into RGB, and its turn.
    fn textures(
        &mut self,
        decoded: Decoded,
        slot: usize,
        on_device: &mut bool,
    ) -> Result<(Views, [[f32; 4]; 3], u16), Refused> {
        let (device, queue) = (&self.composing.device, &self.composing.queue);
        Ok(match decoded {
            Decoded::Gpu(frame) => {
                let wrap = self.composing.wrap.as_ref();
                let (luma, chroma) = wrap.and_then(|wrap| wrap(&frame)).ok_or(Refused)?;
                *on_device = true;
                let view =
                    |texture: &wgpu::Texture| texture.create_view(&wgpu::TextureViewDescriptor::default());
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
                let upload = Upload::of(&mut self.uploads[slot], device, shape);
                upload.write(queue, frame.into_planes().as_ref());
                (upload.views(), to_rgb, rotation)
            }
            Decoded::Rgba(frame) => {
                let upload = Upload::of(
                    &mut self.uploads[slot],
                    device,
                    Shape::Rgba(frame.width, frame.height),
                );
                upload.write(queue, &frame.rgba);
                // Converted and turned upright already.
                (upload.views(), [[0.0; 4]; 3], 0)
            }
        })
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

/// The small copy of a frame for the scopes, and the buffer it is read
/// back through.
struct ScopeCopy {
    small: Frame,
    buffer: wgpu::Buffer,
}

impl ScopeCopy {
    fn new(compositor: &Compositor, device: &wgpu::Device, size: (u32, u32)) -> ScopeCopy {
        let small = compositor.frame(device, size);
        let row = row_bytes(size.0, small.working() == Working::Linear);
        ScopeCopy {
            small,
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scope read back"),
                size: u64::from(row) * u64::from(size.1),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        }
    }

    fn linear(&self) -> bool {
        self.small.working() == Working::Linear
    }

    /// Bytes a pixel takes: 16-bit floats in linear light, else 8 bits.
    fn bytes(&self) -> u32 {
        pixel_bytes(self.linear())
    }

    fn row(&self) -> u32 {
        row_bytes(self.small.size().0, self.linear())
    }
}

fn pixel_bytes(linear: bool) -> u32 {
    if linear { 8 } else { 4 }
}

/// Bytes a row of `width` pixels takes in a read-back buffer (rows start at
/// multiples of 256).
fn row_bytes(width: u32, linear: bool) -> u32 {
    (width * pixel_bytes(linear)).div_ceil(256) * 256
}

/// One layer's decoded picture on the device, ready to draw.
struct Placed {
    views: Views,
    /// The next recorded frame, for frame blending.
    next: Option<Views>,
    to_rgb: [[f32; 4]; 3],
    rotation: u16,
}

/// Putting one frame together: the steps of the timeline, and of every
/// timeline nested in it, drawn into frames in one command encoder.
///
/// Why an adjustment layer is drawn this way: its colour must apply to the
/// picture below it as a whole, in linear light, exactly as a clip's own
/// quick colour does. Colouring each layer below on its own would be wrong
/// wherever layers are see-through or overlap (the colour would be applied
/// to each before they are mixed, and twice where one shows through
/// another), and could not reach the inside of a nested timeline drawn as
/// one picture. So what is below it is put together into a picture of its
/// own, which is drawn as it is and then again, coloured, at the
/// adjustment layer's opacity: where it is fully opaque the result is the
/// picture below, coloured; at half, halfway between.
struct Drawing<'a> {
    compositor: &'a Compositor,
    effects: &'a render::Effects,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    layers: &'a [Layer],
    placed: &'a [Option<Placed>],
    /// Frames from the last time, to draw into again.
    spare: Vec<Frame>,
    /// Frames drawn into this time: each only once, as its numbers are
    /// written when the encoder is submitted.
    used: Vec<Frame>,
}

impl Drawing<'_> {
    /// A frame of `size` to draw into, kept from last time or new.
    fn take(&mut self, size: (u32, u32)) -> Frame {
        match self.spare.iter().position(|frame| frame.size() == size) {
            Some(found) => self.spare.swap_remove(found),
            None => self.compositor.frame(self.device, size),
        }
    }

    /// Draws `steps` into `target` over `background`.
    fn steps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        steps: &[Step],
        target: &mut Frame,
        background: Background,
    ) {
        let (layers, placed) = (self.layers, self.placed);
        // Cut at each adjustment layer: what comes before it is put
        // together first, then drawn again coloured under what follows.
        let mut below: Option<(Frame, Adjust)> = None;
        let mut rest = steps;
        loop {
            let cut = rest.iter().position(|step| matches!(step, Step::Adjust(_)));
            let (stretch, adjust) = match cut {
                Some(at) => (&rest[..at], rest.get(at)),
                None => (rest, None),
            };
            // Nested timelines in this stretch, each into a picture of its own.
            let mut nested = Vec::new();
            for step in stretch {
                if let Step::Timeline(group) = step {
                    let mut picture = self.take(group.size);
                    self.steps(encoder, &group.steps, &mut picture, Background::Clear);
                    nested.push(self.with_effects(encoder, picture, &group.effects));
                }
            }
            // Pictures with effects, each drawn upright on its own first.
            let mut changed = Vec::new();
            for step in stretch {
                if let Step::Picture(index) = step
                    && let (Some(layer), Some(Some(placed))) = (layers.get(*index), placed.get(*index))
                    && !layer.effects.is_empty()
                {
                    changed.push(self.picture_with_effects(encoder, layer, placed));
                }
            }
            let mut drawn = Vec::with_capacity(stretch.len() + 2);
            if let Some((picture, adjust)) = &below {
                let whole = Area::fitted(target.size(), target.size());
                drawn.push(made(picture, whole, Part::WHOLE, 1.0, Colour::NEUTRAL));
                drawn.push(made(picture, whole, Part::WHOLE, adjust.opacity, adjust.colour));
            }
            let mut pictures = nested.iter();
            let mut with_effects = changed.iter();
            for step in stretch {
                match step {
                    Step::Picture(index) => {
                        if let (Some(layer), Some(Some(placed))) = (layers.get(*index), placed.get(*index)) {
                            if layer.effects.is_empty() {
                                drawn.push(placed.drawn(layer));
                            } else if let Some(applied) = with_effects.next() {
                                let whole = Part::WHOLE;
                                let (x, y) = applied.grown;
                                let area = layer.area.grown(x, y);
                                let placed =
                                    made(&applied.picture, area, whole, layer.opacity, Colour::NEUTRAL);
                                drawn.push(Drawn {
                                    mode: layer.mode,
                                    ..placed
                                });
                            }
                        }
                    }
                    Step::Timeline(group) => {
                        if let Some(applied) = pictures.next() {
                            let (area, part) = grown_part(group.area, group.part, applied.grown);
                            let placed = made(&applied.picture, area, part, group.opacity, group.colour);
                            drawn.push(Drawn {
                                mode: group.mode,
                                ..placed
                            });
                        }
                    }
                    Step::Adjust(_) => {}
                }
            }
            let next = match adjust {
                Some(Step::Adjust(adjust)) => {
                    let mut picture = self.take(target.size());
                    self.draw(encoder, &mut picture, &drawn, background);
                    Some((picture, *adjust))
                }
                _ => {
                    self.draw(encoder, target, &drawn, background);
                    None
                }
            };
            drop(drawn);
            self.used
                .extend(nested.into_iter().map(|applied| applied.picture));
            self.used
                .extend(changed.into_iter().map(|applied| applied.picture));
            self.used.extend(below.take().map(|(picture, _)| picture));
            match (next, cut) {
                (Some(next), Some(at)) => {
                    below = Some(next);
                    rest = &rest[at + 1..];
                }
                _ => return,
            }
        }
    }

    /// `layer`'s picture drawn upright, coloured, at the size it takes in
    /// the frame, with its effects applied: ready to be placed.
    fn picture_with_effects(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        layer: &Layer,
        placed: &Placed,
    ) -> render::Applied {
        let (w, h) = layer.area.size();
        let size = (
            (w.round() as u32).clamp(1, LARGEST_EFFECT_PICTURE),
            (h.round() as u32).clamp(1, LARGEST_EFFECT_PICTURE),
        );
        let mut picture = self.take(size);
        let upright = Layer {
            area: Area::fitted(size, size),
            opacity: 1.0,
            mode: render::Mode::Normal,
            ..layer.clone()
        };
        self.draw(
            encoder,
            &mut picture,
            &[placed.drawn(&upright)],
            Background::Clear,
        );
        self.with_effects(encoder, picture, &layer.effects)
    }

    /// `picture` with `effects` applied; the pictures the passes wrote on
    /// the way go to `used`, to be drawn into again next time.
    fn with_effects(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        picture: Frame,
        effects: &[render::Effect],
    ) -> render::Applied {
        if effects.is_empty() {
            return render::Applied {
                picture,
                grown: (0.0, 0.0),
            };
        }
        let (compositor, device) = (self.compositor, self.device);
        let spare = &mut self.spare;
        let mut take = |size: (u32, u32)| match spare.iter().position(|frame| frame.size() == size) {
            Some(found) => spare.swap_remove(found),
            None => compositor.frame(device, size),
        };
        let pictures = render::Pictures {
            take: &mut take,
            done: &mut self.used,
        };
        self.effects
            .apply(self.device, self.queue, encoder, picture, effects, pictures)
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &mut Frame,
        drawn: &[Drawn<'_>],
        background: Background,
    ) {
        self.compositor
            .draw_over(self.device, self.queue, encoder, frame, drawn, background);
    }
}

/// The largest side a picture with effects is drawn at before it is placed:
/// a clip zoomed in far is changed at this size and enlarged after.
const LARGEST_EFFECT_PICTURE: u32 = 8192;

/// Where a nested timeline's picture goes once its effects made room around
/// it (`grown`, shares of its width and height on each side): shown whole,
/// the room too, when it was shown whole; else the same part of it as
/// before, in the same place, and what is outside that part stays cut off.
fn grown_part(area: Area, part: Part, grown: (f32, f32)) -> (Area, Part) {
    let (x, y) = grown;
    if (x, y) == (0.0, 0.0) {
        return (area, part);
    }
    if part == Part::WHOLE {
        return (area.grown(x, y), Part::WHOLE);
    }
    let inside = Part {
        left: (part.left + x) / (1.0 + 2.0 * x),
        top: (part.top + y) / (1.0 + 2.0 * y),
        width: part.width / (1.0 + 2.0 * x),
        height: part.height / (1.0 + 2.0 * y),
    };
    (area, inside)
}

/// A picture the compositor made, drawn again where `area` says.
fn made<'a>(picture: &'a Frame, area: Area, part: Part, opacity: f32, colour: Colour) -> Drawn<'a> {
    Drawn {
        pixels: Pixels::Made(picture.view()),
        to_rgb: [[0.0; 4]; 3],
        rotation: 0,
        area,
        part,
        opacity,
        colour,
        blend: None,
        mode: render::Mode::Normal,
    }
}

impl Placed {
    fn drawn<'a>(&'a self, layer: &Layer) -> Drawn<'a> {
        Drawn {
            pixels: self.views.pixels(),
            to_rgb: self.to_rgb,
            rotation: self.rotation,
            area: layer.area,
            part: layer.part,
            opacity: layer.opacity,
            colour: layer.colour,
            blend: self
                .next
                .as_ref()
                .zip(layer.blend)
                .map(|(next, blend)| (next.pixels(), blend.weight)),
            mode: layer.mode,
        }
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

#[cfg(test)]
mod tests;

/// A 16-bit float as an f32.
fn half(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff) / 1024.0;
    match exponent {
        0 => sign * fraction * 2f32.powi(-14),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + fraction) * 2f32.powi(exponent - 15),
    }
}
