//! Reading a picture's planes back from the graphics card into memory.
//!
//! Frames kept for playback live on the card; when the card's share for
//! them is full, the ones furthest from the playhead can go on in the
//! computer's memory instead. [`Reader`] brings a pair of [`PlaneImages`]
//! down: one submission copies both images into a buffer the processor
//! can read (luma, then interleaved colour, without padding), it is waited
//! for, and the bytes are copied out. The images are left as they were,
//! ready for sampling.
//!
//! Like [`crate::Copier`] it submits to the helper queue, locked by the
//! caller's `lock` and `unlock` around the submission.

use crate::{PlaneImages, VideoDevice, chroma_size};
use ash::vk;
use std::sync::Arc;

/// How long a read may take before it counts as stuck (it normally takes
/// a millisecond or two).
const READ_TIMEOUT_NS: u64 = 5_000_000_000;

/// The bytes a picture of `size` takes in memory: luma, then interleaved
/// colour at half size.
pub fn planes_bytes(size: (u32, u32)) -> u64 {
    let (cw, ch) = chroma_size(size);
    u64::from(size.0) * u64::from(size.1) + 2 * u64::from(cw) * u64::from(ch)
}

/// A buffer the processor reads, as large as the largest picture read yet.
struct Staging {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    bytes: u64,
}

/// The command buffer, fence and buffer for reading pictures back, one
/// after another. Used by one thread at a time.
pub struct Reader {
    device: Arc<VideoDevice>,
    queue: vk::Queue,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    staging: Option<Staging>,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reader")
    }
}

impl Reader {
    pub fn new(device: &Arc<VideoDevice>) -> Result<Reader, String> {
        let vulkan = device.device();
        let family = device.helper().index;
        let failed = |what: &str, err: vk::Result| format!("{what} failed ({err})");
        // SAFETY: the device is alive for as long as the handle; the helper
        // family has its queue 0; what is made here is destroyed on the
        // error paths and in `drop`.
        unsafe {
            let queue = vulkan.get_device_queue(family, 0);
            let pool = vulkan
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(|e| failed("making a command pool", e))?;
            let commands = match vulkan.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            ) {
                Ok(buffers) => buffers[0],
                Err(err) => {
                    vulkan.destroy_command_pool(pool, None);
                    return Err(failed("making a command buffer", err));
                }
            };
            let fence = match vulkan.create_fence(&vk::FenceCreateInfo::default(), None) {
                Ok(fence) => fence,
                Err(err) => {
                    vulkan.destroy_command_pool(pool, None);
                    return Err(failed("making a fence", err));
                }
            };
            Ok(Reader {
                device: Arc::clone(device),
                queue,
                pool,
                commands,
                fence,
                staging: None,
            })
        }
    }

    /// The picture in `planes`, as bytes: luma (one a pixel, row after
    /// row), then interleaved colour at half size ([`planes_bytes`] in all).
    /// Waits until the card has done it.
    ///
    /// # Safety
    /// - `planes` belongs to this reader's device and holds a picture (it
    ///   was copied in and that copy was waited for), in the layout for
    ///   sampling.
    /// - Nothing else uses `planes` on the card while this runs (the
    ///   caller holds the only reference to it).
    pub unsafe fn read(
        &mut self,
        planes: &PlaneImages,
        lock: &dyn Fn(),
        unlock: &dyn Fn(),
    ) -> Result<Vec<u8>, String> {
        let size = planes.size();
        let bytes = planes_bytes(size);
        let luma_bytes = u64::from(size.0) * u64::from(size.1);
        self.make_room(bytes)?;
        let Some(staging) = &self.staging else {
            return Err("no buffer to read into".into());
        };
        let (buffer, memory) = (staging.buffer, staging.memory);
        let vulkan = self.device.device();
        let failed = |what: &str, err: vk::Result| format!("{what} failed ({err})");
        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let sampled = (
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::AccessFlags::SHADER_READ,
        );
        let reading = (
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::AccessFlags::TRANSFER_READ,
        );
        let change =
            |image, from: (vk::ImageLayout, vk::AccessFlags), to: (vk::ImageLayout, vk::AccessFlags)| {
                vk::ImageMemoryBarrier::default()
                    .image(image)
                    .old_layout(from.0)
                    .src_access_mask(from.1)
                    .new_layout(to.0)
                    .dst_access_mask(to.1)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .subresource_range(whole)
            };
        let images = [
            (planes.luma(), size, 0),
            (planes.chroma(), chroma_size(size), luma_bytes),
        ];
        let before: Vec<_> = images
            .iter()
            .map(|(image, _, _)| change(*image, sampled, reading))
            .collect();
        let after: Vec<_> = images
            .iter()
            .map(|(image, _, _)| change(*image, reading, sampled))
            .collect();
        let into_buffer = [vk::BufferMemoryBarrier::default()
            .buffer(buffer)
            .offset(0)
            .size(bytes)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)];
        // SAFETY: the command buffer, fence and buffer are this reader's and
        // idle (every read is waited for before returning); the images are
        // valid, filled and in the layout for sampling, and nothing else
        // uses them (this function's contract); the buffer holds `bytes`;
        // the queue is locked around the submission; the arrays passed to
        // Vulkan outlive the calls that read them; the memory is mapped only
        // after the work is done, and unmapped before returning.
        unsafe {
            vulkan
                .reset_command_buffer(self.commands, vk::CommandBufferResetFlags::empty())
                .map_err(|e| failed("resetting the command buffer", e))?;
            vulkan
                .reset_fences(&[self.fence])
                .map_err(|e| failed("resetting the fence", e))?;
            vulkan
                .begin_command_buffer(
                    self.commands,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| failed("starting the commands", e))?;
            vulkan.cmd_pipeline_barrier(
                self.commands,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &before,
            );
            for (image, (width, height), offset) in images {
                let region = vk::BufferImageCopy::default()
                    .buffer_offset(offset)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    });
                vulkan.cmd_copy_image_to_buffer(
                    self.commands,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer,
                    &[region],
                );
            }
            vulkan.cmd_pipeline_barrier(
                self.commands,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &into_buffer,
                &after,
            );
            vulkan
                .end_command_buffer(self.commands)
                .map_err(|e| failed("finishing the commands", e))?;
            let commands = [self.commands];
            let submission = [vk::SubmitInfo::default().command_buffers(&commands)];
            lock();
            let submitted = vulkan.queue_submit(self.queue, &submission, self.fence);
            unlock();
            submitted.map_err(|e| failed("submitting the read", e))?;
            match vulkan.wait_for_fences(&[self.fence], true, READ_TIMEOUT_NS) {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => {
                    // Left to finish: `drop` waits for it before the buffer goes.
                    return Err(
                        "the graphics card didn't finish reading the picture back in five seconds".into(),
                    );
                }
                Err(other) => return Err(format!("waiting for the read failed ({other})")),
            }
            let mapped = vulkan
                .map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())
                .map_err(|e| failed("reading the buffer", e))?;
            let len = usize::try_from(bytes).map_err(|_| "a picture too large to read back".to_string())?;
            let read = std::slice::from_raw_parts(mapped.cast::<u8>(), len).to_vec();
            vulkan.unmap_memory(memory);
            Ok(read)
        }
    }

    /// Makes sure the buffer holds at least `bytes`.
    fn make_room(&mut self, bytes: u64) -> Result<(), String> {
        if self.staging.as_ref().is_some_and(|s| s.bytes >= bytes) {
            return Ok(());
        }
        if let Some(old) = self.staging.take() {
            // SAFETY: made by this reader; idle (every read was waited for).
            unsafe { self.destroy(old) };
        }
        self.staging = Some(self.create(bytes)?);
        Ok(())
    }

    fn create(&self, bytes: u64) -> Result<Staging, String> {
        let vulkan = self.device.device();
        let failed = |what: &str, err: vk::Result| format!("{what} failed ({err})");
        // SAFETY: the device is alive for as long as the handle; what is
        // made here is destroyed on the error paths.
        unsafe {
            let buffer = vulkan
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(bytes)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .map_err(|e| failed("making a buffer to read into", e))?;
            let needs = vulkan.get_buffer_memory_requirements(buffer);
            let Some(kind) = readable_memory(&self.device, needs.memory_type_bits) else {
                vulkan.destroy_buffer(buffer, None);
                return Err("the graphics card has no memory the processor can read".into());
            };
            let memory = match vulkan.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            ) {
                Ok(memory) => memory,
                Err(err) => {
                    vulkan.destroy_buffer(buffer, None);
                    return Err(failed("getting memory to read into", err));
                }
            };
            if let Err(err) = vulkan.bind_buffer_memory(buffer, memory, 0) {
                vulkan.destroy_buffer(buffer, None);
                vulkan.free_memory(memory, None);
                return Err(failed("giving the buffer its memory", err));
            }
            Ok(Staging {
                buffer,
                memory,
                bytes,
            })
        }
    }

    /// # Safety
    /// Made by this reader, and no work on the card uses it any more.
    unsafe fn destroy(&self, staging: Staging) {
        // SAFETY: this function's contract.
        unsafe {
            self.device.device().destroy_buffer(staging.buffer, None);
            self.device.device().free_memory(staging.memory, None);
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: the fence is this reader's. A read that timed out is
        // given its time before what it uses goes.
        unsafe {
            let vulkan = self.device.device();
            let _ = vulkan.wait_for_fences(&[self.fence], true, READ_TIMEOUT_NS);
        }
        if let Some(staging) = self.staging.take() {
            // SAFETY: made by this reader; its last read is done.
            unsafe { self.destroy(staging) };
        }
        // SAFETY: made by this reader on this device, and no longer in use.
        unsafe {
            let vulkan = self.device.device();
            vulkan.destroy_fence(self.fence, None);
            vulkan.destroy_command_pool(self.pool, None);
        }
    }
}

/// Memory the processor reads and that needs no flushing, cached when
/// there is such (reading uncached memory is many times slower).
fn readable_memory(device: &VideoDevice, allowed: u32) -> Option<u32> {
    // SAFETY: the instance and the card are alive for as long as the handle.
    let memory = unsafe {
        device
            .instance()
            .get_physical_device_memory_properties(device.physical_device())
    };
    let kinds = memory.memory_types_as_slice();
    let has = |i: usize, flags: vk::MemoryPropertyFlags| {
        allowed & (1 << i) != 0 && kinds[i].property_flags.contains(flags)
    };
    let visible = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let cached = visible | vk::MemoryPropertyFlags::HOST_CACHED;
    (0..kinds.len())
        .find(|i| has(*i, cached))
        .or_else(|| (0..kinds.len()).find(|i| has(*i, visible)))
        .map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Copier, Source, SourceImage};

    /// A copy of a 16×16 picture of known bytes, cropped to `visible`, read
    /// back: the bytes come back in order, luma then colour.
    #[test]
    fn a_picture_on_the_card_reads_back_as_its_bytes() {
        let Some(device) = crate::testing::device() else {
            return;
        };
        let luma: Vec<u8> = (0..16 * 16).map(|i| (i % 251) as u8).collect();
        let chroma: Vec<u8> = (0..8 * 8)
            .flat_map(|i| [100 + (i / 8) as u8, 200 + (i % 8) as u8])
            .collect();
        let picture = crate::testing::stand_in_picture(&device, (16, 16), &luma, &chroma);
        let mut copier = Copier::new(&device).unwrap();
        let mut reader = Reader::new(&device).unwrap();
        let mut layout = crate::testing::STAND_IN_LAYOUT;
        // A larger picture after a smaller one makes the buffer grow.
        for visible in [(10u32, 6u32), (16, 16), (12, 10)] {
            let planes = PlaneImages::new(&device, visible).unwrap();
            let source = Source::TwoPlanes(SourceImage {
                image: picture.0,
                layout,
                access: vk::AccessFlags::TRANSFER_READ,
                semaphore: vk::Semaphore::null(),
                value: 0,
            });
            // SAFETY: the stand-in is this device's, 16×16, in `layout`,
            // idle and alive past the wait; the target is only used here.
            unsafe {
                copier
                    .copy(&source, visible, (0, 0), &planes, &|| (), &|| ())
                    .unwrap()
            };
            layout = crate::LAYOUT_AFTER_COPY;
            copier.wait().unwrap();
            let locks = std::cell::Cell::new(0);
            // SAFETY: filled by the copy just waited for; only used here.
            let got = unsafe {
                reader
                    .read(&planes, &|| locks.set(locks.get() + 1), &|| ())
                    .unwrap()
            };
            assert_eq!(locks.get(), 1, "the queue is locked for the submission");
            let (w, h) = visible;
            let luma = &luma;
            let want_luma: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| luma[(x + 16 * y) as usize]))
                .collect();
            let (cw, ch) = chroma_size(visible);
            let want_chroma: Vec<u8> = (0..ch)
                .flat_map(|y| (0..cw).flat_map(move |x| [100 + y as u8, 200 + x as u8]))
                .collect();
            assert_eq!(got.len() as u64, planes_bytes(visible));
            assert_eq!(&got[..want_luma.len()], &want_luma[..], "luma of {visible:?}");
            assert_eq!(&got[want_luma.len()..], &want_chroma[..], "colour of {visible:?}");
            // Still a picture to sample afterwards: reading again gives the same.
            // SAFETY: as above.
            let again = unsafe { reader.read(&planes, &|| (), &|| ()).unwrap() };
            assert_eq!(again, got);
        }
        drop((copier, reader));
        crate::testing::destroy_stand_in(&device, picture);
    }

    #[test]
    fn a_picture_takes_its_luma_and_half_its_colour() {
        assert_eq!(planes_bytes((16, 16)), 16 * 16 + 2 * 8 * 8);
        assert_eq!(planes_bytes((3440, 1440)), 3440 * 1440 * 3 / 2);
        // Odd sizes round the colour up.
        assert_eq!(planes_bytes((3, 3)), 9 + 2 * 2 * 2);
    }
}
