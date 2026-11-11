//! Copying a decoded picture from the decoder's image into [`PlaneImages`],
//! on the graphics card.
//!
//! One submission on the helper queue does it, following the rules FFmpeg
//! sets for anyone who touches one of its frames (`AVVkFrame` in
//! `libavutil/hwcontext_vulkan.h`; `media` reads and writes the frame's
//! fields, this module does the Vulkan part):
//!
//! - "Must be waited on at every submission using the value in sem_value,
//!   and must be signalled at every submission, using an incremented value":
//!   the submission waits for each of the frame's semaphores at its current
//!   value, so the decoding of that picture has finished before the copy
//!   reads it, and signals the next value, so the decoder's next use of the
//!   image (as a reference for later pictures, or to decode into again)
//!   waits for the copy.
//! - "Updated after every barrier": the image is moved to the layout for
//!   reading by a copy, and the caller writes that layout back to the frame.
//! - The queue is locked the way FFmpeg locks it for the moment of
//!   submitting, by the caller's `lock` and `unlock`.
//!
//! The target images end in the layout for sampling, and the copy is waited
//! for before anyone else sees them, so the window's renderer needs no
//! semaphore of its own: by the time it gets the pair, the picture is in it.

use crate::{PlaneImages, VideoDevice, chroma_size};
use ash::vk;
use std::sync::Arc;

/// One of the decoder's images, as its frame describes it at this moment.
#[derive(Debug, Clone, Copy)]
pub struct SourceImage {
    pub image: vk::Image,
    /// The layout and access the image was left in by its last user.
    pub layout: vk::ImageLayout,
    pub access: vk::AccessFlags,
    /// The frame's timeline semaphore and the value at which the image is
    /// ready; null when nothing has to be waited for.
    pub semaphore: vk::Semaphore,
    pub value: u64,
}

/// Where a decoded picture's two planes are.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// One image with a luma plane and a plane of interleaved colour (what
    /// FFmpeg's decoder makes when the card has such a format).
    TwoPlanes(SourceImage),
    /// Two plain images: luma, and interleaved colour at half size.
    TwoImages(SourceImage, SourceImage),
}

impl Source {
    fn images(&self) -> Vec<SourceImage> {
        match *self {
            Source::TwoPlanes(both) => vec![both],
            Source::TwoImages(luma, chroma) => vec![luma, chroma],
        }
    }
}

/// What the caller writes back to the frame after [`Copier::copy`]: the
/// layout and access every source image was left in. Each semaphore's value
/// has gone up by one.
pub const LAYOUT_AFTER_COPY: vk::ImageLayout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
pub const ACCESS_AFTER_COPY: vk::AccessFlags = vk::AccessFlags::TRANSFER_READ;

/// How long a copy may take before it counts as stuck (it normally takes
/// the time the picture's decoding still needs, a few milliseconds).
const COPY_TIMEOUT_NS: u64 = 5_000_000_000;

/// The region of each source plane that is the visible picture: the luma
/// plane's and the colour plane's top left corner and size, for a picture
/// of `visible` size starting `origin` pixels into the decoded image.
/// Decoded images are padded to the codec's block size, and the padding is
/// never copied. The corner is rounded down to whole colour samples.
pub fn visible_regions(visible: (u32, u32), origin: (u32, u32)) -> [((u32, u32), (u32, u32)); 2] {
    let origin = (origin.0 & !1, origin.1 & !1);
    [
        (origin, visible),
        ((origin.0 / 2, origin.1 / 2), chroma_size(visible)),
    ]
}

/// The command buffer and fence for copying pictures, one after another.
/// Used by one thread at a time (the decoder's).
pub struct Copier {
    device: Arc<VideoDevice>,
    queue: vk::Queue,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    /// A copy was submitted and has not been seen to finish.
    under_way: bool,
}

impl std::fmt::Debug for Copier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Copier")
    }
}

impl Copier {
    pub fn new(device: &Arc<VideoDevice>) -> Result<Copier, String> {
        let vulkan = device.device();
        let family = device.helper().index;
        let failed = |what: &str, err: vk::Result| format!("{what} failed ({err})");
        // SAFETY: the device is alive for as long as the handle; the helper
        // family has its queue 0 (the handle's promise); what is made here
        // is destroyed on the error paths and in `drop`.
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
            let commands = vulkan.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            );
            let commands = match commands {
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
            Ok(Copier {
                device: Arc::clone(device),
                queue,
                pool,
                commands,
                fence,
                under_way: false,
            })
        }
    }

    /// Submits the copy of the visible picture (`visible` pixels, starting
    /// `origin` pixels into the source) into `target`, and returns without
    /// waiting for it: call [`Copier::wait`] next, after writing
    /// [`LAYOUT_AFTER_COPY`], [`ACCESS_AFTER_COPY`] and the raised semaphore
    /// values back to the frame.
    ///
    /// `lock` and `unlock` are called around the submission, to lock the
    /// helper queue against everyone else who submits to it.
    ///
    /// # Safety
    /// - The source images belong to this device, exist until the copy has
    ///   been waited for, are at least `origin + visible` in size with their
    ///   colour at half size, and were created shared with the helper
    ///   queue's family (or for it alone) and usable as the source of a copy.
    /// - `source` says what is true of them now, and nobody changes that
    ///   before this returns (for FFmpeg's frames: the frame is locked).
    /// - `target` is `visible` in size and nothing else uses it.
    /// - The previous copy, if any, has been waited for.
    pub unsafe fn copy(
        &mut self,
        source: &Source,
        visible: (u32, u32),
        origin: (u32, u32),
        target: &PlaneImages,
        lock: &dyn Fn(),
        unlock: &dyn Fn(),
    ) -> Result<(), String> {
        // The one command buffer is free again only once its last copy is done.
        self.wait()?;
        let vulkan = self.device.device();
        let failed = |what: &str, err: vk::Result| format!("{what} failed ({err})");
        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        // Layout changes that hand nothing over between queue families.
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
        let nothing = vk::AccessFlags::empty();
        let sources = source.images();
        let targets = [target.luma(), target.chroma()];
        // The source to "being copied from"; the targets, whose old contents
        // don't matter, to "being copied into".
        let mut before: Vec<vk::ImageMemoryBarrier<'_>> = sources
            .iter()
            .map(|s| {
                change(
                    s.image,
                    (s.layout, s.access),
                    (LAYOUT_AFTER_COPY, ACCESS_AFTER_COPY),
                )
            })
            .collect();
        before.extend(targets.iter().map(|image| {
            change(
                *image,
                (vk::ImageLayout::UNDEFINED, nothing),
                (
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::AccessFlags::TRANSFER_WRITE,
                ),
            )
        }));
        // The targets to "being sampled", where the window's renderer expects them.
        let after: Vec<vk::ImageMemoryBarrier<'_>> = targets
            .iter()
            .map(|image| {
                change(
                    *image,
                    (
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::AccessFlags::TRANSFER_WRITE,
                    ),
                    (
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::AccessFlags::SHADER_READ,
                    ),
                )
            })
            .collect();
        // Each plane: where it is in the source, and the picture's part of it.
        let planes = match *source {
            Source::TwoPlanes(both) => [
                (both.image, vk::ImageAspectFlags::PLANE_0),
                (both.image, vk::ImageAspectFlags::PLANE_1),
            ],
            Source::TwoImages(luma, chroma) => [
                (luma.image, vk::ImageAspectFlags::COLOR),
                (chroma.image, vk::ImageAspectFlags::COLOR),
            ],
        };
        let regions = visible_regions(visible, origin);
        let layers = |aspect| {
            vk::ImageSubresourceLayers::default()
                .aspect_mask(aspect)
                .layer_count(1)
        };
        // Wait for every source image at its value; signal the next.
        let waits: Vec<vk::SemaphoreSubmitInfo<'_>> = sources
            .iter()
            .filter(|s| s.semaphore != vk::Semaphore::null())
            .map(|s| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(s.semaphore)
                    .value(s.value)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect();
        let signals: Vec<vk::SemaphoreSubmitInfo<'_>> = waits.iter().map(|w| w.value(w.value + 1)).collect();
        // SAFETY: the command buffer, fence and queue are this copier's and
        // idle (the last copy was waited for); the images are valid, in the
        // layouts given and large enough for the regions (this function's
        // contract); the queue is locked around the submission; the arrays
        // passed to Vulkan outlive the calls that read them.
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
            for (((from, aspect), (corner, size)), to) in planes.into_iter().zip(regions).zip(targets) {
                let region = vk::ImageCopy::default()
                    .src_subresource(layers(aspect))
                    .src_offset(vk::Offset3D {
                        x: corner.0 as i32,
                        y: corner.1 as i32,
                        z: 0,
                    })
                    .dst_subresource(layers(vk::ImageAspectFlags::COLOR))
                    .extent(vk::Extent3D {
                        width: size.0,
                        height: size.1,
                        depth: 1,
                    });
                vulkan.cmd_copy_image(
                    self.commands,
                    from,
                    LAYOUT_AFTER_COPY,
                    to,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            vulkan.cmd_pipeline_barrier(
                self.commands,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &after,
            );
            vulkan
                .end_command_buffer(self.commands)
                .map_err(|e| failed("finishing the commands", e))?;
            let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(self.commands)];
            let submission = [vk::SubmitInfo2::default()
                .wait_semaphore_infos(&waits)
                .command_buffer_infos(&commands)
                .signal_semaphore_infos(&signals)];
            lock();
            let submitted = vulkan.queue_submit2(self.queue, &submission, self.fence);
            unlock();
            self.under_way = submitted.is_ok();
            submitted.map_err(|e| failed("submitting the copy", e))
        }
    }

    /// Waits until the copy submitted last has finished. Returns at once
    /// when none is under way.
    pub fn wait(&mut self) -> Result<(), String> {
        if !self.under_way {
            return Ok(());
        }
        // SAFETY: the fence is this copier's and was given to the copy under way.
        let waited = unsafe {
            self.device
                .device()
                .wait_for_fences(&[self.fence], true, COPY_TIMEOUT_NS)
        };
        match waited {
            Ok(()) => {
                self.under_way = false;
                Ok(())
            }
            Err(vk::Result::TIMEOUT) => {
                Err("the graphics card didn't finish copying the picture in five seconds".into())
            }
            Err(other) => Err(format!("waiting for the copy failed ({other})")),
        }
    }
}

impl Drop for Copier {
    fn drop(&mut self) {
        // A copy still running (only after a failed wait) is given its time
        // before its command buffer goes.
        let _ = self.wait();
        // SAFETY: made by this copier on this device, and no longer in use.
        unsafe {
            let vulkan = self.device.device();
            vulkan.destroy_fence(self.fence, None);
            vulkan.destroy_command_pool(self.pool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_visible_picture_is_copied_never_the_padding() {
        // 1080 rows are decoded into 1088: the copy takes 1920×1080.
        assert_eq!(
            visible_regions((1920, 1080), (0, 0)),
            [((0, 0), (1920, 1080)), ((0, 0), (960, 540))]
        );
        // Odd sizes: the colour plane rounds up, as the target images do.
        assert_eq!(
            visible_regions((1365, 767), (0, 0)),
            [((0, 0), (1365, 767)), ((0, 0), (683, 384))]
        );
        // A picture cropped at the left and top starts further in, on a
        // whole colour sample.
        assert_eq!(
            visible_regions((1280, 720), (8, 4)),
            [((8, 4), (1280, 720)), ((4, 2), (640, 360))]
        );
        assert_eq!(visible_regions((1280, 720), (9, 5))[0].0, (8, 4));
    }

    /// A 16×16 picture as the decoder would leave it: luma counts up along
    /// each row, colour is (row, column) of its sample.
    fn pattern() -> (Vec<u8>, Vec<u8>) {
        let luma = (0..16 * 16).map(|i| (i % 16 + 16 * (i / 16)) as u8).collect();
        let chroma = (0..8 * 8)
            .flat_map(|i| [100 + (i / 8) as u8, 200 + (i % 8) as u8])
            .collect();
        (luma, chroma)
    }

    /// The stand-in image in the layout it was last left in.
    fn source_in(image: vk::Image, layout: vk::ImageLayout) -> Source {
        Source::TwoPlanes(SourceImage {
            image,
            layout,
            access: vk::AccessFlags::TRANSFER_READ,
            semaphore: vk::Semaphore::null(),
            value: 0,
        })
    }

    #[test]
    fn a_picture_is_copied_plane_by_plane_without_its_padding() {
        let Some(device) = crate::testing::device() else {
            return;
        };
        let (luma, chroma) = pattern();
        let picture = crate::testing::stand_in_picture(&device, (16, 16), &luma, &chroma);
        let mut copier = Copier::new(&device).unwrap();
        // The visible picture is 12×10 of the 16×16 that were decoded,
        // first from the corner, then cropped in by (4, 2).
        let mut layout = crate::testing::STAND_IN_LAYOUT;
        for origin in [(0u32, 0u32), (4, 2)] {
            let target = PlaneImages::new(&device, (12, 10)).unwrap();
            // SAFETY: the stand-in image is this device's, 16×16, in the
            // layout named, idle, and outlives the wait below; the target
            // is 12×10 and only used here.
            unsafe {
                copier
                    .copy(
                        &source_in(picture.0, layout),
                        (12, 10),
                        origin,
                        &target,
                        &|| (),
                        &|| (),
                    )
                    .unwrap();
            }
            // What the caller writes back to the frame.
            layout = LAYOUT_AFTER_COPY;
            copier.wait().unwrap();
            let got = crate::testing::read_back(&device, target.luma(), (12, 10), 1);
            let want: Vec<u8> = (0..10)
                .flat_map(|y| (0..12).map(move |x| (x + origin.0 + 16 * (y + origin.1)) as u8))
                .collect();
            assert_eq!(got, want, "luma from {origin:?}");
            let got = crate::testing::read_back(&device, target.chroma(), (6, 5), 2);
            let want: Vec<u8> = (0..5)
                .flat_map(|y| {
                    (0..6).flat_map(move |x| [100 + (y + origin.1 / 2) as u8, 200 + (x + origin.0 / 2) as u8])
                })
                .collect();
            assert_eq!(got, want, "colour from {origin:?}");
        }
        drop(copier);
        crate::testing::destroy_stand_in(&device, picture);
    }

    #[test]
    fn the_copy_takes_its_turn_through_the_frames_semaphore() {
        let Some(device) = crate::testing::device() else {
            return;
        };
        let (luma, chroma) = pattern();
        let picture = crate::testing::stand_in_picture(&device, (16, 16), &luma, &chroma);
        let vulkan = device.device();
        // A timeline semaphore like a frame's, already at the value the
        // picture is ready at.
        let mut timeline = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(7);
        // SAFETY: plain Vulkan calls on a live device; the semaphore is
        // destroyed below, after the copy that uses it has been waited for.
        let semaphore = unsafe {
            vulkan
                .create_semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut timeline), None)
                .unwrap()
        };
        let mut source = SourceImage {
            image: picture.0,
            layout: crate::testing::STAND_IN_LAYOUT,
            access: vk::AccessFlags::TRANSFER_READ,
            semaphore,
            value: 7,
        };
        let mut copier = Copier::new(&device).unwrap();
        let locked = std::cell::Cell::new(0);
        // Two copies in a row, as two pictures shown from the same image
        // would be: each waits at the value the one before signalled.
        for round in 0..2 {
            let target = PlaneImages::new(&device, (16, 16)).unwrap();
            // SAFETY: as in the test above; the semaphore is at `value`.
            unsafe {
                copier
                    .copy(
                        &Source::TwoPlanes(source),
                        (16, 16),
                        (0, 0),
                        &target,
                        &|| locked.set(locked.get() + 1),
                        &|| locked.set(locked.get() + 10),
                    )
                    .unwrap();
            }
            // What the caller writes back to the frame.
            source.value += 1;
            source.layout = LAYOUT_AFTER_COPY;
            source.access = ACCESS_AFTER_COPY;
            copier.wait().unwrap();
            // The copy's fence can be seen a moment before its signal on the
            // semaphore: wait for the value, as whoever uses the frame next does.
            let (semaphores, values) = ([semaphore], [8 + round]);
            let wait = vk::SemaphoreWaitInfo::default()
                .semaphores(&semaphores)
                .values(&values);
            // SAFETY: the semaphore is alive, and the wait is bounded.
            unsafe { vulkan.wait_semaphores(&wait, 5_000_000_000).unwrap() };
            // SAFETY: the semaphore is alive.
            let reached = unsafe { vulkan.get_semaphore_counter_value(semaphore).unwrap() };
            assert_eq!(reached, 8 + round, "signalled one further each time");
            assert_eq!(
                crate::testing::read_back(&device, target.luma(), (16, 16), 1),
                luma
            );
        }
        assert_eq!(
            locked.get(),
            22,
            "the queue was locked and unlocked around each submission"
        );
        drop(copier);
        // SAFETY: nothing uses the semaphore any more.
        unsafe { vulkan.destroy_semaphore(semaphore, None) };
        crate::testing::destroy_stand_in(&device, picture);
    }

    #[test]
    fn a_copier_that_never_copied_waits_for_nothing() {
        let Some(device) = crate::testing::device() else {
            return;
        };
        let started = std::time::Instant::now();
        let mut copier = Copier::new(&device).unwrap();
        copier.wait().unwrap();
        drop(copier);
        // Nothing was submitted, so there is no fence to sit out: a decoder
        // that the device turned down at its first picture closes at once.
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_pair_put_back_is_handed_out_again_for_the_same_size() {
        let Some(device) = crate::testing::device() else {
            return;
        };
        let first = PlaneImages::new(&device, (64, 36)).unwrap();
        let (luma, chroma) = (first.luma(), first.chroma());
        assert_ne!(luma, chroma);
        let other = PlaneImages::new(&device, (64, 36)).unwrap();
        assert_ne!(other.luma(), luma, "two pictures at once get two pairs");
        drop(first);
        let again = PlaneImages::new(&device, (64, 36)).unwrap();
        assert_eq!((again.luma(), again.chroma()), (luma, chroma));
        let smaller = PlaneImages::new(&device, (32, 18)).unwrap();
        assert_ne!(smaller.luma(), luma);
        assert!(PlaneImages::new(&device, (0, 18)).is_err());
    }

    #[test]
    fn after_the_copy_the_source_is_left_readable() {
        assert_eq!(LAYOUT_AFTER_COPY, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        assert_eq!(ACCESS_AFTER_COPY, vk::AccessFlags::TRANSFER_READ);
    }
}
