//! Approach C: FFmpeg decodes with Vulkan Video on a Vulkan device that wgpu
//! created, so frames never leave the GPU.
//!
//! The device is opened through wgpu's lower layer with the video queues and
//! extensions FFmpeg needs added, then handed to FFmpeg. Each decoded frame is
//! only waited for (its timeline semaphore), not copied: in the editor it would
//! be sampled straight from the shader.
//!
//! FFmpeg's Rust bindings don't include its Vulkan header, so the three
//! structures used are declared here to match FFmpeg 8.1 (libavutil 60).

use ash::vk;
use ffmpeg_sys_next as ff;
use std::ffi::{CStr, c_char, c_int, c_void};

const AV_NUM_DATA_POINTERS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy)]
struct AVVulkanDeviceQueueFamily {
    idx: c_int,
    num: c_int,
    flags: vk::QueueFlags,
    video_caps: vk::VideoCodecOperationFlagsKHR,
}

#[repr(C)]
struct AVVulkanDeviceContext {
    alloc: *const vk::AllocationCallbacks<'static>,
    get_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    inst: vk::Instance,
    phys_dev: vk::PhysicalDevice,
    act_dev: vk::Device,
    device_features: vk::PhysicalDeviceFeatures2<'static>,
    enabled_inst_extensions: *const *const c_char,
    nb_enabled_inst_extensions: c_int,
    enabled_dev_extensions: *const *const c_char,
    nb_enabled_dev_extensions: c_int,
    // Deprecated fixed queue fields (FF_API_VULKAN_FIXED_QUEUES, still present in 8.1).
    legacy_queues: [c_int; 10],
    // Deprecated queue locking (FF_API_VULKAN_SYNC_QUEUES, still present in 8.1).
    lock_queue: Option<unsafe extern "C" fn(*mut ff::AVHWDeviceContext, u32, u32)>,
    unlock_queue: Option<unsafe extern "C" fn(*mut ff::AVHWDeviceContext, u32, u32)>,
    qf: [AVVulkanDeviceQueueFamily; 64],
    nb_qf: c_int,
}

#[repr(C)]
struct AVVulkanFramesContext {
    tiling: vk::ImageTiling,
    usage: vk::ImageUsageFlags,
    create_pnext: *mut c_void,
    alloc_pnext: [*mut c_void; AV_NUM_DATA_POINTERS],
    flags: c_int,
    img_flags: vk::ImageCreateFlags,
    format: [vk::Format; AV_NUM_DATA_POINTERS],
    nb_layers: c_int,
    lock_frame: Option<unsafe extern "C" fn(*mut ff::AVHWFramesContext, *mut AVVkFrame)>,
    unlock_frame: Option<unsafe extern "C" fn(*mut ff::AVHWFramesContext, *mut AVVkFrame)>,
}

#[repr(C)]
struct AVVkFrame {
    img: [vk::Image; AV_NUM_DATA_POINTERS],
    tiling: vk::ImageTiling,
    mem: [vk::DeviceMemory; AV_NUM_DATA_POINTERS],
    size: [usize; AV_NUM_DATA_POINTERS],
    flags: vk::MemoryPropertyFlags,
    access: [vk::AccessFlags; AV_NUM_DATA_POINTERS],
    layout: [vk::ImageLayout; AV_NUM_DATA_POINTERS],
    sem: [vk::Semaphore; AV_NUM_DATA_POINTERS],
    sem_value: [u64; AV_NUM_DATA_POINTERS],
    internal: *mut c_void,
    offset: [isize; AV_NUM_DATA_POINTERS],
    queue_family: [u32; AV_NUM_DATA_POINTERS],
}

/// The wgpu device shared with FFmpeg. Kept alive as long as the decoder.
pub struct Shared {
    _device: wgpu::Device,
    _queue: wgpu::Queue,
    _adapter: wgpu::Adapter,
    _instance: wgpu::Instance,
    raw: ash::Device,
    pub description: String,
}

const VIDEO_EXTENSIONS: [&CStr; 6] = [
    ash::khr::video_queue::NAME,
    ash::khr::video_decode_queue::NAME,
    ash::khr::video_decode_h264::NAME,
    ash::khr::video_decode_h265::NAME,
    c"VK_KHR_video_decode_av1",
    ash::khr::video_maintenance1::NAME,
];

static PRIORITY: [f32; 1] = [1.0];

impl Shared {
    /// Opens a wgpu Vulkan device with video decoding enabled and returns it
    /// with an FFmpeg device context (an `AVBufferRef`) that uses it.
    pub fn open() -> Result<(Shared, *mut ff::AVBufferRef), String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no Vulkan adapter: {e}"))?;
        let features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_NV12;

        unsafe {
            let hal_adapter = adapter
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("not a Vulkan adapter")?;
            let shared_instance = hal_adapter.shared_instance();
            let vk_instance = shared_instance.raw_instance().clone();
            let entry = shared_instance.entry().clone();
            let phys = hal_adapter.raw_physical_device();

            if shared_instance.instance_api_version() < vk::API_VERSION_1_3 {
                return Err("Vulkan 1.3 is needed for video decoding".into());
            }

            // Queue families: the first with video decoding, and what it can decode.
            let count = vk_instance.get_physical_device_queue_family_properties2_len(phys);
            let mut video = vec![vk::QueueFamilyVideoPropertiesKHR::default(); count];
            let flags: Vec<vk::QueueFlags> = {
                let mut props: Vec<vk::QueueFamilyProperties2> = video
                    .iter_mut()
                    .map(|v| vk::QueueFamilyProperties2::default().push_next(v))
                    .collect();
                vk_instance.get_physical_device_queue_family_properties2(phys, &mut props);
                props
                    .iter()
                    .map(|p| p.queue_family_properties.queue_flags)
                    .collect()
            };
            let decode_family = flags
                .iter()
                .position(|f| f.contains(vk::QueueFlags::VIDEO_DECODE_KHR))
                .ok_or("this GPU's Vulkan driver offers no video decode queue")?
                as u32;
            let decode_caps = video[decode_family as usize].video_codec_operations;

            // Only the video extensions the driver has.
            let available: Vec<String> = vk_instance
                .enumerate_device_extension_properties(phys)
                .map_err(|e| format!("list extensions: {e}"))?
                .iter()
                .filter_map(|e| {
                    e.extension_name_as_c_str()
                        .ok()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .collect();
            let extra: Vec<&'static CStr> = VIDEO_EXTENSIONS
                .iter()
                .copied()
                .filter(|n| available.iter().any(|a| a.as_str() == n.to_string_lossy()))
                .collect();
            if !extra.contains(&ash::khr::video_decode_queue::NAME) {
                return Err("the driver lacks VK_KHR_video_decode_queue".into());
            }
            let maintenance1 = extra.contains(&ash::khr::video_maintenance1::NAME);

            // Features FFmpeg's decoder needs that wgpu doesn't turn on by itself.
            // Leaked: they must outlive device creation inside wgpu (spike).
            let sync2: &'static mut _ = Box::leak(Box::new(
                vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true),
            ));
            let vm1: &'static mut _ = Box::leak(Box::new(
                vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default().video_maintenance1(true),
            ));
            let added: std::cell::RefCell<Vec<&'static CStr>> = Default::default();
            let extra_ref = &extra;
            let added_ref = &added;
            let callback: Box<wgpu::hal::vulkan::CreateDeviceCallback> = Box::new(move |args| {
                for e in extra_ref {
                    if !args.extensions.contains(e) {
                        args.extensions.push(e);
                    }
                }
                if !args
                    .queue_create_infos
                    .iter()
                    .any(|q| q.queue_family_index == decode_family)
                {
                    args.queue_create_infos.push(
                        vk::DeviceQueueCreateInfo::default()
                            .queue_family_index(decode_family)
                            .queue_priorities(&PRIORITY),
                    );
                }
                let mut chain = std::mem::take(args.create_info);
                chain = chain.push_next(sync2);
                if maintenance1 {
                    chain = chain.push_next(vm1);
                }
                *args.create_info = chain;
                *added_ref.borrow_mut() = args.extensions.clone();
            });
            let open = hal_adapter
                .open_with_callback(
                    features,
                    &wgpu::Limits::default(),
                    &wgpu::MemoryHints::Performance,
                    Some(callback),
                )
                .map_err(|e| format!("open the Vulkan device with video queues: {e}"))?;
            let enabled = added.into_inner();
            let raw = open.device.raw_device().clone();
            let graphics_family = open.device.queue_family_index();
            drop(hal_adapter);
            let (device, queue) = adapter
                .create_device_from_hal(
                    open,
                    &wgpu::DeviceDescriptor {
                        required_features: features,
                        ..Default::default()
                    },
                )
                .map_err(|e| format!("wgpu device from the Vulkan device: {e}"))?;

            // Hand the same device to FFmpeg.
            let buf = ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN);
            if buf.is_null() {
                return Err("FFmpeg can't allocate a Vulkan device context".into());
            }
            let hw = (*buf).data as *mut ff::AVHWDeviceContext;
            let ctx = (*hw).hwctx as *mut AVVulkanDeviceContext;
            // FFmpeg reads these for as long as the device lives: leaked on purpose (spike).
            let names: &'static [*const c_char] = Box::leak(
                enabled
                    .iter()
                    .map(|n| n.as_ptr())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let v11 = Box::leak(Box::new(
                vk::PhysicalDeviceVulkan11Features::default().sampler_ycbcr_conversion(true),
            ));
            let v12 = Box::leak(Box::new(
                vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true),
            ));
            let v13 = Box::leak(Box::new(
                vk::PhysicalDeviceVulkan13Features::default().synchronization2(true),
            ));
            v12.p_next = v13 as *mut _ as *mut c_void;
            v11.p_next = v12 as *mut _ as *mut c_void;
            let mut features2 = vk::PhysicalDeviceFeatures2::default();
            features2.p_next = v11 as *mut _ as *mut c_void;

            (*ctx).get_proc_addr = Some(entry.static_fn().get_instance_proc_addr);
            (*ctx).inst = vk_instance.handle();
            (*ctx).phys_dev = phys;
            (*ctx).act_dev = raw.handle();
            (*ctx).device_features = features2;
            (*ctx).enabled_dev_extensions = names.as_ptr();
            (*ctx).nb_enabled_dev_extensions = names.len() as c_int;
            // wgpu's own queue is listed so frames are shared with it; nothing
            // else submits in this test, so FFmpeg may use it for its transfers.
            (*ctx).qf[0] = AVVulkanDeviceQueueFamily {
                idx: graphics_family as c_int,
                num: 1,
                flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
                video_caps: vk::VideoCodecOperationFlagsKHR::empty(),
            };
            (*ctx).qf[1] = AVVulkanDeviceQueueFamily {
                idx: decode_family as c_int,
                num: 1,
                flags: vk::QueueFlags::VIDEO_DECODE_KHR,
                video_caps: decode_caps,
            };
            (*ctx).nb_qf = 2;
            let r = ff::av_hwdevice_ctx_init(buf);
            if r < 0 {
                let mut b = buf;
                ff::av_buffer_unref(&mut b);
                return Err(crate::decoder::describe(
                    r,
                    "FFmpeg rejected the shared Vulkan device",
                ));
            }

            let info = adapter.get_info();
            let description = format!(
                "{} driver {}; decode family {decode_family}; codecs {:?}",
                info.name, info.driver_info, decode_caps
            );
            Ok((
                Shared {
                    _device: device,
                    _queue: queue,
                    _adapter: adapter,
                    _instance: instance,
                    raw,
                    description,
                },
                buf,
            ))
        }
    }

    /// Waits until the GPU has finished decoding this frame. Nothing is copied.
    ///
    /// # Safety
    /// `frame` must be a decoded frame in the `AV_PIX_FMT_VULKAN` format from this device.
    pub unsafe fn wait_for(&self, frame: *const ff::AVFrame) -> Result<(), String> {
        unsafe {
            let vkf = (*frame).data[0] as *mut AVVkFrame;
            let fc = (*(*frame).hw_frames_ctx).data as *mut ff::AVHWFramesContext;
            let vfc = (*fc).hwctx as *mut AVVulkanFramesContext;
            if let Some(lock) = (*vfc).lock_frame {
                lock(fc, vkf);
            }
            let (sem, value) = ((*vkf).sem[0], (*vkf).sem_value[0]);
            if let Some(unlock) = (*vfc).unlock_frame {
                unlock(fc, vkf);
            }
            if sem == vk::Semaphore::null() {
                return Ok(());
            }
            let sems = [sem];
            let values = [value];
            self.raw
                .wait_semaphores(
                    &vk::SemaphoreWaitInfo::default().semaphores(&sems).values(&values),
                    u64::MAX,
                )
                .map_err(|e| format!("wait for decoded frame: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Sizes and offsets printed by a C program compiled against FFmpeg 8.1's
    /// hwcontext_vulkan.h on x86-64.
    #[test]
    fn layouts_match_ffmpeg_8_1() {
        assert_eq!(size_of::<AVVulkanDeviceContext>(), 1400);
        assert_eq!(offset_of!(AVVulkanDeviceContext, device_features), 40);
        assert_eq!(offset_of!(AVVulkanDeviceContext, enabled_dev_extensions), 296);
        assert_eq!(offset_of!(AVVulkanDeviceContext, lock_queue), 352);
        assert_eq!(offset_of!(AVVulkanDeviceContext, qf), 368);
        assert_eq!(offset_of!(AVVulkanDeviceContext, nb_qf), 1392);
        assert_eq!(size_of::<AVVulkanFramesContext>(), 144);
        assert_eq!(offset_of!(AVVulkanFramesContext, flags), 80);
        assert_eq!(offset_of!(AVVulkanFramesContext, img_flags), 84);
        assert_eq!(offset_of!(AVVulkanFramesContext, lock_frame), 128);
        assert_eq!(offset_of!(AVVulkanFramesContext, unlock_frame), 136);
        assert_eq!(size_of::<AVVkFrame>(), 504);
        assert_eq!(offset_of!(AVVkFrame, sem), 272);
        assert_eq!(offset_of!(AVVkFrame, sem_value), 336);
        assert_eq!(offset_of!(AVVkFrame, internal), 400);
        assert_eq!(offset_of!(AVVkFrame, queue_family), 472);
    }
}
