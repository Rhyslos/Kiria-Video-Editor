//! A device for tests: opened here with plain Vulkan calls and no window, on
//! whatever Vulkan the machine has (a software driver will do).
//!
//! A machine with one queue family can't keep the decoder's queues apart
//! from the window's; the handle made here then names the same family for
//! both, which is fine for a test that uses one thread and draws nothing
//! else. It has no video decoding either: tests stand in an image of their
//! own for the decoder's.

// Test support may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::{Parts, QueueFamily, VideoDevice};
use ash::vk;
use std::sync::Arc;

/// Destroys the device and the instance once the last user is gone.
struct Owner {
    instance: ash::Instance,
    device: ash::Device,
    _entry: ash::Entry,
}

impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: everything made on the device is gone by now (the handle
        // drops its `keep_alive` last), and nothing else holds these.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// A Vulkan 1.3 device with one queue, or `None` (with a line on the error
/// output) where there is no Vulkan 1.3 to open.
pub fn device() -> Option<Arc<VideoDevice>> {
    let skipped = |why: String| {
        eprintln!("no Vulkan device for this check: {why}");
        None
    };
    // SAFETY: plain Vulkan set-up; each handle is used while its parent is
    // alive, and the `Owner` destroys device and instance in that order.
    unsafe {
        let entry = match ash::Entry::load() {
            Ok(entry) => entry,
            Err(err) => return skipped(format!("the Vulkan library didn't load ({err})")),
        };
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        let instance =
            match entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) {
                Ok(instance) => instance,
                Err(err) => return skipped(format!("no Vulkan 1.3 instance ({err})")),
            };
        let cards = instance.enumerate_physical_devices().unwrap_or_default();
        let found = cards.into_iter().find_map(|card| {
            let recent = instance.get_physical_device_properties(card).api_version >= vk::API_VERSION_1_3;
            let family = instance
                .get_physical_device_queue_family_properties(card)
                .iter()
                .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE) && f.queue_count > 0)?;
            recent.then_some((card, family as u32))
        });
        let Some((card, family)) = found else {
            instance.destroy_instance(None);
            return skipped("no card with Vulkan 1.3 and a compute queue".into());
        };
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let mut newer = vk::PhysicalDeviceVulkan13Features::default().synchronization2(true);
        let mut new = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
        let wanted = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .push_next(&mut newer)
            .push_next(&mut new);
        let device = match instance.create_device(card, &wanted, None) {
            Ok(device) => device,
            Err(err) => {
                instance.destroy_instance(None);
                return skipped(format!("the device didn't open ({err})"));
            }
        };
        let helper = QueueFamily {
            index: family,
            flags: vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
            codecs: vk::VideoCodecOperationFlagsKHR::empty(),
        };
        let parts = Parts {
            get_instance_proc_addr: entry.static_fn().get_instance_proc_addr,
            instance: instance.clone(),
            physical_device: card,
            device: device.clone(),
            instance_extensions: Vec::new(),
            device_extensions: Vec::new(),
            window_family: family,
            helper,
            decode: Vec::new(),
            codecs: Vec::new(),
            synchronization2: true,
            timeline_semaphore: true,
            video_maintenance1: false,
            keep_alive: Box::new(Owner {
                instance,
                device,
                _entry: entry,
            }),
        };
        // The handles are one live device's, described as it was created;
        // the "window's" queue is the helper's own here, and only the test
        // that asked for the device uses it.
        Some(Arc::new(VideoDevice::new(parts)))
    }
}

/// A two-plane picture made by hand, standing in for one from the decoder:
/// an image of `coded` size in the format FFmpeg's decoder uses (luma plane
/// and interleaved colour plane), filled from `luma` and `chroma` bytes
/// (rows without padding) and left in the layout for being copied from.
/// Destroy it with [`destroy_stand_in`].
///
/// # Panics
/// When the driver refuses any step: this is test support.
pub fn stand_in_picture(
    device: &VideoDevice,
    coded: (u32, u32),
    luma: &[u8],
    chroma: &[u8],
) -> (vk::Image, vk::DeviceMemory) {
    let vulkan = device.device();
    let (width, height) = coded;
    assert_eq!(luma.len(), (width * height) as usize);
    assert_eq!(
        chroma.len(),
        (width.div_ceil(2) * height.div_ceil(2) * 2) as usize
    );
    // SAFETY: test support on a device the caller keeps alive; every object
    // is destroyed before returning, except the image and its memory, which
    // the caller destroys.
    unsafe {
        let memory_for = |needs: vk::MemoryRequirements, wanted: vk::MemoryPropertyFlags| {
            let memory = device
                .instance()
                .get_physical_device_memory_properties(device.physical_device());
            let kind = memory
                .memory_types_as_slice()
                .iter()
                .enumerate()
                .position(|(i, t)| {
                    needs.memory_type_bits & (1 << i) != 0 && t.property_flags.contains(wanted)
                })
                .expect("a memory type");
            vulkan
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind as u32),
                    None,
                )
                .expect("memory")
        };
        let image = vulkan
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST),
                None,
            )
            .expect("a two-plane image");
        let image_memory = memory_for(
            vulkan.get_image_memory_requirements(image),
            vk::MemoryPropertyFlags::empty(),
        );
        vulkan.bind_image_memory(image, image_memory, 0).expect("bind");

        // The bytes go through a buffer: luma first, then colour.
        let bytes: Vec<u8> = [luma, chroma].concat();
        let buffer = vulkan
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )
            .expect("a buffer");
        let buffer_memory = memory_for(
            vulkan.get_buffer_memory_requirements(buffer),
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        );
        vulkan.bind_buffer_memory(buffer, buffer_memory, 0).expect("bind");
        let mapped = vulkan
            .map_memory(buffer_memory, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
            .expect("map");
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
        vulkan.unmap_memory(buffer_memory);

        let family = device.helper().index;
        let queue = vulkan.get_device_queue(family, 0);
        let pool = vulkan
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
            .expect("a command pool");
        let commands = vulkan
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .expect("a command buffer")[0];
        vulkan
            .begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to = |from, into, before, after| {
            [vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(from)
                .new_layout(into)
                .src_access_mask(before)
                .dst_access_mask(after)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .subresource_range(whole)]
        };
        let everything = vk::PipelineStageFlags::ALL_COMMANDS;
        vulkan.cmd_pipeline_barrier(
            commands,
            everything,
            everything,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to(
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            ),
        );
        let plane = |aspect, offset: usize, size: (u32, u32)| {
            vk::BufferImageCopy::default()
                .buffer_offset(offset as u64)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(aspect)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: size.0,
                    height: size.1,
                    depth: 1,
                })
        };
        vulkan.cmd_copy_buffer_to_image(
            commands,
            buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[
                plane(vk::ImageAspectFlags::PLANE_0, 0, coded),
                plane(
                    vk::ImageAspectFlags::PLANE_1,
                    luma.len(),
                    (width.div_ceil(2), height.div_ceil(2)),
                ),
            ],
        );
        vulkan.cmd_pipeline_barrier(
            commands,
            everything,
            everything,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to(
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::GENERAL,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            ),
        );
        vulkan.end_command_buffer(commands).expect("end");
        let buffers = [commands];
        vulkan
            .queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&buffers)],
                vk::Fence::null(),
            )
            .expect("submit");
        vulkan.queue_wait_idle(queue).expect("wait");
        vulkan.destroy_command_pool(pool, None);
        vulkan.destroy_buffer(buffer, None);
        vulkan.free_memory(buffer_memory, None);
        (image, image_memory)
    }
}

/// The layout [`stand_in_picture`] leaves its image in.
pub const STAND_IN_LAYOUT: vk::ImageLayout = vk::ImageLayout::GENERAL;

/// Destroys what [`stand_in_picture`] made.
pub fn destroy_stand_in(device: &VideoDevice, picture: (vk::Image, vk::DeviceMemory)) {
    // SAFETY: made by `stand_in_picture` on this device; the caller is done with it.
    unsafe {
        device.device().destroy_image(picture.0, None);
        device.device().free_memory(picture.1, None);
    }
}

/// Reads an image of `size` back as bytes (`bytes_per_pixel` each), from
/// the layout for sampling, which it is left in.
pub fn read_back(device: &VideoDevice, image: vk::Image, size: (u32, u32), bytes_per_pixel: u32) -> Vec<u8> {
    let vulkan = device.device();
    let length = u64::from(size.0 * size.1 * bytes_per_pixel);
    // SAFETY: test support on a device the caller keeps alive; the image is
    // the caller's, idle, and in the layout for sampling; everything made
    // here is destroyed before returning.
    unsafe {
        let buffer = vulkan
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(length)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
            .expect("a buffer");
        let needs = vulkan.get_buffer_memory_requirements(buffer);
        let memory = device
            .instance()
            .get_physical_device_memory_properties(device.physical_device());
        let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let kind = memory
            .memory_types_as_slice()
            .iter()
            .enumerate()
            .position(|(i, t)| needs.memory_type_bits & (1 << i) != 0 && t.property_flags.contains(wanted))
            .expect("a memory type");
        let buffer_memory = vulkan
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind as u32),
                None,
            )
            .expect("memory");
        vulkan.bind_buffer_memory(buffer, buffer_memory, 0).expect("bind");
        let family = device.helper().index;
        let queue = vulkan.get_device_queue(family, 0);
        let pool = vulkan
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
            .expect("a command pool");
        let commands = vulkan
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .expect("a command buffer")[0];
        vulkan
            .begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        // Sampling's layout doesn't allow copying out: borrow the image for a moment.
        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to = |from, into, before, after| {
            [vk::ImageMemoryBarrier::default()
                .image(image)
                .old_layout(from)
                .new_layout(into)
                .src_access_mask(before)
                .dst_access_mask(after)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .subresource_range(whole)]
        };
        let everything = vk::PipelineStageFlags::ALL_COMMANDS;
        let (sampled, general) = (
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        vulkan.cmd_pipeline_barrier(
            commands,
            everything,
            everything,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to(
                sampled,
                general,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::TRANSFER_READ,
            ),
        );
        let all = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: size.0,
                height: size.1,
                depth: 1,
            });
        vulkan.cmd_copy_image_to_buffer(commands, image, general, buffer, &[all]);
        vulkan.cmd_pipeline_barrier(
            commands,
            everything,
            everything,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to(
                general,
                sampled,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::SHADER_READ,
            ),
        );
        vulkan.end_command_buffer(commands).expect("end");
        let buffers = [commands];
        vulkan
            .queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&buffers)],
                vk::Fence::null(),
            )
            .expect("submit");
        vulkan.queue_wait_idle(queue).expect("wait");
        let mapped = vulkan
            .map_memory(buffer_memory, 0, length, vk::MemoryMapFlags::empty())
            .expect("map");
        let bytes = std::slice::from_raw_parts(mapped.cast::<u8>(), length as usize).to_vec();
        vulkan.unmap_memory(buffer_memory);
        vulkan.destroy_command_pool(pool, None);
        vulkan.destroy_buffer(buffer, None);
        vulkan.free_memory(buffer_memory, None);
        bytes
    }
}

/// The name of the software driver's device, for a test's notes.
pub fn card_name(device: &VideoDevice) -> String {
    // SAFETY: the instance and the card are alive for as long as the handle.
    let properties = unsafe {
        device
            .instance()
            .get_physical_device_properties(device.physical_device())
    };
    properties
        .device_name_as_c_str()
        .unwrap_or(c"?")
        .to_string_lossy()
        .into_owned()
}
