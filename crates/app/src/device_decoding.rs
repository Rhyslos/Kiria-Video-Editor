//! Decoding on the window's graphics device, as the editor wires it up.
//!
//! The window crate opens the device and keeps the guard against a driver
//! that crashes there; the decoder uses the device; the shared viewer draws
//! from the two images a picture ends up in. None of those crates knows the
//! others' types, so this adapts between them: the device becomes the
//! decoder's, the guard becomes the preview worker's watcher, and a decoded
//! picture's images become the viewer's textures.

use eframe::wgpu;
use engine::{Convert, OnDevice};
use graphics::{DecodeGuard, WindowGraphics};
use media::{GpuFrame, WindowDevice};
use std::sync::Arc;
use ui_kit::{PlaneTextures, VideoPicture};

/// The preview worker's reports, passed on to the guard.
struct Watch(Arc<DecodeGuard>);

impl engine::DeviceWatch for Watch {
    fn about_to_decode(&self) {
        self.0.about_to_decode();
    }

    fn answered(&self) {
        self.0.answered();
    }

    fn reported_an_error(&self) {
        self.0.reported_an_error();
    }
}

/// What the viewer needs to show video decoded on the window's device.
#[derive(Debug)]
pub struct WindowDecoding {
    device: Arc<WindowDevice>,
    guard: Arc<DecodeGuard>,
    /// The window's renderer, which the pictures' images are handed to.
    renderer: wgpu::Device,
}

impl WindowDecoding {
    /// `None` when this window doesn't decode on its device: the device
    /// can't, it is switched off, the window isn't drawn with wgpu, the
    /// viewer converts colour on the CPU, or the decoder turns the device down.
    pub fn new(
        graphics: &WindowGraphics,
        render_state: Option<&eframe::egui_wgpu::RenderState>,
        convert: Convert,
    ) -> Option<WindowDecoding> {
        let decoding = graphics.device_decoding()?;
        let renderer = render_state?.device.clone();
        if convert == Convert::OnCpu {
            return None;
        }
        // Guarded like decoding itself: a driver that takes the app down
        // here is noticed at the next start, which then leaves this out.
        decoding.guard.about_to_decode();
        let made = WindowDevice::new(decoding.device.clone());
        decoding.guard.set_up();
        match made {
            Ok(device) => Some(WindowDecoding {
                device,
                guard: decoding.guard.clone(),
                renderer,
            }),
            Err(err) => {
                tracing::warn!("video is not decoded on the window's device: {}", err.report());
                None
            }
        }
    }

    /// The device and its watcher, for the preview worker.
    pub fn for_worker(&self) -> OnDevice {
        OnDevice {
            device: self.device.clone(),
            watch: Arc::new(Watch(self.guard.clone())),
        }
    }

    /// A picture decoded on the device as one the viewer draws. Nothing is
    /// copied. `None` when the renderer can't take the images, which the
    /// caller answers by decoding the usual way from then on.
    pub fn picture(&self, frame: &GpuFrame) -> Option<VideoPicture> {
        let (luma, chroma) = graphics::plane_textures(&self.renderer, &frame.planes)?;
        Some(VideoPicture::from_textures(PlaneTextures {
            luma,
            chroma,
            to_rgb: frame.colour.rgb_rows(),
            rotation: frame.rotation,
        }))
    }

    /// A picture decoded on the device was drawn, and the frame it was in
    /// went to the screen.
    pub fn drawn(&self) {
        self.guard.picture_drawn();
    }

    /// The renderer turned a picture down: an error, not a crash.
    pub fn failed(&self) {
        self.guard.reported_an_error();
    }
}
