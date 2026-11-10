//! Uploads decoded planes into wgpu textures, as the editor would before
//! drawing them. No window: the textures are written and the queue submitted.

use crate::decoder::{PlaneFormat, Planes};

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    textures: Vec<wgpu::Texture>,
    size: (u32, u32, Option<PlaneFormat>),
    pub adapter: String,
}

impl Gpu {
    pub fn new() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("no GPU device: {e}"))?;
        Ok(Gpu {
            device,
            queue,
            textures: Vec::new(),
            size: (0, 0, None),
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    fn ensure(&mut self, p: &Planes) {
        if self.size == (p.width, p.height, Some(p.format)) {
            return;
        }
        let (cw, ch) = (p.width.div_ceil(2), p.height.div_ceil(2));
        let make = |w, h, format| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
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
        self.textures = match p.format {
            PlaneFormat::Nv12 => vec![
                make(p.width, p.height, wgpu::TextureFormat::R8Unorm),
                make(cw, ch, wgpu::TextureFormat::Rg8Unorm),
            ],
            PlaneFormat::Yuv420p => vec![
                make(p.width, p.height, wgpu::TextureFormat::R8Unorm),
                make(cw, ch, wgpu::TextureFormat::R8Unorm),
                make(cw, ch, wgpu::TextureFormat::R8Unorm),
            ],
            PlaneFormat::OnGpu => Vec::new(),
        };
        self.size = (p.width, p.height, Some(p.format));
    }

    /// Copies one frame's planes to the GPU.
    pub fn upload(&mut self, p: &Planes) {
        if p.format == PlaneFormat::OnGpu {
            return; // already there
        }
        self.ensure(p);
        for (texture, (data, stride)) in self.textures.iter().zip(&p.planes) {
            let size = texture.size();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(*stride),
                    rows_per_image: None,
                },
                size,
            );
        }
        self.queue.submit([]);
    }

    /// Waits until everything sent so far has reached the GPU.
    pub fn finish(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}
