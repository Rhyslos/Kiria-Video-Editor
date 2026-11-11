//! A video picture drawn by the GPU straight from the decoder's YUV planes.
//!
//! The page hands over the planes inside a [`VideoPicture`]; the viewer
//! draws it with a paint callback, and the shader converts colour, scales
//! and turns the picture while drawing. The planes come one of two ways:
//!
//! - in memory, as a [`YuvImage`]: just before the window is drawn, the
//!   callback writes them into textures (the only work on the interface
//!   thread: one copy);
//! - as textures already, as [`PlaneTextures`]: a picture decoded on the
//!   window's own graphics device. Nothing is written; the textures are
//!   drawn from as they are.
//!
//! [`install`] sets this up once per window. Without it (the window isn't
//! drawn with wgpu) pages keep showing RGBA textures.

use crate::yuv_renderer::YuvRenderer;
use egui_wgpu::wgpu;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How the two colour channels follow the luma plane in [`YuvImage::planes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaLayout {
    /// One plane, U and V side by side in each pair of bytes.
    Interleaved,
    /// All of U, then all of V.
    Planar,
}

/// A picture as 8-bit YUV planes with colour at half size both ways, and
/// what the shader needs to show it.
pub struct YuvImage {
    /// The luma plane's size. Colour planes are half of it, rounded up.
    pub luma_size: (u32, u32),
    pub layout: ChromaLayout,
    /// Luma rows, then the colour samples, with no padding anywhere.
    pub planes: Box<dyn AsRef<[u8]> + Send + Sync>,
    /// Red, green and blue as weights of (Y, U, V, 1), samples in 0–1.
    pub to_rgb: [[f32; 4]; 3],
    /// The clockwise turn (0, 90, 180 or 270) that shows it upright.
    pub rotation: u16,
}

impl std::fmt::Debug for YuvImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.luma_size;
        write!(f, "YuvImage({w}×{h} {:?}, turned {})", self.layout, self.rotation)
    }
}

impl YuvImage {
    /// The size of each colour plane (in U and V pairs when interleaved).
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.luma_size.0.div_ceil(2), self.luma_size.1.div_ceil(2))
    }

    /// How many bytes `planes` must hold.
    pub fn bytes_needed(&self) -> usize {
        let (cw, ch) = self.chroma_size();
        self.luma_size.0 as usize * self.luma_size.1 as usize + 2 * cw as usize * ch as usize
    }

    pub fn bytes(&self) -> &[u8] {
        (*self.planes).as_ref()
    }

    /// The size on screen, after turning upright.
    pub fn upright_size(&self) -> (u32, u32) {
        let (w, h) = self.luma_size;
        if self.rotation % 180 == 90 { (h, w) } else { (w, h) }
    }
}

/// A picture whose planes are textures on the window's device already:
/// luma with one byte a pixel, and U and V together in a texture of half
/// the size (rounded up) with two bytes a sample.
#[derive(Debug, Clone)]
pub struct PlaneTextures {
    pub luma: wgpu::Texture,
    pub chroma: wgpu::Texture,
    /// Red, green and blue as weights of (Y, U, V, 1), samples in 0–1.
    pub to_rgb: [[f32; 4]; 3],
    /// The clockwise turn (0, 90, 180 or 270) that shows it upright.
    pub rotation: u16,
}

impl PlaneTextures {
    /// The size on screen, after turning upright.
    pub fn upright_size(&self) -> (u32, u32) {
        let (w, h) = (self.luma.width(), self.luma.height());
        if self.rotation % 180 == 90 { (h, w) } else { (w, h) }
    }
}

/// A picture on its way to the renderer.
#[derive(Debug)]
enum Fresh {
    Memory(YuvImage),
    Textures(PlaneTextures),
}

/// Where the corners of the rectangle on screen fall in the stored picture,
/// for a picture shown turned clockwise by `rotation`: the texture position
/// of the rectangle's top left corner, then the step to its top right, then
/// the step to its bottom left. Positions run 0 to 1 across and down.
pub fn texture_axes(rotation: u16) -> [[f32; 2]; 3] {
    match rotation % 360 {
        90 => [[0.0, 1.0], [0.0, -1.0], [1.0, 0.0]],
        180 => [[1.0, 1.0], [-1.0, 0.0], [0.0, -1.0]],
        270 => [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]],
        _ => [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
    }
}

/// The renderer kept with the window, and which picture its textures hold.
struct Shown {
    renderer: YuvRenderer,
    picture: Option<u64>,
}

/// Sets up drawing video pictures in a window. Returns the largest plane
/// side the graphics device takes, or `None` when the window isn't drawn
/// with wgpu: then pages show RGBA textures instead.
pub fn install(render_state: Option<&egui_wgpu::RenderState>) -> Option<u32> {
    let state = render_state?;
    let renderer = YuvRenderer::new(&state.device, state.target_format);
    state.renderer.write().callback_resources.insert(Shown {
        renderer,
        picture: None,
    });
    Some(state.device.limits().max_texture_dimension_2d)
}

/// One picture for the viewer. Make a new one for every picture decoded;
/// its planes are uploaded the first time it is drawn.
pub struct VideoPicture {
    id: u64,
    size: (u32, u32),
    /// The planes, until the renderer has them.
    fresh: Arc<Mutex<Option<Fresh>>>,
}

impl std::fmt::Debug for VideoPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VideoPicture({}×{})", self.size.0, self.size.1)
    }
}

impl VideoPicture {
    /// A picture from planes in memory.
    pub fn new(image: YuvImage) -> VideoPicture {
        VideoPicture::from_fresh(image.upright_size(), Fresh::Memory(image))
    }

    /// A picture whose planes are textures already.
    pub fn from_textures(planes: PlaneTextures) -> VideoPicture {
        VideoPicture::from_fresh(planes.upright_size(), Fresh::Textures(planes))
    }

    fn from_fresh(size: (u32, u32), fresh: Fresh) -> VideoPicture {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        VideoPicture {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            size,
            fresh: Arc::new(Mutex::new(Some(fresh))),
        }
    }

    /// The size on screen, upright.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The shape that draws this picture filling `rect`.
    pub(crate) fn shape(&self, rect: egui::Rect) -> egui::Shape {
        let draw = Draw {
            id: self.id,
            fresh: Arc::clone(&self.fresh),
        };
        egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(rect, draw))
    }
}

/// The paint callback: uploads the picture if the GPU doesn't have it yet,
/// then draws it.
struct Draw {
    id: u64,
    fresh: Arc<Mutex<Option<Fresh>>>,
}

impl egui_wgpu::CallbackTrait for Draw {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let fresh = self.fresh.lock().ok().and_then(|mut fresh| fresh.take());
        if let (Some(fresh), Some(shown)) = (fresh, resources.get_mut::<Shown>()) {
            let started = std::time::Instant::now();
            let (taken, bytes) = match &fresh {
                Fresh::Memory(image) => (shown.renderer.upload(device, queue, image), image.bytes().len()),
                Fresh::Textures(planes) => (shown.renderer.show(device, queue, planes), 0),
            };
            shown.picture = taken.then_some(self.id);
            tracing::debug!(
                micros = started.elapsed().as_micros() as u64,
                bytes,
                shown = taken,
                "video planes handed to the renderer"
            );
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        // Only this picture: never another one's planes in this one's place.
        if let Some(shown) = resources.get::<Shown>()
            && shown.picture == Some(self.id)
        {
            shown.renderer.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a point of the rectangle on screen (0–1 across and down) reads
    /// the stored picture.
    fn stored(rotation: u16, across: f32, down: f32) -> [f32; 2] {
        let [origin, right, below] = texture_axes(rotation);
        [
            origin[0] + across * right[0] + down * below[0],
            origin[1] + across * right[1] + down * below[1],
        ]
    }

    #[test]
    fn turning_reads_the_stored_picture_from_the_right_corner() {
        // Upright: the same corners.
        assert_eq!(stored(0, 0.0, 0.0), [0.0, 0.0]);
        assert_eq!(stored(0, 1.0, 1.0), [1.0, 1.0]);
        // A clockwise quarter turn puts the stored left edge on top: the
        // screen's top left shows the stored bottom left, its top right the
        // stored top left.
        assert_eq!(stored(90, 0.0, 0.0), [0.0, 1.0]);
        assert_eq!(stored(90, 1.0, 0.0), [0.0, 0.0]);
        assert_eq!(stored(90, 1.0, 1.0), [1.0, 0.0]);
        // Half a turn swaps opposite corners.
        assert_eq!(stored(180, 0.0, 0.0), [1.0, 1.0]);
        assert_eq!(stored(180, 1.0, 0.0), [0.0, 1.0]);
        // A counter-clockwise quarter turn puts the stored right edge on top.
        assert_eq!(stored(270, 0.0, 0.0), [1.0, 0.0]);
        assert_eq!(stored(270, 1.0, 0.0), [1.0, 1.0]);
        assert_eq!(stored(270, 0.0, 1.0), [0.0, 0.0]);
        // Every turn shows the whole picture: the centre stays the centre.
        for turn in [0, 90, 180, 270, 360] {
            assert_eq!(stored(turn, 0.5, 0.5), [0.5, 0.5]);
        }
    }

    #[test]
    fn plane_sizes_round_up_and_turns_swap_the_shape() {
        let image = |luma_size, rotation| YuvImage {
            luma_size,
            layout: ChromaLayout::Planar,
            planes: Box::new(Vec::new()),
            to_rgb: [[0.0; 4]; 3],
            rotation,
        };
        let odd = image((5, 3), 0);
        assert_eq!(odd.chroma_size(), (3, 2));
        assert_eq!(odd.bytes_needed(), 15 + 2 * 6);
        assert_eq!(image((1920, 1080), 0).bytes_needed(), 1920 * 1080 * 3 / 2);
        assert_eq!(image((320, 180), 270).upright_size(), (180, 320));
        assert_eq!(VideoPicture::new(image((320, 180), 90)).size(), (180, 320));
        assert_eq!(VideoPicture::new(image((320, 180), 180)).size(), (320, 180));
    }
}
