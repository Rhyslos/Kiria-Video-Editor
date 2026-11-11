//! Decoding on the window's own graphics device (approach C in
//! `docs/research/decoding.md`): FFmpeg's Vulkan decoder works on the device
//! that draws the window, so a decoded picture never leaves the graphics card.
//!
//! [`WindowDevice`] is FFmpeg's view of that device: an FFmpeg device
//! context built around the handles in a [`VideoDevice`], not one FFmpeg
//! creates. Every decoder that decodes this way shares the one context, and
//! with it FFmpeg's locks around each queue.
//!
//! A picture that is to be shown is copied, on the card, out of the
//! decoder's image into a pair of plain images ([`PlaneImages`]) that the
//! viewer draws from ([`GpuFrame`]). The decoder's own image is given
//! straight back: it is one of a fixed number, the decoder keeps using it
//! as a reference for the pictures that follow, and it may be larger than
//! the picture (padded to the codec's block size). The copy takes only the
//! picture, and follows the rules FFmpeg sets for outside users of a frame,
//! quoted where they are followed below.
//!
//! FFmpeg's Rust bindings don't include its Vulkan header, so the four
//! structures used are declared here to match FFmpeg 8.1 (libavutil 60),
//! with a test of their layout.

use crate::ffi::error_text;
use crate::yuv::Colour;
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::ffi::{c_char, c_int, c_void};
use std::sync::Arc;
use video_device::ash::vk;
use video_device::{
    ACCESS_AFTER_COPY, Codec, Copier, LAYOUT_AFTER_COPY, PlaneImages, Source, SourceImage, VideoDevice,
};

const AV_NUM_DATA_POINTERS: usize = 8;

/// `AVVulkanDeviceQueueFamily`.
#[repr(C)]
#[derive(Clone, Copy)]
struct AVVulkanDeviceQueueFamily {
    idx: c_int,
    num: c_int,
    flags: vk::QueueFlags,
    video_caps: vk::VideoCodecOperationFlagsKHR,
}

/// `AVVulkanDeviceContext`.
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
    /// The fixed queue fields from before `qf` (still in 8.1, unused here).
    legacy_queues: [c_int; 10],
    /// Set by FFmpeg when the context is initialised: its own lock around a queue.
    lock_queue: Option<unsafe extern "C" fn(*mut ff::AVHWDeviceContext, u32, u32)>,
    unlock_queue: Option<unsafe extern "C" fn(*mut ff::AVHWDeviceContext, u32, u32)>,
    qf: [AVVulkanDeviceQueueFamily; 64],
    nb_qf: c_int,
}

/// `AVVulkanFramesContext`.
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

/// `AVVkFrame`: what a frame in the `AV_PIX_FMT_VULKAN` format points to.
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

/// A decoded picture on the window's graphics device: two images the
/// viewer draws from, and what its shader needs to convert them.
#[derive(Debug, Clone)]
pub struct GpuFrame {
    /// Luma, and interleaved colour at half size: the picture only, without
    /// the decoder's padding. Ready for sampling.
    pub planes: Arc<PlaneImages>,
    pub colour: Colour,
    /// The clockwise turn (0, 90, 180 or 270) that shows it upright.
    pub rotation: u16,
    /// When it starts, in seconds from the start of the file.
    pub time: f64,
}

impl GpuFrame {
    /// The size on screen, after turning upright.
    pub fn upright_size(&self) -> (u32, u32) {
        let (w, h) = self.planes.size();
        if self.rotation % 180 == 90 { (h, w) } else { (w, h) }
    }
}

/// An error on this path. The preview falls back to the usual decoding for
/// the file and only logs it, so the words are for the log.
pub(crate) fn device_error(why: impl Into<String>) -> AppError {
    AppError::new("The video can't be decoded on the window's graphics device")
        .why(why)
        .fix("It is decoded the usual way instead. If this keeps happening, turn off \"Decode video on the window's graphics device\" in Settings, under Graphics.")
}

/// The window's graphics device as FFmpeg uses it. One for the whole
/// program: every decoder on the device shares it.
pub struct WindowDevice {
    device: Arc<VideoDevice>,
    /// FFmpeg's device context around our device (an `AVBufferRef`).
    context: *mut ff::AVBufferRef,
    /// What the context points into, for as long as it lives.
    _extension_names: (Vec<*const c_char>, Vec<*const c_char>),
    _features: Box<Features>,
}

/// The device features as FFmpeg's context lists them.
struct Features {
    vulkan_1_2: vk::PhysicalDeviceVulkan12Features<'static>,
    vulkan_1_3: vk::PhysicalDeviceVulkan13Features<'static>,
}

// The context is FFmpeg's reference-counted, internally locked device
// context, and the pointers kept beside it are only read by FFmpeg.
unsafe impl Send for WindowDevice {}
unsafe impl Sync for WindowDevice {}

impl std::fmt::Debug for WindowDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WindowDevice({})", self.device.codec_names())
    }
}

impl WindowDevice {
    /// Wraps the window's device for FFmpeg. Fails when this FFmpeg build
    /// has no Vulkan support or doesn't accept the device.
    pub fn new(device: Arc<VideoDevice>) -> diag::Result<Arc<WindowDevice>> {
        let names =
            |list: &[std::ffi::CString]| -> Vec<*const c_char> { list.iter().map(|n| n.as_ptr()).collect() };
        let extension_names = (
            names(device.instance_extensions()),
            names(device.device_extensions()),
        );
        let mut features = Box::new(Features {
            vulkan_1_2: vk::PhysicalDeviceVulkan12Features::default()
                .timeline_semaphore(device.timeline_semaphore()),
            vulkan_1_3: vk::PhysicalDeviceVulkan13Features::default()
                .synchronization2(device.synchronization2()),
        });
        features.vulkan_1_2.p_next = (&raw mut features.vulkan_1_3).cast();
        // SAFETY: the context is allocated by FFmpeg and filled in before it
        // is initialised, as `AVVulkanDeviceContext`'s documentation asks
        // ("All of these can be set before init"); every pointer written
        // into it (the extension names, the feature structures, the
        // device's handles) is kept alive by `self` for as long as the
        // context. FFmpeg doesn't destroy a device it didn't create: its
        // destructor for that is only installed by `av_hwdevice_ctx_create`.
        unsafe {
            let buffer = ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN);
            if buffer.is_null() {
                return Err(device_error(
                    "This FFmpeg build has no Vulkan support (or memory ran out).",
                ));
            }
            let hw = (*buffer).data as *mut ff::AVHWDeviceContext;
            let context = (*hw).hwctx as *mut AVVulkanDeviceContext;
            (*context).get_proc_addr = Some(device.get_instance_proc_addr());
            (*context).inst = device.instance().handle();
            (*context).phys_dev = device.physical_device();
            (*context).act_dev = device.device().handle();
            (*context).device_features = vk::PhysicalDeviceFeatures2::default();
            (*context).device_features.p_next = (&raw mut features.vulkan_1_2).cast();
            // "If no extensions are enabled, set these fields to NULL, and 0."
            let (instance_names, device_names) = &extension_names;
            if !instance_names.is_empty() {
                (*context).enabled_inst_extensions = instance_names.as_ptr();
                (*context).nb_enabled_inst_extensions = instance_names.len() as c_int;
            }
            if !device_names.is_empty() {
                (*context).enabled_dev_extensions = device_names.as_ptr();
                (*context).nb_enabled_dev_extensions = device_names.len() as c_int;
            }
            // "Queue families used. Must be preferentially ordered." FFmpeg
            // takes the first family whose flags fit each job: the helper
            // queue for its own compute and transfer work, a decode queue
            // for decoding. The window's own queue is not in the list, so
            // FFmpeg never submits to it.
            let families = std::iter::once(device.helper()).chain(device.decode().iter().copied());
            for (slot, family) in (*context).qf.iter_mut().zip(families) {
                *slot = AVVulkanDeviceQueueFamily {
                    idx: family.index as c_int,
                    num: 1,
                    flags: family.flags,
                    video_caps: family.codecs,
                };
                (*context).nb_qf += 1;
            }
            let result = ff::av_hwdevice_ctx_init(buffer);
            if result < 0 {
                let mut buffer = buffer;
                ff::av_buffer_unref(&mut buffer);
                return Err(device_error(format!(
                    "FFmpeg didn't accept the window's device ({}).",
                    error_text(result)
                )));
            }
            Ok(Arc::new(WindowDevice {
                device,
                context: buffer,
                _extension_names: extension_names,
                _features: features,
            }))
        }
    }

    pub fn device(&self) -> &Arc<VideoDevice> {
        &self.device
    }

    /// Whether the device decodes this kind of video.
    pub(crate) fn decodes(&self, codec: ff::AVCodecID) -> bool {
        let codec = match codec {
            ff::AVCodecID::AV_CODEC_ID_H264 => Codec::H264,
            ff::AVCodecID::AV_CODEC_ID_HEVC => Codec::H265,
            ff::AVCodecID::AV_CODEC_ID_AV1 => Codec::Av1,
            _ => return false,
        };
        self.device.decodes(codec)
    }

    /// The graphics card's own memory, in bytes (0 when it has none of its own).
    pub fn card_memory(&self) -> u64 {
        self.device.card_memory()
    }

    /// A new reference to FFmpeg's device context, for a codec context to
    /// own. Null when memory ran out.
    pub(crate) fn new_reference(&self) -> *mut ff::AVBufferRef {
        // SAFETY: the context is alive for as long as `self`.
        unsafe { ff::av_buffer_ref(self.context) }
    }

    /// Copies the picture in `frame` out of the decoder's image into a pair
    /// of images for the viewer, and waits until the graphics card has done it.
    ///
    /// # Safety
    /// `frame` must be a decoded frame from a decoder working on this
    /// device, alive until this returns, and `copier` must be on this device too.
    pub(crate) unsafe fn copy_out(
        &self,
        frame: *const ff::AVFrame,
        copier: &mut Copier,
        rotation: u16,
        time: f64,
    ) -> diag::Result<GpuFrame> {
        // SAFETY: `frame` is a live decoded frame (this function's
        // contract). A frame in the Vulkan format carries its frames context
        // and points to an `AVVkFrame`; both are checked before use and
        // stay alive with the frame.
        unsafe {
            if (*frame).format != ff::AVPixelFormat::AV_PIX_FMT_VULKAN as i32
                || (*frame).hw_frames_ctx.is_null()
            {
                return Err(device_error(
                    "FFmpeg decoded this file on the CPU instead: the device's decoder doesn't take it.",
                ));
            }
            let frames = (*(*frame).hw_frames_ctx).data as *mut ff::AVHWFramesContext;
            // The shader takes 8-bit pictures with colour at half size; in
            // Vulkan frames those are always luma plus interleaved colour.
            if (*frames).sw_format != ff::AVPixelFormat::AV_PIX_FMT_NV12 {
                return Err(device_error(
                    "The picture isn't 8-bit with colour at half size (10-bit and the like go the usual way).",
                ));
            }
            let vulkan_frames = (*frames).hwctx as *mut AVVulkanFramesContext;
            let picture = (*frame).data[0] as *mut AVVkFrame;
            let (Some(lock_frame), Some(unlock_frame)) =
                ((*vulkan_frames).lock_frame, (*vulkan_frames).unlock_frame)
            else {
                return Err(device_error("FFmpeg's frames have no lock."));
            };
            if picture.is_null() {
                return Err(device_error("The decoded frame has no image."));
            }
            // The picture inside the decoded image: decoders pad images to
            // their block size, and a cropped stream starts further in.
            let region = Region::of(
                ((*frame).width, (*frame).height),
                ((*frame).crop_left, (*frame).crop_top),
                ((*frames).width, (*frames).height),
            )
            .ok_or_else(|| device_error("The picture doesn't fit in the decoded image."))?;
            let colour = Colour::of(
                (*frame).colorspace,
                (*frame).color_range,
                false,
                region.visible.0,
                region.visible.1,
            )
            .ok_or_else(|| {
                device_error("The file's colour matrix isn't one the viewer's shader converts.")
            })?;
            let target = PlaneImages::new(&self.device, region.visible).map_err(device_error)?;

            // FFmpeg's own lock around the helper queue, for the moment of
            // submitting: FFmpeg prepares its images on that queue too.
            let hw = (*self.context).data as *mut ff::AVHWDeviceContext;
            let context = (*hw).hwctx as *mut AVVulkanDeviceContext;
            let (Some(lock_queue), Some(unlock_queue)) = ((*context).lock_queue, (*context).unlock_queue)
            else {
                return Err(device_error("FFmpeg's device has no queue lock."));
            };
            let helper = self.device.helper().index;
            let lock = || lock_queue(hw, helper, 0);
            let unlock = || unlock_queue(hw, helper, 0);

            // `AVVulkanFramesContext::lock_frame`: "Locks a frame, preventing
            // other threads from changing frame properties. Users SHOULD
            // only ever lock just before command submission in order to get
            // accurate frame properties, and unlock immediately after
            // command submission without waiting for it to finish."
            lock_frame(frames, picture);
            let image = |i: usize| SourceImage {
                image: (*picture).img[i],
                layout: (*picture).layout[i],
                access: (*picture).access[i],
                semaphore: (*picture).sem[i],
                value: (*picture).sem_value[i],
            };
            // `AVVkFrame::img`: "May be one for multiplane formats, or multiple."
            let two_images = (*picture).img[1] != vk::Image::null();
            let source = if two_images {
                Source::TwoImages(image(0), image(1))
            } else {
                Source::TwoPlanes(image(0))
            };
            // `AVVkFrame::queue_family`: "Must be VK_QUEUE_FAMILY_IGNORED if
            // the image was allocated with the CONCURRENT concurrency
            // option." That is how FFmpeg allocates when it has more than
            // one queue family, as here, and what lets the helper queue
            // read the image. An image owned by another family is left alone.
            let owners = (*picture).queue_family; // a copy: read under the lock
            let readable = owners[..if two_images { 2 } else { 1 }]
                .iter()
                .all(|family| *family == vk::QUEUE_FAMILY_IGNORED || *family == helper);
            let submitted = if readable {
                copier.copy(&source, region.visible, region.origin, &target, &lock, &unlock)
            } else {
                Err("The decoded image belongs to a queue the copy can't use.".to_owned())
            };
            if submitted.is_ok() {
                for i in 0..if two_images { 2 } else { 1 } {
                    // `AVVkFrame::sem_value`: "Clients must wait on this
                    // value when submitting a command queue, and increment
                    // it when signalling." The copy waited on it and
                    // signals the next value, so FFmpeg's next use of the
                    // image waits for the copy.
                    (*picture).sem_value[i] += 1;
                    // `AVVkFrame::access`, `layout`: "Updated after every
                    // barrier. One per VkImage."
                    (*picture).layout[i] = LAYOUT_AFTER_COPY;
                    (*picture).access[i] = ACCESS_AFTER_COPY;
                }
            }
            unlock_frame(frames, picture);
            submitted.map_err(device_error)?;
            // Now wait: when this returns the picture is in the pair, and
            // the decoder's image is no longer needed for it.
            copier.wait().map_err(device_error)?;
            Ok(GpuFrame {
                planes: Arc::new(target),
                colour,
                rotation,
                time,
            })
        }
    }
}

impl Drop for WindowDevice {
    fn drop(&mut self) {
        // SAFETY: our reference, released once. Decoders hold their own.
        unsafe { ff::av_buffer_unref(&mut self.context) }
    }
}

/// Where the picture is inside a decoded image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Region {
    /// The picture's size.
    visible: (u32, u32),
    /// Its top left corner in the image.
    origin: (u32, u32),
}

impl Region {
    /// From a frame's size and crop and its image's size, as FFmpeg gives
    /// them. `None` when they contradict each other.
    fn of(size: (c_int, c_int), crop: (usize, usize), image: (c_int, c_int)) -> Option<Region> {
        let visible = (u32::try_from(size.0).ok()?, u32::try_from(size.1).ok()?);
        let origin = (u32::try_from(crop.0).ok()?, u32::try_from(crop.1).ok()?);
        let image = (u32::try_from(image.0).ok()?, u32::try_from(image.1).ok()?);
        let fits = visible.0 > 0
            && visible.1 > 0
            && origin.0.checked_add(visible.0)? <= image.0
            && origin.1.checked_add(visible.1)? <= image.1;
        fits.then_some(Region { visible, origin })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Sizes and offsets printed by a C program compiled against FFmpeg
    /// 8.1's `hwcontext_vulkan.h` on x86-64 (the same on Windows and Linux:
    /// no field's type differs between them).
    #[test]
    fn layouts_match_ffmpeg_8_1() {
        assert_eq!(size_of::<AVVulkanDeviceQueueFamily>(), 16);
        assert_eq!(size_of::<AVVulkanDeviceContext>(), 1400);
        assert_eq!(offset_of!(AVVulkanDeviceContext, get_proc_addr), 8);
        assert_eq!(offset_of!(AVVulkanDeviceContext, inst), 16);
        assert_eq!(offset_of!(AVVulkanDeviceContext, phys_dev), 24);
        assert_eq!(offset_of!(AVVulkanDeviceContext, act_dev), 32);
        assert_eq!(offset_of!(AVVulkanDeviceContext, device_features), 40);
        assert_eq!(offset_of!(AVVulkanDeviceContext, enabled_inst_extensions), 280);
        assert_eq!(offset_of!(AVVulkanDeviceContext, nb_enabled_inst_extensions), 288);
        assert_eq!(offset_of!(AVVulkanDeviceContext, enabled_dev_extensions), 296);
        assert_eq!(offset_of!(AVVulkanDeviceContext, nb_enabled_dev_extensions), 304);
        assert_eq!(offset_of!(AVVulkanDeviceContext, lock_queue), 352);
        assert_eq!(offset_of!(AVVulkanDeviceContext, unlock_queue), 360);
        assert_eq!(offset_of!(AVVulkanDeviceContext, qf), 368);
        assert_eq!(offset_of!(AVVulkanDeviceContext, nb_qf), 1392);
        assert_eq!(size_of::<AVVulkanFramesContext>(), 144);
        assert_eq!(offset_of!(AVVulkanFramesContext, usage), 4);
        assert_eq!(offset_of!(AVVulkanFramesContext, flags), 80);
        assert_eq!(offset_of!(AVVulkanFramesContext, img_flags), 84);
        assert_eq!(offset_of!(AVVulkanFramesContext, format), 88);
        assert_eq!(offset_of!(AVVulkanFramesContext, nb_layers), 120);
        assert_eq!(offset_of!(AVVulkanFramesContext, lock_frame), 128);
        assert_eq!(offset_of!(AVVulkanFramesContext, unlock_frame), 136);
        assert_eq!(size_of::<AVVkFrame>(), 504);
        assert_eq!(offset_of!(AVVkFrame, img), 0);
        assert_eq!(offset_of!(AVVkFrame, tiling), 64);
        assert_eq!(offset_of!(AVVkFrame, mem), 72);
        assert_eq!(offset_of!(AVVkFrame, size), 136);
        assert_eq!(offset_of!(AVVkFrame, flags), 200);
        assert_eq!(offset_of!(AVVkFrame, access), 204);
        assert_eq!(offset_of!(AVVkFrame, layout), 236);
        assert_eq!(offset_of!(AVVkFrame, sem), 272);
        assert_eq!(offset_of!(AVVkFrame, sem_value), 336);
        assert_eq!(offset_of!(AVVkFrame, internal), 400);
        assert_eq!(offset_of!(AVVkFrame, offset), 408);
        assert_eq!(offset_of!(AVVkFrame, queue_family), 472);
    }

    #[test]
    fn the_picture_is_found_inside_the_padded_image() {
        // 1080 rows decoded into 1088.
        let hd = Region::of((1920, 1080), (0, 0), (1920, 1088)).unwrap();
        assert_eq!((hd.visible, hd.origin), ((1920, 1080), (0, 0)));
        // Cropped at the left and top.
        let cropped = Region::of((1280, 720), (8, 4), (1296, 736)).unwrap();
        assert_eq!(cropped.origin, (8, 4));
        // Numbers that can't be: larger than the image, empty, negative.
        assert_eq!(Region::of((1920, 1080), (0, 0), (1920, 1072)), None);
        assert_eq!(Region::of((1920, 1080), (8, 0), (1920, 1088)), None);
        assert_eq!(Region::of((0, 1080), (0, 0), (1920, 1088)), None);
        assert_eq!(Region::of((-1, 1080), (0, 0), (1920, 1088)), None);
        assert_eq!(Region::of((1920, 1080), (usize::MAX, 0), (1920, 1088)), None);
    }

    /// An 8-bit picture with colour at half size, in memory: luma counts
    /// along the rows, U and V differ per colour sample.
    fn pattern(width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        let luma = (0..width * height)
            .map(|i| ((i % width) * 3 + (i / width) * 5) as u8)
            .collect();
        let chroma = (0..(width / 2) * (height / 2))
            .flat_map(|i| [(60 + i % (width / 2)) as u8, (140 + i / (width / 2)) as u8])
            .collect();
        (luma, chroma)
    }

    /// The rows `0..rows` and columns `0..columns` (in pixels of
    /// `bytes_per_pixel`) of a plane `width` pixels wide.
    fn top_left(plane: &[u8], width: usize, columns: usize, rows: usize, bytes_per_pixel: usize) -> Vec<u8> {
        (0..rows)
            .flat_map(|y| {
                plane[y * width * bytes_per_pixel..][..columns * bytes_per_pixel]
                    .iter()
                    .copied()
            })
            .collect()
    }

    /// The whole path except the decoder itself, on whatever Vulkan this
    /// machine has: FFmpeg takes the device, makes a frame in its own pool
    /// (the kind of image and semaphore its decoder would hand over), the
    /// picture is copied out by the rules, and FFmpeg then reads the frame
    /// again, which only works if the frame was left as FFmpeg expects.
    #[test]
    fn a_frame_from_ffmpegs_own_pool_is_copied_out_by_its_rules() {
        let Some(device) = video_device::testing::device() else {
            return;
        };
        let window = match WindowDevice::new(Arc::clone(&device)) {
            Ok(window) => window,
            Err(err) => panic!("FFmpeg takes the device: {}", err.report()),
        };
        // Both shapes FFmpeg's frames come in: one image with two planes,
        // and (asked for by a flag here) two separate images.
        for two_images in [false, true] {
            copied_out_by_the_rules(&device, &window, two_images);
        }
    }

    /// `AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE`.
    const DISABLE_MULTIPLANE: c_int = 1 << 2;

    fn copied_out_by_the_rules(device: &Arc<VideoDevice>, window: &WindowDevice, two_images: bool) {
        let (coded, visible) = ((64usize, 48usize), (60u32, 44u32));
        let (luma, chroma) = pattern(coded.0, coded.1);
        // SAFETY: plain FFmpeg calls; every allocation is released below.
        unsafe {
            let hw = (*window.context).data as *mut ff::AVHWDeviceContext;
            let context = (*hw).hwctx as *mut AVVulkanDeviceContext;
            assert!(
                (*context).lock_queue.is_some() && (*context).unlock_queue.is_some(),
                "FFmpeg filled in its queue locks where we expect them"
            );
            assert_eq!(
                (*context).nb_qf,
                1,
                "the helper queue; a software driver decodes nothing"
            );

            // A pool of frames like a decoder's: Vulkan frames holding NV12.
            let mut frames_ref = ff::av_hwframe_ctx_alloc(window.context);
            assert!(!frames_ref.is_null());
            let frames = (*frames_ref).data as *mut ff::AVHWFramesContext;
            (*frames).format = ff::AVPixelFormat::AV_PIX_FMT_VULKAN;
            (*frames).sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            (*frames).width = coded.0 as c_int;
            (*frames).height = coded.1 as c_int;
            if two_images {
                (*((*frames).hwctx as *mut AVVulkanFramesContext)).flags = DISABLE_MULTIPLANE;
            }
            let result = ff::av_hwframe_ctx_init(frames_ref);
            assert!(result >= 0, "frames pool: {}", error_text(result));

            // The picture in memory, uploaded into a frame of the pool.
            let mut memory = ff::av_frame_alloc();
            (*memory).format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as c_int;
            (*memory).width = coded.0 as c_int;
            (*memory).height = coded.1 as c_int;
            assert!(ff::av_frame_get_buffer(memory, 0) >= 0);
            for (plane, bytes, row) in [(0, &luma, coded.0), (1, &chroma, coded.0)] {
                let stride = (*memory).linesize[plane] as usize;
                for (y, line) in bytes.chunks(row).enumerate() {
                    std::ptr::copy_nonoverlapping(line.as_ptr(), (*memory).data[plane].add(y * stride), row);
                }
            }
            let mut on_card = ff::av_frame_alloc();
            assert!(ff::av_hwframe_get_buffer(frames_ref, on_card, 0) >= 0);
            let result = ff::av_hwframe_transfer_data(on_card, memory, 0);
            assert!(result >= 0, "upload: {}", error_text(result));
            // As a decoder leaves it: the picture is smaller than the image.
            (*on_card).width = visible.0 as c_int;
            (*on_card).height = visible.1 as c_int;
            (*on_card).colorspace = ff::AVColorSpace::AVCOL_SPC_BT709;
            (*on_card).color_range = ff::AVColorRange::AVCOL_RANGE_JPEG;

            let picture = (*on_card).data[0] as *mut AVVkFrame;
            assert_eq!(
                (*picture).img[1] != vk::Image::null(),
                two_images,
                "the frame has the shape asked for"
            );
            let before = (*picture).sem_value[0];
            let mut copier = Copier::new(device).unwrap();
            let frame = window.copy_out(on_card, &mut copier, 90, 1.25).unwrap();

            assert_eq!(frame.planes.size(), visible);
            assert_eq!(frame.upright_size(), (44, 60));
            assert_eq!((frame.rotation, frame.time), (90, 1.25));
            assert_eq!(frame.colour.matrix, crate::Matrix::Bt709);
            assert_eq!(frame.colour.range, crate::Range::Full);
            // The frame was left as the rules say.
            for i in 0..if two_images { 2 } else { 1 } {
                assert_eq!((*picture).layout[i], LAYOUT_AFTER_COPY);
                assert_eq!((*picture).access[i], ACCESS_AFTER_COPY);
            }
            assert_eq!((*picture).sem_value[0], before + 1);
            // The pair holds the picture, without the padding.
            let (w, h) = (visible.0 as usize, visible.1 as usize);
            let got = video_device::testing::read_back(device, frame.planes.luma(), visible, 1);
            assert_eq!(got, top_left(&luma, coded.0, w, h, 1), "luma");
            let got = video_device::testing::read_back(device, frame.planes.chroma(), (30, 22), 2);
            assert_eq!(got, top_left(&chroma, coded.0 / 2, w / 2, h / 2, 2), "colour");

            // FFmpeg uses the frame again: it waits on the value we left
            // and moves the image on from the layout we recorded.
            (*on_card).width = coded.0 as c_int;
            (*on_card).height = coded.1 as c_int;
            let mut back = ff::av_frame_alloc();
            (*back).format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as c_int;
            let result = ff::av_hwframe_transfer_data(back, on_card, 0);
            assert!(
                result >= 0,
                "FFmpeg reads the frame after us: {}",
                error_text(result)
            );
            let stride = (*back).linesize[0] as usize;
            for y in 0..coded.1 {
                let row = std::slice::from_raw_parts((*back).data[0].add(y * stride), coded.0);
                assert_eq!(row, &luma[y * coded.0..][..coded.0], "row {y} is untouched");
            }
            assert!((*picture).sem_value[0] > before + 1, "and it took its own turn");

            drop(frame);
            drop(copier);
            ff::av_frame_free(&mut back);
            ff::av_frame_free(&mut on_card);
            ff::av_frame_free(&mut memory);
            ff::av_buffer_unref(&mut frames_ref);
        }
    }

    #[test]
    fn only_the_three_codecs_are_ever_asked_of_the_device() {
        let Some(device) = video_device::testing::device() else {
            return;
        };
        let window = WindowDevice::new(device).unwrap();
        // The test device decodes nothing; other codecs aren't even looked up.
        assert!(!window.decodes(ff::AVCodecID::AV_CODEC_ID_H264));
        assert!(!window.decodes(ff::AVCodecID::AV_CODEC_ID_VP9));
    }
}
