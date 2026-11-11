//! The window's graphics device as the video decoder shares it.
//!
//! Video decodes fastest straight into the device that draws the window
//! (`docs/research/decoding.md`, approach C). For that, the device has to be
//! opened with its video decoding queues and extensions, and the decoder has
//! to be told exactly how it was opened. [`VideoDevice`] is that knowledge in
//! one handle: `crates/graphics` fills it in when it opens the window's
//! Vulkan device, and `crates/media` hands it to FFmpeg. Neither depends on
//! the other, which is why this small crate sits under both.
//!
//! # Who submits to which queue
//!
//! A Vulkan queue takes work from one thread at a time, and the window's
//! renderer offers no lock around its own. So the queues are kept apart
//! instead of shared:
//!
//! - the **window's queue** belongs to the renderer alone;
//! - the **decode queues** and one **helper queue** (a compute queue of
//!   another family, for the decoder's own housekeeping and for copying
//!   decoded pictures) belong to the decoder's side.
//!
//! A card without such a helper queue doesn't get a [`VideoDevice`] at all:
//! its video is decoded the older ways. On the decoder's side, FFmpeg guards
//! each queue with its own lock, and everything else that submits there
//! takes the same lock.

// This crate is one of the GPU wrappers the working rules allow `unsafe` in.
#![allow(unsafe_code)]

mod copier;
mod planes;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use ash::vk;
use planes::{RawPair, SPARE_PAIRS, Spare};
use std::any::Any;
use std::ffi::CString;
use std::sync::Mutex;

pub use ash;
pub use copier::{ACCESS_AFTER_COPY, Copier, LAYOUT_AFTER_COPY, Source, SourceImage, visible_regions};
pub use planes::{PlaneImages, chroma_size};

/// A kind of video the device's decoder takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
    Av1,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Codec::H264, Codec::H265, Codec::Av1];

    pub fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::H265 => "H.265",
            Codec::Av1 => "AV1",
        }
    }

    /// The decode operation as Vulkan names it.
    pub fn operation(self) -> vk::VideoCodecOperationFlagsKHR {
        match self {
            Codec::H264 => vk::VideoCodecOperationFlagsKHR::DECODE_H264,
            Codec::H265 => vk::VideoCodecOperationFlagsKHR::DECODE_H265,
            Codec::Av1 => vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
        }
    }
}

/// One of the device's queue families that the decoder's side uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFamily {
    pub index: u32,
    /// What this family is used for (not everything it could do).
    pub flags: vk::QueueFlags,
    /// For a decode family: the kinds of video its queue decodes.
    pub codecs: vk::VideoCodecOperationFlagsKHR,
}

/// What [`VideoDevice::new`] is made from.
pub struct Parts {
    /// Vulkan's entry point, for loading functions by name.
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    /// The extensions the instance and the device were created with.
    pub instance_extensions: Vec<CString>,
    pub device_extensions: Vec<CString>,
    /// The family of the window's own queue. The decoder's side never submits there.
    pub window_family: u32,
    /// The helper queue's family: compute and transfer work, queue 0.
    pub helper: QueueFamily,
    /// The families with a video decode queue (queue 0 of each).
    pub decode: Vec<QueueFamily>,
    /// What the decoder may decode here: offered by a decode queue and by an
    /// enabled extension.
    pub codecs: Vec<Codec>,
    /// Device features switched on besides the renderer's own.
    pub synchronization2: bool,
    pub timeline_semaphore: bool,
    pub video_maintenance1: bool,
    /// What keeps the instance and the device alive (the renderer's own handles).
    pub keep_alive: Box<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("window_family", &self.window_family)
            .field("helper", &self.helper)
            .field("decode", &self.decode)
            .field("codecs", &self.codecs)
            .field("device_extensions", &self.device_extensions)
            .finish_non_exhaustive()
    }
}

/// The window's Vulkan device, opened for video decoding: every fact the
/// decoder needs to use it, and a hold on the device so it outlives every user.
pub struct VideoDevice {
    /// Image pairs waiting for the next picture of their size.
    spare: Mutex<Spare<RawPair>>,
    // Last: the device goes after everything made on it.
    parts: Parts,
}

impl Drop for VideoDevice {
    fn drop(&mut self) {
        let spare = self.spare.get_mut().map(|s| s.drain()).unwrap_or_default();
        for pair in spare {
            // SAFETY: spare pairs are used by nobody, and the device is
            // still alive: `parts.keep_alive` is dropped after this.
            unsafe { planes::destroy_pair(self, pair) };
        }
    }
}

impl std::fmt::Debug for VideoDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VideoDevice({})", self.codec_names())
    }
}

impl VideoDevice {
    /// # Safety
    /// Every handle in `parts` must belong to one live Vulkan device that
    /// `parts.keep_alive` keeps alive; the extension, queue family and
    /// feature fields must say how that device was really created, with one
    /// queue (index 0) created in the helper family and in each decode
    /// family; and nothing but the decoder's side may ever submit to those queues.
    pub unsafe fn new(parts: Parts) -> VideoDevice {
        VideoDevice {
            spare: Mutex::new(Spare::new(SPARE_PAIRS)),
            parts,
        }
    }

    pub fn get_instance_proc_addr(&self) -> vk::PFN_vkGetInstanceProcAddr {
        self.parts.get_instance_proc_addr
    }

    pub fn instance(&self) -> &ash::Instance {
        &self.parts.instance
    }

    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.parts.physical_device
    }

    pub fn device(&self) -> &ash::Device {
        &self.parts.device
    }

    pub fn instance_extensions(&self) -> &[CString] {
        &self.parts.instance_extensions
    }

    pub fn device_extensions(&self) -> &[CString] {
        &self.parts.device_extensions
    }

    pub fn window_family(&self) -> u32 {
        self.parts.window_family
    }

    pub fn helper(&self) -> QueueFamily {
        self.parts.helper
    }

    pub fn decode(&self) -> &[QueueFamily] {
        &self.parts.decode
    }

    pub fn codecs(&self) -> &[Codec] {
        &self.parts.codecs
    }

    pub fn decodes(&self, codec: Codec) -> bool {
        self.parts.codecs.contains(&codec)
    }

    pub fn synchronization2(&self) -> bool {
        self.parts.synchronization2
    }

    pub fn timeline_semaphore(&self) -> bool {
        self.parts.timeline_semaphore
    }

    pub fn video_maintenance1(&self) -> bool {
        self.parts.video_maintenance1
    }

    /// "H.264, H.265, AV1".
    pub fn codec_names(&self) -> String {
        codec_names(&self.parts.codecs)
    }

    /// The card's own memory, in bytes: its largest heap that lives on the
    /// card. 0 when the card reports none (graphics built into the
    /// processor share the computer's memory).
    pub fn card_memory(&self) -> u64 {
        // SAFETY: the instance and the physical device are alive for as
        // long as `self` (`parts.keep_alive`); this only reads properties.
        let memory = unsafe {
            self.parts
                .instance
                .get_physical_device_memory_properties(self.parts.physical_device)
        };
        memory
            .memory_heaps_as_slice()
            .iter()
            .filter(|heap| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|heap| heap.size)
            .max()
            .unwrap_or(0)
    }
}

/// The codecs' names in one line, as Diagnostics shows them.
pub fn codec_names(codecs: &[Codec]) -> String {
    let names: Vec<&str> = codecs.iter().map(|c| c.label()).collect();
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codecs_read_as_a_list() {
        assert_eq!(codec_names(&Codec::ALL), "H.264, H.265, AV1");
        assert_eq!(codec_names(&[Codec::H265]), "H.265");
        assert_eq!(codec_names(&[]), "");
    }

    #[test]
    fn each_codec_is_its_own_vulkan_operation() {
        let all = Codec::ALL
            .iter()
            .fold(vk::VideoCodecOperationFlagsKHR::empty(), |a, c| a | c.operation());
        assert_eq!(all.as_raw().count_ones(), 3);
        assert!(!all.contains(vk::VideoCodecOperationFlagsKHR::ENCODE_H264));
    }

    #[test]
    fn the_handle_can_cross_threads() {
        fn crosses<T: Send + Sync>() {}
        crosses::<VideoDevice>();
    }
}
