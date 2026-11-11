//! The graphics device the measurements run on, and the small things every
//! measurement needs from it: textures, waiting, and the card's own clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// What the device is, for the report.
    pub about: String,
    /// Whether the card can stamp the start and end of a pass with its own clock.
    pub stamps: bool,
}

/// Opens the fastest device `backends` offers, or any at all when that finds
/// nothing.
pub fn open(backends: wgpu::Backends) -> Result<Gpu, String> {
    let adapter = match adapter(backends) {
        Some(adapter) => adapter,
        None => adapter(wgpu::Backends::all()).ok_or("this machine has no graphics device wgpu can use")?,
    };
    let info = adapter.get_info();
    let stamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
    let descriptor = wgpu::DeviceDescriptor {
        label: Some("render-bench"),
        required_features: if stamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
        .map_err(|err| format!("the device didn't open: {err}"))?;
    let about = format!(
        "{} ({:?}, {:?}), driver {} {}",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    );
    Ok(Gpu {
        device,
        queue,
        about,
        stamps,
    })
}

fn adapter(backends: wgpu::Backends) -> Option<wgpu::Adapter> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    let instance = wgpu::Instance::new(descriptor);
    let options = wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    };
    pollster::block_on(instance.request_adapter(&options)).ok()
}

impl Gpu {
    pub fn texture(
        &self,
        label: &str,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
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
            usage,
            view_formats: &[],
        })
    }

    /// Fills a texture from rows without padding.
    pub fn write(&self, texture: &wgpu::Texture, bytes: &[u8], bytes_per_sample: u32) {
        self.queue.write_texture(
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

    /// Waits until everything handed to the card so far is done.
    pub fn wait(&self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|err| format!("waiting for the card failed: {err}"))
    }

    /// Reads a buffer back.
    pub fn read(&self, buffer: &wgpu::Buffer) -> Result<Vec<u8>, String> {
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            if result.is_err() {
                flag.store(true, Ordering::Relaxed);
            }
        });
        self.wait()?;
        if failed.load(Ordering::Relaxed) {
            return Err("a buffer couldn't be read back".into());
        }
        let bytes = buffer
            .get_mapped_range(..)
            .map_err(|err| format!("a buffer couldn't be read back: {err}"))?
            .to_vec();
        buffer.unmap();
        Ok(bytes)
    }
}

/// A mark that a submission sets when the card has finished it.
#[derive(Clone, Default)]
pub struct Done(Arc<AtomicBool>);

impl Done {
    /// Has `encoder`'s work report here when it is finished.
    pub fn of(encoder: &wgpu::CommandEncoder) -> Done {
        let done = Done::default();
        let flag = done.0.clone();
        encoder.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        done
    }

    pub fn is(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Looks again and again, without blocking the device for anyone else,
    /// until the work is done or `limit` has passed. The moment it was seen
    /// done, or nothing after the limit.
    pub fn watch(&self, gpu: &Gpu, limit: Duration, rest: Option<Duration>) -> Option<Instant> {
        let give_up = Instant::now() + limit;
        loop {
            // Callbacks run inside a poll, whoever makes it.
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            if self.is() {
                return Some(Instant::now());
            }
            if Instant::now() >= give_up {
                return None;
            }
            match rest {
                Some(rest) => std::thread::sleep(rest),
                None => std::hint::spin_loop(),
            }
        }
    }
}

/// The card's own clock: two stamps a picture, read back together.
pub struct Stamps {
    set: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readable: wgpu::Buffer,
    pictures: u32,
}

impl Stamps {
    pub fn new(gpu: &Gpu, pictures: u32) -> Option<Stamps> {
        if !gpu.stamps {
            return None;
        }
        let bytes = u64::from(pictures) * 2 * u64::from(wgpu::QUERY_SIZE);
        let set = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: pictures * 2,
        });
        let buffer = |usage| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("stamps"),
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Stamps {
            set,
            resolved: buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readable: buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
            pictures,
        })
    }

    /// What pass `pass` of `passes` writes for picture `picture`: the first
    /// pass stamps the start, the last the end.
    pub fn writes(
        &self,
        picture: u32,
        pass: usize,
        passes: usize,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let first = pass == 0;
        let last = pass + 1 == passes;
        (first || last).then(|| wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: first.then_some(picture * 2),
            end_of_pass_write_index: last.then_some(picture * 2 + 1),
        })
    }

    /// Milliseconds on the card for each picture.
    pub fn read(&self, gpu: &Gpu) -> Result<Vec<f64>, String> {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.resolve_query_set(&self.set, 0..self.pictures * 2, &self.resolved, 0);
        encoder.copy_buffer_to_buffer(&self.resolved, 0, &self.readable, 0, self.resolved.size());
        gpu.queue.submit([encoder.finish()]);
        let bytes = gpu.read(&self.readable)?;
        let ticks: Vec<u64> = bytes
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();
        let nanoseconds_a_tick = f64::from(gpu.queue.get_timestamp_period());
        Ok(ticks
            .chunks_exact(2)
            .map(|pair| pair[1].saturating_sub(pair[0]) as f64 * nanoseconds_a_tick / 1e6)
            .collect())
    }
}
