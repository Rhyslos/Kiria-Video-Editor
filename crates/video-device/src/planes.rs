//! The images a decoded picture is copied into, for the window to draw.
//!
//! The decoder's own images stay the decoder's: they are its reference
//! pictures, FFmpeg hands them out from a pool of fixed size, and every use
//! of one must take its turn through the frame's semaphore. So a picture
//! that is to be shown is copied, on the graphics card, into two plain
//! images that belong to this side: luma (one byte a pixel) and colour (two
//! bytes for every two-by-two pixels), exactly the two textures the viewer's
//! shader already reads for pictures uploaded from memory.
//!
//! [`PlaneImages`] owns such a pair. Both the helper queue (which fills
//! them) and the window's queue (which samples them) may use them without
//! handing ownership back and forth: they are created as shared between the
//! two families. Dropping a pair puts it back for the next picture of the
//! same size; a few are kept, the rest are destroyed.

use crate::VideoDevice;
use ash::vk;
use std::sync::Arc;

/// How many spare pairs are kept, of all sizes together.
pub(crate) const SPARE_PAIRS: usize = 4;

/// Things of some size waiting to be used again; the oldest goes when there
/// are too many.
#[derive(Debug)]
pub(crate) struct Spare<T> {
    kept: Vec<((u32, u32), T)>,
    limit: usize,
}

impl<T> Spare<T> {
    pub(crate) fn new(limit: usize) -> Spare<T> {
        Spare {
            kept: Vec::new(),
            limit,
        }
    }

    /// The one of this size put back last, if any.
    pub(crate) fn take(&mut self, size: (u32, u32)) -> Option<T> {
        let at = self.kept.iter().rposition(|(s, _)| *s == size)?;
        Some(self.kept.remove(at).1)
    }

    /// Keeps `item`. Returns what no longer fits, to be destroyed.
    pub(crate) fn give(&mut self, size: (u32, u32), item: T) -> Option<T> {
        self.kept.push((size, item));
        (self.kept.len() > self.limit).then(|| self.kept.remove(0).1)
    }

    pub(crate) fn drain(&mut self) -> Vec<T> {
        self.kept.drain(..).map(|(_, item)| item).collect()
    }
}

/// One image with its memory.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
}

/// A luma image and a colour image.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawPair {
    pub luma: RawImage,
    pub chroma: RawImage,
}

/// Half of a size, rounded up: the colour image's size.
pub fn chroma_size(size: (u32, u32)) -> (u32, u32) {
    (size.0.div_ceil(2), size.1.div_ceil(2))
}

/// A pair of images holding one picture: luma at `size`, colour at half of
/// it. Both are left ready for sampling (shader read-only layout) once a
/// picture has been copied in.
pub struct PlaneImages {
    device: Arc<VideoDevice>,
    size: (u32, u32),
    raw: RawPair,
}

impl std::fmt::Debug for PlaneImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PlaneImages({}×{})", self.size.0, self.size.1)
    }
}

impl PlaneImages {
    /// A pair for a picture of `size`, reused when one is spare.
    pub fn new(device: &Arc<VideoDevice>, size: (u32, u32)) -> Result<PlaneImages, String> {
        if size.0 == 0 || size.1 == 0 {
            return Err("a picture with no size".into());
        }
        let spare = device.spare.lock().ok().and_then(|mut s| s.take(size));
        let raw = match spare {
            Some(raw) => raw,
            None => create_pair(device, size)?,
        };
        Ok(PlaneImages {
            device: Arc::clone(device),
            size,
            raw,
        })
    }

    pub fn device(&self) -> &Arc<VideoDevice> {
        &self.device
    }

    /// The luma image's size; the colour image has [`chroma_size`] of it.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The luma image: one byte a pixel (`R8_UNORM`).
    pub fn luma(&self) -> vk::Image {
        self.raw.luma.image
    }

    /// The colour image: U and V for every two-by-two pixels (`R8G8_UNORM`).
    pub fn chroma(&self) -> vk::Image {
        self.raw.chroma.image
    }
}

impl Drop for PlaneImages {
    fn drop(&mut self) {
        let surplus = match self.device.spare.lock() {
            Ok(mut spare) => spare.give(self.size, self.raw),
            Err(_) => Some(self.raw),
        };
        if let Some(raw) = surplus {
            // SAFETY: whoever drops a pair has finished with it on the
            // graphics card too (the copy has been waited for; the window's
            // renderer drops its textures only after its work with them is
            // done), and this pair is no longer in the spare list.
            unsafe { destroy_pair(&self.device, raw) };
        }
    }
}

/// Destroys both images of a pair and frees their memory.
///
/// # Safety
/// The pair must have been made by [`create_pair`] on this device, and no
/// work on the graphics card may still use it.
pub(crate) unsafe fn destroy_pair(device: &VideoDevice, raw: RawPair) {
    for RawImage { image, memory } in [raw.luma, raw.chroma] {
        // SAFETY: made on this device and unused (this function's contract).
        unsafe {
            device.device().destroy_image(image, None);
            device.device().free_memory(memory, None);
        }
    }
}

fn create_pair(device: &VideoDevice, size: (u32, u32)) -> Result<RawPair, String> {
    let luma = create_image(device, vk::Format::R8_UNORM, size)?;
    match create_image(device, vk::Format::R8G8_UNORM, chroma_size(size)) {
        Ok(chroma) => Ok(RawPair { luma, chroma }),
        Err(err) => {
            // SAFETY: the luma image was just made and nothing uses it.
            unsafe {
                device.device().destroy_image(luma.image, None);
                device.device().free_memory(luma.memory, None);
            }
            Err(err)
        }
    }
}

/// One image to copy into and to sample, shared between the helper queue's
/// family and the window's.
fn create_image(device: &VideoDevice, format: vk::Format, size: (u32, u32)) -> Result<RawImage, String> {
    let families = [device.helper().index, device.window_family()];
    let mut info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: size.0,
            height: size.1,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        // Copied into, sampled, and readable back by a copy (for checks).
        .usage(
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // Sharing needs two different families to name.
    if families[0] != families[1] {
        info = info
            .sharing_mode(vk::SharingMode::CONCURRENT)
            .queue_family_indices(&families);
    }
    let vulkan = device.device();
    let failed =
        |what: &str, err: vk::Result| format!("{what} for a {}×{} picture failed ({err})", size.0, size.1);
    // SAFETY: the device is alive for as long as the handle; every object
    // made here is destroyed again on each error path before returning.
    unsafe {
        let image = vulkan
            .create_image(&info, None)
            .map_err(|e| failed("making an image", e))?;
        let needs = vulkan.get_image_memory_requirements(image);
        let Some(kind) = memory_type(device, needs.memory_type_bits) else {
            vulkan.destroy_image(image, None);
            return Err("the graphics card has no memory of the kind an image needs".into());
        };
        let wanted = vk::MemoryAllocateInfo::default()
            .allocation_size(needs.size)
            .memory_type_index(kind);
        let memory = match vulkan.allocate_memory(&wanted, None) {
            Ok(memory) => memory,
            Err(err) => {
                vulkan.destroy_image(image, None);
                return Err(failed("getting memory", err));
            }
        };
        if let Err(err) = vulkan.bind_image_memory(image, memory, 0) {
            vulkan.destroy_image(image, None);
            vulkan.free_memory(memory, None);
            return Err(failed("giving an image its memory", err));
        }
        Ok(RawImage { image, memory })
    }
}

/// A memory type among `allowed` (one bit per type), on the card itself when there is one.
fn memory_type(device: &VideoDevice, allowed: u32) -> Option<u32> {
    // SAFETY: the instance and the card are alive for as long as the handle.
    let memory = unsafe {
        device
            .instance()
            .get_physical_device_memory_properties(device.physical_device())
    };
    let kinds = memory.memory_types_as_slice();
    let fits = |i: &usize| allowed & (1 << i) != 0;
    let on_card = |i: &usize| {
        kinds[*i]
            .property_flags
            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
    };
    let found = (0..kinds.len())
        .filter(fits)
        .find(on_card)
        .or_else(|| (0..kinds.len()).find(fits));
    found.map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spare_things_come_back_by_size_newest_first() {
        let mut spare = Spare::new(4);
        assert_eq!(spare.give((1920, 1080), "a"), None);
        assert_eq!(spare.give((1280, 720), "b"), None);
        assert_eq!(spare.give((1920, 1080), "c"), None);
        assert_eq!(spare.take((640, 360)), None, "none of that size");
        assert_eq!(spare.take((1920, 1080)), Some("c"));
        assert_eq!(spare.take((1920, 1080)), Some("a"));
        assert_eq!(spare.take((1920, 1080)), None);
        assert_eq!(spare.take((1280, 720)), Some("b"));
    }

    #[test]
    fn no_more_than_the_limit_is_kept_and_the_oldest_goes_first() {
        let mut spare = Spare::new(2);
        assert_eq!(spare.give((4, 4), 1), None);
        assert_eq!(spare.give((8, 8), 2), None);
        assert_eq!(
            spare.give((4, 4), 3),
            Some(1),
            "one too many: the oldest is handed back"
        );
        assert_eq!(spare.give((4, 4), 4), Some(2));
        assert_eq!(spare.drain(), [3, 4]);
        assert_eq!(spare.take((4, 4)), None, "drained");
    }

    #[test]
    fn the_colour_image_is_half_the_size_rounded_up() {
        assert_eq!(chroma_size((2560, 1440)), (1280, 720));
        assert_eq!(chroma_size((1365, 767)), (683, 384));
        assert_eq!(chroma_size((1, 1)), (1, 1));
    }
}
