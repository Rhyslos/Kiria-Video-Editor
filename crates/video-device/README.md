# video-device

The window's graphics device as the video decoder shares it. A small crate under both `graphics` (which opens the device) and `media` (which decodes on it), so that neither depends on the other: `media` stays free of the window toolkit, and the launcher stays free of FFmpeg.

## Status

Holds the handle. The decoder's use of it (approach C in `docs/research/decoding.md`) comes with the next step.

## What is in it

`VideoDevice` is everything the decoder needs to know about the window's Vulkan device: the instance, the card and the device, the extensions and features they were created with, the queue families set aside for the decoder, and the kinds of video the device decodes (`Codec`: H.264, H.265, AV1). It also holds the renderer's own handles, so the device outlives every user of the handle.

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

This crate is one of the GPU wrappers where the working rules allow `unsafe`: making a `VideoDevice` is a promise that the handles and the description match one live device.
