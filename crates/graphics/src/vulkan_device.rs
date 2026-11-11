//! Opening the window's Vulkan device ourselves, with video decoding switched on.
//!
//! Left alone, the window toolkit opens a device that can draw and nothing
//! else. Decoding video straight into the window's device
//! (`docs/research/decoding.md`, approach C) needs the same device opened
//! with its video decode queues and extensions, which only the one who
//! creates the device can ask for. So for a start with Vulkan this module
//! opens the device through the renderer's lower layer, adds what video
//! decoding needs, and hands the result to the toolkit as an existing device.
//!
//! Anything missing (an older driver, no decode queue, no spare queue for
//! the decoder's side) is an ordinary error with a reason: the caller then
//! lets the toolkit open its own device as before, and Diagnostics says why.
//!
//! The device is opened exactly as the toolkit would open it (same card
//! rule, same limits and features), plus:
//!
//! - the video extensions the driver has (the decode queue, H.264, H.265,
//!   AV1, and the first maintenance extension);
//! - one queue in each family that decodes video, and one in a compute
//!   family other than the window's (the helper queue: see
//!   `video_device` for who submits where);
//! - two device features FFmpeg's decoder uses that the renderer doesn't
//!   switch on by itself.
//!
//! This is the crate's GPU wrapper: all of its `unsafe` is in this file.

#![allow(unsafe_code)]

use eframe::{egui_wgpu, wgpu};
use std::ffi::{CStr, CString};
use std::sync::Arc;
use video_device::ash::{self, vk};
use video_device::{Codec, Parts, PlaneImages, QueueFamily, VideoDevice};

/// The window's device, opened here, and what the decoder needs to share it.
pub(crate) struct OwnDevice {
    pub setup: egui_wgpu::WgpuSetupExisting,
    pub video: Arc<VideoDevice>,
}

/// One queue family as the card reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Family {
    pub flags: vk::QueueFlags,
    pub count: u32,
    /// The kinds of video a decode family takes.
    pub video: vk::VideoCodecOperationFlagsKHR,
}

/// What to add to the device so that it can decode video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Additions {
    pub extensions: Vec<&'static CStr>,
    pub helper: QueueFamily,
    pub decode: Vec<QueueFamily>,
    pub codecs: Vec<Codec>,
    pub maintenance1: bool,
}

/// The renderer takes queue 0 of this family for the window, always (wgpu 30).
/// Checked again once the device is open.
const WINDOW_FAMILY: u32 = 0;

const VULKAN_1_3: u32 = vk::API_VERSION_1_3;

/// The extension that goes with each codec's decode operation.
fn codec_extension(codec: Codec) -> &'static CStr {
    match codec {
        Codec::H264 => ash::khr::video_decode_h264::NAME,
        Codec::H265 => ash::khr::video_decode_h265::NAME,
        Codec::Av1 => ash::khr::video_decode_av1::NAME,
    }
}

/// Decides what to add to the device, or says why this card and driver
/// can't decode video on the window's device. Pure: everything it needs is
/// passed in.
///
/// - `versions`: the Vulkan versions of the instance and of the card's driver.
/// - `families`: the card's queue families, in order.
/// - `has_extension`: whether the driver offers a device extension.
/// - `maintenance1_feature`: whether the driver supports the feature that
///   goes with the first video maintenance extension.
pub(crate) fn plan_video(
    versions: (u32, u32),
    families: &[Family],
    has_extension: impl Fn(&CStr) -> bool,
    maintenance1_feature: bool,
) -> Result<Additions, String> {
    if versions.0 < VULKAN_1_3 || versions.1 < VULKAN_1_3 {
        return Err("the graphics driver is older than Vulkan 1.3; updating it may help".into());
    }
    for needed in [ash::khr::video_queue::NAME, ash::khr::video_decode_queue::NAME] {
        if !has_extension(needed) {
            return Err(format!(
                "the graphics driver has no video decoding through Vulkan ({} is missing)",
                needed.to_string_lossy()
            ));
        }
    }
    // The decoder's queues must be the decoder's alone: never the window's.
    let spare = |index: &usize| *index as u32 != WINDOW_FAMILY && families[*index].count > 0;
    let decode: Vec<QueueFamily> = (0..families.len())
        .filter(spare)
        .filter(|i| families[*i].flags.contains(vk::QueueFlags::VIDEO_DECODE_KHR))
        .map(|i| QueueFamily {
            index: i as u32,
            flags: vk::QueueFlags::VIDEO_DECODE_KHR,
            codecs: families[i].video,
        })
        .collect();
    if decode.is_empty() {
        return Err("the graphics card offers no video decode queue".into());
    }
    let offered = decode
        .iter()
        .fold(vk::VideoCodecOperationFlagsKHR::empty(), |all, f| all | f.codecs);
    let codecs: Vec<Codec> = Codec::ALL
        .into_iter()
        .filter(|c| offered.contains(c.operation()) && has_extension(codec_extension(*c)))
        .collect();
    if codecs.is_empty() {
        return Err("the graphics card decodes none of H.264, H.265 and AV1 through Vulkan".into());
    }
    // The helper queue: compute work in a family of its own. The plainest
    // such family is taken (the fewest other abilities).
    let helper = (0..families.len())
        .filter(spare)
        .filter(|i| families[*i].flags.contains(vk::QueueFlags::COMPUTE))
        .min_by_key(|i| families[*i].flags.as_raw().count_ones())
        .map(|i| QueueFamily {
            index: i as u32,
            flags: vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
            codecs: vk::VideoCodecOperationFlagsKHR::empty(),
        })
        .ok_or_else(|| {
            "the graphics card has no spare queue for the decoder besides the one that draws the window"
                .to_owned()
        })?;
    let maintenance1 = maintenance1_feature && has_extension(ash::khr::video_maintenance1::NAME);
    let mut extensions = vec![ash::khr::video_queue::NAME, ash::khr::video_decode_queue::NAME];
    extensions.extend(codecs.iter().map(|c| codec_extension(*c)));
    if maintenance1 {
        extensions.push(ash::khr::video_maintenance1::NAME);
    }
    Ok(Additions {
        extensions,
        helper,
        decode,
        codecs,
        maintenance1,
    })
}

/// Opens Vulkan alone, picks the card by the usual rule and opens its
/// device with video decoding. The reason comes back in words when any
/// step can't be done.
///
/// `instance` and `device` are the toolkit's own descriptions of the
/// instance and the device it would make, so that this device differs from
/// the toolkit's only in what video decoding adds.
pub(crate) fn open(
    instance: wgpu::InstanceDescriptor,
    prefer_integrated: bool,
    device: &dyn Fn(&wgpu::Adapter) -> wgpu::DeviceDescriptor<'static>,
) -> Result<OwnDevice, String> {
    let instance = wgpu::Instance::new(instance);
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    // No window yet to check against: a card that turns out unable to draw
    // to it makes the toolkit report an error, and the next try picks with
    // the window in hand.
    let adapter = crate::start::pick_card(&adapters, None, prefer_integrated)?;
    let descriptor = device(&adapter);
    open_on(&instance, &adapter, &descriptor)
}

/// [`open`] on a card already chosen.
pub(crate) fn open_on(
    instance: &wgpu::Instance,
    adapter: &wgpu::Adapter,
    descriptor: &wgpu::DeviceDescriptor<'static>,
) -> Result<OwnDevice, String> {
    // SAFETY: the adapter's own Vulkan handles are only read here, while the
    // adapter is alive.
    let (found, additions) = unsafe {
        let hal_adapter = adapter
            .as_hal::<wgpu::hal::api::Vulkan>()
            .ok_or("the card isn't driven through Vulkan")?;
        let found = Found::read(&hal_adapter);
        let has_extension = |name: &CStr| {
            hal_adapter
                .physical_device_capabilities()
                .supports_extension(name)
        };
        let additions = plan_video(
            found.versions,
            &found.families,
            has_extension,
            found.maintenance1_feature,
        )?;
        (found, additions)
    };
    open_with(instance, adapter, descriptor, &found, additions)
}

/// What is read from the card before deciding.
struct Found {
    versions: (u32, u32),
    families: Vec<Family>,
    maintenance1_feature: bool,
}

impl Found {
    /// # Safety
    /// `adapter` must be alive (its instance and physical device are used).
    unsafe fn read(adapter: &wgpu::hal::vulkan::Adapter) -> Found {
        let shared = adapter.shared_instance();
        let (instance, card) = (shared.raw_instance(), adapter.raw_physical_device());
        let device_version = adapter.physical_device_capabilities().properties().api_version;
        let versions = (shared.instance_api_version(), device_version);
        // Asking about video queues and features needs Vulkan 1.1 calls and
        // structures a 1.3 driver knows; an older one is turned away by the
        // version alone.
        if versions.0 < VULKAN_1_3 || versions.1 < VULKAN_1_3 {
            return Found {
                versions,
                families: Vec::new(),
                maintenance1_feature: false,
            };
        }
        // SAFETY: the instance and the card are alive (this function's
        // contract); each structure chained for the driver to fill outlives
        // the call that fills it.
        unsafe {
            let capabilities = adapter.physical_device_capabilities();
            let count = instance.get_physical_device_queue_family_properties2_len(card);
            let mut video = vec![vk::QueueFamilyVideoPropertiesKHR::default(); count];
            // The driver only knows the video structure with the extension.
            let with_video = capabilities.supports_extension(ash::khr::video_queue::NAME);
            let basics: Vec<(vk::QueueFlags, u32)> = {
                let mut properties: Vec<vk::QueueFamilyProperties2<'_>> = video
                    .iter_mut()
                    .map(|v| {
                        let basic = vk::QueueFamilyProperties2::default();
                        if with_video { basic.push_next(v) } else { basic }
                    })
                    .collect();
                instance.get_physical_device_queue_family_properties2(card, &mut properties);
                properties
                    .iter()
                    .map(|p| p.queue_family_properties)
                    .map(|p| (p.queue_flags, p.queue_count))
                    .collect()
            };
            let families = basics
                .into_iter()
                .zip(&video)
                .map(|((flags, count), v)| Family {
                    flags,
                    count,
                    video: v.video_codec_operations,
                })
                .collect();
            let mut maintenance1 = vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default();
            if capabilities.supports_extension(ash::khr::video_maintenance1::NAME) {
                let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut maintenance1);
                instance.get_physical_device_features2(card, &mut features);
            }
            Found {
                versions,
                families,
                maintenance1_feature: maintenance1.video_maintenance1 == vk::TRUE,
            }
        }
    }
}

/// Every queue gets the same priority; there is one per family.
static PRIORITY: [f32; 1] = [1.0];

/// Opens the device with `additions` and wraps it for the toolkit and the decoder.
fn open_with(
    instance: &wgpu::Instance,
    adapter: &wgpu::Adapter,
    descriptor: &wgpu::DeviceDescriptor<'static>,
    found: &Found,
    additions: Additions,
) -> Result<OwnDevice, String> {
    // Features FFmpeg's decoder uses that the renderer doesn't ask for.
    // Vulkan 1.3 always has the first; the second was asked about above.
    let mut synchronization2 = vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let mut maintenance1 = vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default().video_maintenance1(true);
    let mut queue_families: Vec<u32> = additions.decode.iter().map(|f| f.index).collect();
    queue_families.push(additions.helper.index);
    let (extensions, with_maintenance1) = (&additions.extensions, additions.maintenance1);
    // Borrowed, not moved in: the device is created from these after the
    // callback has returned.
    let (synchronization2, maintenance1) = (&mut synchronization2, &mut maintenance1);
    let add: Box<wgpu::hal::vulkan::CreateDeviceCallback<'_>> = Box::new(move |args| {
        for extension in extensions {
            if !args.extensions.contains(extension) {
                args.extensions.push(extension);
            }
        }
        for family in queue_families {
            let listed = args
                .queue_create_infos
                .iter()
                .any(|q| q.queue_family_index == family);
            if !listed {
                args.queue_create_infos.push(
                    vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(family)
                        .queue_priorities(&PRIORITY),
                );
            }
        }
        let mut chain = std::mem::take(args.create_info).push_next(synchronization2);
        if with_maintenance1 {
            chain = chain.push_next(maintenance1);
        }
        *args.create_info = chain;
    });
    // SAFETY: the callback only adds: extensions the driver was asked about,
    // queues in families the card reported with at least one queue, and two
    // feature structures the renderer doesn't chain itself (it uses the
    // per-extension structures and none for these two), both supported by
    // this driver. The opened device is handed to the renderer, which owns
    // it from then on; the clones of its handles made here stay valid for as
    // long as the renderer's device, which `keep_alive` holds.
    unsafe {
        let hal_adapter = adapter
            .as_hal::<wgpu::hal::api::Vulkan>()
            .ok_or("the card isn't driven through Vulkan")?;
        let open = hal_adapter
            .open_with_callback(
                descriptor.required_features,
                &descriptor.required_limits,
                &descriptor.memory_hints,
                Some(add),
            )
            .map_err(|e| format!("the device didn't open with video decoding ({e})"))?;
        // The decoder's queues are only its own if the window's is another.
        let (window_family, window_queue) = (open.device.queue_family_index(), open.device.queue_index());
        if (window_family, window_queue) != (WINDOW_FAMILY, 0) {
            return Err(format!(
                "the renderer took queue {window_queue} of family {window_family} for the window, not the first"
            ));
        }
        let shared = hal_adapter.shared_instance();
        let owned =
            |names: &[&'static CStr]| -> Vec<CString> { names.iter().map(|n| (*n).to_owned()).collect() };
        let get_instance_proc_addr = shared.entry().static_fn().get_instance_proc_addr;
        let raw_instance = shared.raw_instance().clone();
        let instance_extensions = owned(shared.extensions());
        let physical_device = hal_adapter.raw_physical_device();
        let raw_device = open.device.raw_device().clone();
        let device_extensions = owned(open.device.enabled_device_extensions());
        drop(hal_adapter);
        let (device, queue) = adapter
            .create_device_from_hal(open, descriptor)
            .map_err(|e| format!("the renderer didn't take the device opened for video decoding ({e})"))?;
        let parts = Parts {
            get_instance_proc_addr,
            instance: raw_instance,
            physical_device,
            device: raw_device,
            instance_extensions,
            device_extensions,
            window_family,
            helper: additions.helper,
            decode: additions.decode,
            codecs: additions.codecs,
            synchronization2: true,
            // Part of Vulkan 1.2, and the renderer switches it on when it is there.
            timeline_semaphore: found.versions.1 >= vk::API_VERSION_1_2,
            video_maintenance1: additions.maintenance1,
            keep_alive: Box::new((instance.clone(), adapter.clone(), device.clone(), queue.clone())),
        };
        Ok(OwnDevice {
            setup: egui_wgpu::WgpuSetupExisting {
                instance: instance.clone(),
                adapter: adapter.clone(),
                device,
                queue,
            },
            // The handles are this device's, kept alive by the renderer's own
            // in `keep_alive`; the fields say how it was opened just above;
            // and the renderer only ever submits to the window's queue, which
            // is none of the queues listed.
            video: Arc::new(VideoDevice::new(parts)),
        })
    }
}

/// A pair of images holding a decoded picture, as two textures of the
/// window's renderer: luma (one byte a pixel) and colour (two bytes a
/// sample, at half size). Nothing is copied: the textures are the images.
/// The pair is kept alive for as long as the renderer uses either texture,
/// work still under way on the graphics card included, and goes back for
/// the next picture after that.
///
/// `None` when `device` isn't the device the images were made on.
pub fn plane_textures(
    device: &wgpu::Device,
    planes: &Arc<PlaneImages>,
) -> Option<(wgpu::Texture, wgpu::Texture)> {
    let size = planes.size();
    let texture = |image: vk::Image, format: wgpu::TextureFormat, (width, height): (u32, u32)| {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let keep = Arc::clone(planes);
        // SAFETY: the image is one of the pair's, made on this very device
        // (checked below before anything is made) with the format and size
        // described, for sampling, without the flag that would allow views
        // in other formats (none are asked for). It stays valid until the
        // callback runs, which is what drops this hold on the pair; its
        // memory is the pair's, not the renderer's. The copy that filled it
        // left it in the layout for sampling, which is the state named here,
        // so the renderer's first use starts from the truth.
        unsafe {
            let hal_device = device.as_hal::<wgpu::hal::api::Vulkan>()?;
            let raw = hal_device.texture_from_raw(
                image,
                &wgpu::hal::TextureDescriptor {
                    label: Some("decoded video plane"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUses::RESOURCE,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: Vec::new(),
                },
                Some(Box::new(move || drop(keep))),
                wgpu::hal::vulkan::TextureMemory::External,
            );
            drop(hal_device);
            Some(device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                raw,
                &wgpu::TextureDescriptor {
                    label: Some("decoded video plane"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::TextureUses::RESOURCE,
            ))
        }
    };
    // SAFETY: only compares the two devices' handles.
    let same_device = unsafe {
        let hal_device = device.as_hal::<wgpu::hal::api::Vulkan>()?;
        hal_device.raw_device().handle() == planes.device().device().handle()
    };
    if !same_device {
        return None;
    }
    let luma = texture(planes.luma(), wgpu::TextureFormat::R8Unorm, size)?;
    let chroma = texture(
        planes.chroma(),
        wgpu::TextureFormat::Rg8Unorm,
        video_device::chroma_size(size),
    )?;
    Some((luma, chroma))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vk::QueueFlags as Q;
    use vk::VideoCodecOperationFlagsKHR as Op;

    const V1_3: (u32, u32) = (vk::API_VERSION_1_3, vk::API_VERSION_1_3);

    fn family(flags: Q, count: u32) -> Family {
        Family {
            flags,
            count,
            video: Op::empty(),
        }
    }

    fn decoding(video: Op) -> Family {
        Family {
            flags: Q::VIDEO_DECODE_KHR | Q::TRANSFER,
            count: 1,
            video,
        }
    }

    const ALL_OPS: Op =
        Op::from_raw(Op::DECODE_H264.as_raw() | Op::DECODE_H265.as_raw() | Op::DECODE_AV1.as_raw());

    /// A card with separate families for drawing, copying, compute and decoding.
    fn many_families() -> Vec<Family> {
        vec![
            family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING, 16),
            family(Q::TRANSFER | Q::SPARSE_BINDING, 2),
            family(Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING, 8),
            decoding(ALL_OPS),
            family(Q::VIDEO_ENCODE_KHR | Q::TRANSFER, 1),
        ]
    }

    fn everything(_: &CStr) -> bool {
        true
    }

    #[test]
    fn a_card_with_decode_and_compute_families_gets_all_three_codecs() {
        let plan = plan_video(V1_3, &many_families(), everything, true).unwrap();
        assert_eq!(plan.codecs, Codec::ALL);
        assert_eq!(plan.decode.len(), 1);
        assert_eq!(plan.decode[0].index, 3);
        assert_eq!(
            plan.decode[0].flags,
            Q::VIDEO_DECODE_KHR,
            "used for decoding only"
        );
        assert_eq!(plan.decode[0].codecs, ALL_OPS);
        assert_eq!(plan.helper.index, 2, "the compute family, not the window's");
        assert_eq!(plan.helper.flags, Q::COMPUTE | Q::TRANSFER);
        assert!(plan.maintenance1);
        let names: Vec<_> = plan.extensions.iter().map(|n| n.to_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "VK_KHR_video_queue",
                "VK_KHR_video_decode_queue",
                "VK_KHR_video_decode_h264",
                "VK_KHR_video_decode_h265",
                "VK_KHR_video_decode_av1",
                "VK_KHR_video_maintenance1"
            ]
        );
    }

    #[test]
    fn the_window_queue_is_never_shared_with_the_decoder() {
        // One family does everything but decoding: no helper queue to spare.
        let single = vec![
            family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, 1),
            decoding(ALL_OPS),
        ];
        let why = plan_video(V1_3, &single, everything, true).unwrap_err();
        assert!(why.contains("no spare queue"), "{why}");
        // Even with many queues in the window's family: FFmpeg takes queues
        // from the first up, and the first is the window's.
        let wide = vec![
            family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, 16),
            decoding(ALL_OPS),
        ];
        assert!(plan_video(V1_3, &wide, everything, true).is_err());
        // Decoding offered only by the window's own family is no use either.
        let odd = vec![
            Family {
                flags: Q::GRAPHICS | Q::COMPUTE | Q::VIDEO_DECODE_KHR,
                count: 4,
                video: ALL_OPS,
            },
            family(Q::COMPUTE | Q::TRANSFER, 2),
        ];
        let why = plan_video(V1_3, &odd, everything, true).unwrap_err();
        assert!(why.contains("no video decode queue"), "{why}");
    }

    #[test]
    fn the_plainest_compute_family_becomes_the_helper() {
        let families = vec![
            family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, 1),
            family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING, 1),
            family(Q::COMPUTE | Q::TRANSFER, 2),
            family(Q::COMPUTE, 0), // listed, but with no queue to give
            decoding(Op::DECODE_H264),
        ];
        let plan = plan_video(V1_3, &families, everything, false).unwrap();
        assert_eq!(plan.helper.index, 2);
        assert_eq!(plan.codecs, [Codec::H264]);
    }

    #[test]
    fn a_codec_needs_both_the_queue_and_the_extension() {
        let without_av1 = |name: &CStr| name != ash::khr::video_decode_av1::NAME;
        let plan = plan_video(V1_3, &many_families(), without_av1, true).unwrap();
        assert_eq!(plan.codecs, [Codec::H264, Codec::H265]);
        assert!(!plan.extensions.contains(&ash::khr::video_decode_av1::NAME));
        // The queue says H.265 only.
        let mut families = many_families();
        families[3].video = Op::DECODE_H265;
        let plan = plan_video(V1_3, &families, everything, true).unwrap();
        assert_eq!(plan.codecs, [Codec::H265]);
        // Nothing we play.
        families[3].video = Op::ENCODE_H264;
        let why = plan_video(V1_3, &families, everything, true).unwrap_err();
        assert!(why.contains("none of H.264"), "{why}");
    }

    #[test]
    fn two_decode_families_with_different_codecs_are_both_used() {
        let mut families = many_families();
        families[3].video = Op::DECODE_H264 | Op::DECODE_H265;
        families.push(decoding(Op::DECODE_AV1));
        let plan = plan_video(V1_3, &families, everything, true).unwrap();
        assert_eq!(plan.decode.iter().map(|f| f.index).collect::<Vec<_>>(), [3, 5]);
        assert_eq!(plan.codecs, Codec::ALL);
    }

    #[test]
    fn what_is_missing_is_said_in_words() {
        let old = (vk::API_VERSION_1_3, vk::API_VERSION_1_2);
        let why = plan_video(old, &many_families(), everything, true).unwrap_err();
        assert!(why.contains("older than Vulkan 1.3"), "{why}");
        let old_loader = (vk::API_VERSION_1_1, vk::API_VERSION_1_3);
        assert!(plan_video(old_loader, &many_families(), everything, true).is_err());

        let no_video = |name: &CStr| name != ash::khr::video_decode_queue::NAME;
        let why = plan_video(V1_3, &many_families(), no_video, true).unwrap_err();
        assert!(why.contains("VK_KHR_video_decode_queue is missing"), "{why}");

        let drawing_only = vec![family(Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, 1)];
        let why = plan_video(V1_3, &drawing_only, everything, true).unwrap_err();
        assert!(why.contains("no video decode queue"), "{why}");
    }

    /// Vulkan on this machine, whatever drives it (a software driver will
    /// do). Without any, the checks that need a device say so and pass.
    fn any_vulkan_card() -> Option<(wgpu::Instance, wgpu::Adapter)> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::VULKAN;
        // Without the renderer's own message handler, a validation layer
        // switched on from outside (VK_INSTANCE_LAYERS) prints what it finds.
        descriptor.flags = wgpu::InstanceFlags::empty();
        let instance = wgpu::Instance::new(descriptor);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
        let Some(adapter) = adapters.into_iter().next() else {
            eprintln!("no Vulkan here: opening the device is not checked");
            return None;
        };
        Some((instance, adapter))
    }

    fn toolkit_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
        match eframe::NativeOptions::default().wgpu_options.wgpu_setup {
            egui_wgpu::WgpuSetup::CreateNew(setup) => (setup.device_descriptor)(adapter),
            egui_wgpu::WgpuSetup::Existing(_) => panic!("the toolkit creates the device"),
        }
    }

    #[test]
    fn opening_for_video_decoding_works_or_says_why_not() {
        let Some((instance, adapter)) = any_vulkan_card() else {
            return;
        };
        match open_on(&instance, &adapter, &toolkit_descriptor(&adapter)) {
            // A card with video decoding: what was planned is what the handle says.
            Ok(own) => {
                let video = &own.video;
                assert!(!video.codecs().is_empty());
                assert_ne!(video.helper().index, video.window_family());
                assert!(video.decode().iter().all(|f| f.index != video.window_family()));
                let has = |name: &CStr| video.device_extensions().iter().any(|e| e.as_c_str() == name);
                assert!(has(ash::khr::video_queue::NAME) && has(ash::khr::video_decode_queue::NAME));
            }
            // A software driver, or a card without it: an ordinary reason, nothing opened.
            Err(why) => {
                eprintln!("no video decoding on {}: {why}", adapter.get_info().name);
                assert!(!why.is_empty());
            }
        }
    }

    #[test]
    fn a_device_opened_here_draws_like_one_the_toolkit_opened() {
        let Some((instance, adapter)) = any_vulkan_card() else {
            return;
        };
        let descriptor = toolkit_descriptor(&adapter);
        // SAFETY: only reads the adapter's own handles.
        let found = unsafe { Found::read(&adapter.as_hal::<wgpu::hal::api::Vulkan>().unwrap()) };
        if found.versions.0 < VULKAN_1_3 || found.versions.1 < VULKAN_1_3 {
            eprintln!("Vulkan here is older than 1.3: opening the device is not checked");
            return;
        }
        // The same way of opening (the callback, the extra feature, handing
        // the device to the renderer), without the video parts a software
        // driver doesn't have. The handle made here names the window's own
        // family as the helper, so it is only looked at, never submitted to.
        let nothing_added = Additions {
            extensions: Vec::new(),
            helper: QueueFamily {
                index: WINDOW_FAMILY,
                flags: Q::COMPUTE | Q::TRANSFER,
                codecs: Op::empty(),
            },
            decode: Vec::new(),
            codecs: Vec::new(),
            maintenance1: false,
        };
        let own = open_with(&instance, &adapter, &descriptor, &found, nothing_added).unwrap();
        assert_eq!(own.video.window_family(), WINDOW_FAMILY);
        assert!(own.video.synchronization2());
        assert!(!own.video.device_extensions().is_empty());
        // The renderer works on it: a texture is written, read back and compared.
        let (device, queue) = (&own.setup.device, &own.setup.queue);
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let pixels: Vec<u8> = (0..64).collect();
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(256),
            rows_per_image: Some(4),
        };
        queue.write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                bytes_per_row: Some(16),
                ..layout
            },
            size,
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout,
            },
            size,
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let read = buffer.get_mapped_range(..).unwrap();
        for row in 0..4 {
            assert_eq!(read[row * 256..row * 256 + 16], pixels[row * 16..row * 16 + 16]);
        }
    }

    /// A device opened by this module without the video parts, for a
    /// machine whose Vulkan has none (see the test above).
    fn plain_device() -> Option<OwnDevice> {
        let (instance, adapter) = any_vulkan_card()?;
        // SAFETY: only reads the adapter's own handles.
        let found = unsafe { Found::read(&adapter.as_hal::<wgpu::hal::api::Vulkan>().unwrap()) };
        if found.versions.0 < VULKAN_1_3 || found.versions.1 < VULKAN_1_3 {
            eprintln!("Vulkan here is older than 1.3: not checked");
            return None;
        }
        let nothing_added = Additions {
            extensions: Vec::new(),
            helper: QueueFamily {
                index: WINDOW_FAMILY,
                flags: Q::COMPUTE | Q::TRANSFER,
                codecs: Op::empty(),
            },
            decode: Vec::new(),
            codecs: Vec::new(),
            maintenance1: false,
        };
        let descriptor = toolkit_descriptor(&adapter);
        open_with(&instance, &adapter, &descriptor, &found, nothing_added).ok()
    }

    /// Draws `planes` with the viewer's shader into a target of their size
    /// and reads it back as RGBA. The conversion is set so that red is the
    /// luma, green is U and blue is V, to see each plane arrive.
    fn draw(own: &OwnDevice, textures: (wgpu::Texture, wgpu::Texture), rotation: u16) -> Vec<u8> {
        let (device, queue) = (&own.setup.device, &own.setup.queue);
        let planes = ui_kit::PlaneTextures {
            luma: textures.0,
            chroma: textures.1,
            to_rgb: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            rotation,
        };
        let (width, height) = planes.upright_size();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = ui_kit::yuv_renderer::YuvRenderer::new(device, format);
        assert!(renderer.show(device, queue, &planes));
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.draw(&mut pass);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped
            .chunks(row as usize)
            .flat_map(|r| r[..width as usize * 4].to_vec())
            .collect()
    }

    #[test]
    fn a_picture_copied_on_the_card_is_drawn_from_its_own_images_and_given_back_after() {
        use video_device::{Copier, Source, SourceImage, testing};
        let Some(own) = plain_device() else { return };
        let video = &own.video;
        // The decoder's picture, stood in for: 16×8 decoded, of which 12×6
        // is picture. Luma counts up; colour is the same everywhere.
        let luma: Vec<u8> = (0..16 * 8).map(|i| i as u8).collect();
        let chroma: Vec<u8> = [90u8, 170].repeat(8 * 4);
        let picture = testing::stand_in_picture(video, (16, 8), &luma, &chroma);
        let planes = Arc::new(PlaneImages::new(video, (12, 6)).unwrap());
        let (luma_image, chroma_image) = (planes.luma(), planes.chroma());
        let mut copier = Copier::new(video).unwrap();
        let source = Source::TwoPlanes(SourceImage {
            image: picture.0,
            layout: testing::STAND_IN_LAYOUT,
            access: vk::AccessFlags::TRANSFER_READ,
            semaphore: vk::Semaphore::null(),
            value: 0,
        });
        // SAFETY: the stand-in is this device's, 16×8, idle, in the layout
        // named, and outlives the wait; the pair is 12×6 and only used here.
        // This test is the only user of the device, so its one queue needs no lock.
        unsafe {
            copier
                .copy(&source, (12, 6), (0, 0), &planes, &|| (), &|| ())
                .unwrap();
        }
        copier.wait().unwrap();

        // Another device's images are refused.
        if let Some(other) = plain_device() {
            assert!(plane_textures(&other.setup.device, &planes).is_none());
        }
        let started = std::time::Instant::now();
        let textures = plane_textures(&own.setup.device, &planes).unwrap();
        // On the interface thread in the editor: it has to stay small.
        eprintln!("wrapping a picture's images took {:?}", started.elapsed());
        assert_eq!((textures.0.width(), textures.0.height()), (12, 6));
        assert_eq!((textures.1.width(), textures.1.height()), (6, 3));
        drop(planes); // the textures alone keep the images now

        let rgba = draw(&own, textures.clone(), 0);
        for (i, pixel) in rgba.chunks_exact(4).enumerate() {
            let (x, y) = (i % 12, i / 12);
            let want = [luma[y * 16 + x], 90, 170, 255];
            assert_eq!(pixel, want, "pixel {x},{y}: the visible part, not the padding");
        }
        // Turned a quarter clockwise: the stored left column ends on top.
        let turned = draw(&own, textures.clone(), 90);
        assert_eq!(turned.len(), 6 * 12 * 4);
        assert_eq!(turned[0], luma[5 * 16], "top left shows the stored bottom left");
        assert_eq!(turned[5 * 4], luma[0], "top right shows the stored top left");

        // While the renderer holds the textures, the images are not handed
        // out again; once it has let go and its work is done, they are.
        let meanwhile = PlaneImages::new(video, (12, 6)).unwrap();
        assert_ne!(meanwhile.luma(), luma_image);
        drop(textures);
        own.setup
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let again = PlaneImages::new(video, (12, 6)).unwrap();
        assert_eq!((again.luma(), again.chroma()), (luma_image, chroma_image));

        drop(copier);
        testing::destroy_stand_in(video, picture);
    }

    #[test]
    fn the_maintenance_extension_is_only_added_with_its_feature() {
        let plan = plan_video(V1_3, &many_families(), everything, false).unwrap();
        assert!(!plan.maintenance1);
        assert!(!plan.extensions.contains(&ash::khr::video_maintenance1::NAME));
        let without = |name: &CStr| name != ash::khr::video_maintenance1::NAME;
        assert!(
            !plan_video(V1_3, &many_families(), without, true)
                .unwrap()
                .maintenance1
        );
    }
}
