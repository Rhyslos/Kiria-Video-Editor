# video-device

The window's graphics device as the video decoder shares it. A small crate under both `graphics` (which opens the device) and `media` (which decodes on it), so that neither depends on the other: `media` stays free of the window toolkit, and the launcher stays free of FFmpeg.

## Status

Done for approach C in `docs/research/decoding.md`: the handle, the images a decoded picture is shown from, and the copy that fills them on the graphics card.

## What is in it

`VideoDevice` is everything the decoder needs to know about the window's Vulkan device: the instance, the card and the device, the extensions and features they were created with, the queue families set aside for the decoder, and the kinds of video the device decodes (`Codec`: H.264, H.265, AV1). It also holds the renderer's own handles, so the device outlives every user of the handle.

## From the decoder's image to the viewer's

The viewer never draws from the decoder's own image. That image is one of a fixed number, the decoder keeps using it as a reference for the pictures that follow, and it may be larger than the picture (padded to the codec's block size). Instead, each picture that is shown is copied on the graphics card, never through system memory, into a pair of plain images:

- `PlaneImages` is the pair: luma with one byte a pixel, and interleaved colour with two bytes a sample at half size (rounded up), both the size of the picture alone. They are made for sampling and shared between the helper queue's family (which fills them) and the window's (which samples them). Dropping a pair gives its images back to the device, which keeps the last few (`SPARE_PAIRS`) for the next pictures of the same size: during playback no image is created or destroyed.
- `Copier` copies one picture: one submission on the helper queue that waits for the decoder's image, copies the picture's part of each plane (`visible_regions`), and leaves the pair ready for sampling. It follows the rules FFmpeg sets for anyone who touches one of its frames; they are quoted in `src/copier.rs`. The copy is waited for (a fence, at most five seconds) before the pair is handed on, so the renderer needs no semaphore.

`media` reads and writes FFmpeg's frame around the copy; `graphics` wraps the pair as the renderer's textures.

## Who submits to which queue

A Vulkan queue takes work from one thread at a time, and the window's renderer offers no lock around its own. The queues are therefore kept apart instead of shared:

| Queue | Used by |
|---|---|
| The window's (queue 0 of family 0) | the renderer, alone |
| One in each video decode family | FFmpeg's decoder |
| The helper: one in a compute family other than the window's | FFmpeg's own housekeeping (preparing its images), and copying decoded pictures |

FFmpeg guards each of its queues with its own lock; anything else that submits on the decoder's side takes that same lock. A card with no spare compute family gets no `VideoDevice`: its video is decoded the older ways.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | `VideoDevice`, `Parts` (what it is made from), `QueueFamily`, `Codec` |
| `src/planes.rs` | `PlaneImages`, `chroma_size`, and the short list of spare pairs (`Spare`) |
| `src/copier.rs` | `Copier`, `Source` and `SourceImage` (the decoder's image as its frame describes it), `visible_regions` |
| `src/testing.rs` | For tests here and in the crates above (feature `testing`): a device without a window on whatever Vulkan the machine has, a stand-in for a decoded picture, and reading an image back |

## Tests

The pure parts (plane sizes, the picture's regions inside a padded image, the spare list with a stand-in type) run everywhere. The rest runs on whatever Vulkan the machine has, a software driver included, and passes without checking when there is none: a padded and cropped picture is copied and read back, the decoder's semaphore is waited for and signalled, the queue lock is taken, and pairs are reused.

This crate is one of the GPU wrappers where the working rules allow `unsafe`: making a `VideoDevice` is a promise that the handles and the description match one live device, and `Copier::copy` a promise that the source images are as described.
