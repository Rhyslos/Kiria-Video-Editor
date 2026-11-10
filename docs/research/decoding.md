# Decoding

How video frames get from a file on disk to a texture the viewport and the timeline can draw. Build plan step 4A "Decoding". Playback timing (the audio clock) and caching have their own notes; this one is only about producing frames fast.

## 1. Problem

The editor must turn compressed game recordings into GPU textures fast enough to:

| Need | Target |
| --- | --- |
| Play one 1440p 60 recording in real time | 60 frames a second, every frame on time |
| Play a 1440p 144 recording | 144 frames a second, or drop resolution before frames (preview quality, step 4A) |
| Scrub: show the frame under the pointer | first frame within 50 ms of a jump |
| Picture-in-picture and facecam layouts (4C) | at least 2 streams at once, ideally 4 |
| A 1-hour recording | memory stays flat; no growth per frame |
| Stay off the interface thread | decoding never runs on the thread that draws the window |

The recordings are mostly what a GPU's own recorder writes: H.264, H.265 or AV1, 8-bit, 4:2:0, in MP4 or MKV, often with variable frame rate and long gaps between key frames (2 seconds is common). Seeking to a frame means decoding from the key frame before it, so a jump can cost up to about 120 frames at 60 frames a second. That makes raw decode speed matter for scrubbing too, not only for playback.

Every GPU must work, at least through software decoding.

Fixed parts that any approach builds on:

- **FFmpeg 8.1, linked dynamically** (`packaging/ffmpeg.env`). It already handles every container and codec we need, and it can drive the GPU's video decoder through several interfaces.
- **The window draws with wgpu** (version 30, through the GUI toolkit). On Windows and Linux it can run on Vulkan; on Windows also on Direct3D 12.
- Decoded frames are YUV (luma plus half-size chroma, called NV12 when chroma is interleaved). Converting to RGB is one small shader on the GPU, whichever approach delivers the frames.

## 2. Approaches

### A. Software decoding, then upload

FFmpeg decodes on the CPU (several threads per stream) into YUV planes in memory. The planes are copied into three wgpu textures and a shader converts them.

- Works everywhere, with every codec FFmpeg knows. Needed anyway as the fallback.
- Cost grows with resolution and codec: AV1 and H.265 at 1440p are heavy, and two or four streams at once multiply it. A long key-frame distance makes scrubbing slow, because every frame from the key frame up must be decoded on the CPU.
- Copy cost is small: a 1440p frame in NV12 is about 5.5 MB, so 60 frames a second is about 330 MB/s, far below what a PCIe link carries.

### B. Hardware decoding, copied back to memory, then upload

FFmpeg asks the GPU's video decoder to do the work (Direct3D 11 or 12 video on Windows, VA-API on Linux, or Vulkan Video on both), then copies each finished frame back to memory (`av_hwframe_transfer_data`) and the app uploads it as in A.

- The decoder runs in fixed-function hardware, so CPU load stays low and several streams fit. Current GPUs from the major vendors decode H.264, H.265 and AV1 in hardware.
- Each frame crosses the PCIe link twice (down, then up again). At 1440p 60 that is about 660 MB/s in total, still small, but it adds latency per frame and a CPU copy.
- No sharing of GPU objects between FFmpeg and wgpu, so it is simple and robust: the same code works on every interface FFmpeg supports. A driver problem in one interface can fall back to another, or to A, per file.

### C. Hardware decoding straight into wgpu textures (no copies)

FFmpeg decodes with Vulkan Video on the app's own Vulkan device: the app creates its Vulkan device with the video queues and extensions FFmpeg needs, hands it to FFmpeg (`AVVulkanDeviceContext`: instance, physical device, device, queue families, enabled extensions), and wraps each decoded image (`AVVkFrame`) as a wgpu texture. The shader samples the planes directly.

- No copies at all, lowest latency, lowest CPU load. The best result if it works.
- Most work and most risk:
  - wgpu must be created from a Vulkan device the app made itself (through wgpu's lower layer), with the video queue families and extensions enabled. The GUI toolkit normally creates the device; it has to accept ours.
  - Synchronisation is manual: wait on each frame's timeline semaphore at its value, change the image layout, signal it back when done, and lock frames while FFmpeg's threads use them.
  - Decoded images can be padded to the codec's block size; the shader must clamp chroma sampling.
  - Vulkan Video depends on fairly new drivers. Reports from other projects show it working with recent drivers on Windows and Linux, with older cards and some files hitting driver bugs. The newest cards needed a recent Mesa in 2025.
- Windows only gets this if wgpu runs on Vulkan there.

### Considered and left out

- **A Rust crate that decodes with Vulkan Video into wgpu textures directly.** It shows that C is practical, but it only supports H.264 for now. Worth watching; not enough for H.265 and AV1 recordings.
- **Direct3D 11 decoding shared into wgpu's Direct3D 12 backend through shared handles.** Zero-copy on Windows only, needs wgpu on Direct3D 12, and the public attempts found so far are unproven. C covers the same need on both systems.
- **Decoding in a separate process.** Protects the editor from decoder crashes, but adds a copy per frame. Revisit if decoder crashes show up in practice; the crash reports will tell.

### Which FFmpeg bindings

| Option | For | Against |
| --- | --- | --- |
| Raw generated bindings (the `-sys` crate that tracks FFmpeg versions) | Full API, including the hardware and Vulkan structures C needs; versions follow FFmpeg's | Every call is `unsafe`; we write the safe layer |
| The older safe wrapper on top of it | Less code to write | Mostly in maintenance, and it does not cover the Vulkan hardware structures |
| A newer safe wrapper that already supports FFmpeg 8 | Safer API | Slower release cadence; still needs raw calls for C |

Working rule 6 already allows `unsafe` inside an FFmpeg wrapper, so the plan is the raw bindings, wrapped in one small crate with a safe, explained-error API. The spikes use the raw bindings directly.

## 3. Prototypes

Three spikes in `spikes/decoding/`, one per approach, sharing one measuring harness. Each runs a clip through decode-and-upload into a wgpu texture (no window), then prints one line of results.

**Clips**, the same for all three:

| Clip | Why |
| --- | --- |
| 1080p 60 H.264, 2 s key frames | the most common recording |
| 1440p 60 H.265, 2 s key frames | Checkpoint 4A's clip |
| 1440p 144 H.265 | high frame rate |
| 1440p 60 AV1 | newest codec from the GPU recorder |
| 1440p 60 H.264, variable frame rate, 1 hour | sync and memory over time |
| Real recordings from a GPU recorder | real content (never committed; listed in `test-media/MANIFEST.md`) |

Synthetic clips are generated with a script so anyone can recreate them; real recordings from the GPU's recorder matter more, because encoder settings change how hard a file is to decode.

**Measured for each clip and approach:**

- decode-to-texture frames per second, playing straight through
- time to the first frame after a random jump (median and worst of 100 jumps)
- CPU use while playing
- the same with 2 and 4 streams at once
- memory after 10 minutes, compared with after 1 minute

Software decoding can be measured on any machine. B and C need the real GPU, so the harness is a single program that runs all three on every clip and writes the table; it gets built by CI and run once on the test machine.

## 4. Result

Not measured yet.

Expectation before measuring, to be proven or disproven: B is enough for one 1440p 60 stream and keeps the code simple, A is the required fallback, and C is the only way to keep 4 streams and fast scrubbing cheap. If B meets every target in the table above, it gets built first and C waits until a measured need appears.

## Sources

- [FFmpeg's Vulkan hardware context header](https://ffmpeg.org/doxygen/trunk/hwcontext__vulkan_8h_source.html): the fields an app fills to share its own Vulkan device, and the frame synchronisation rules.
- [Notes from integrating FFmpeg's Vulkan decoding into an engine](https://themaister.net/blog/2023/01/05/vulkan-video-shenanigans-ffmpeg-radv-integration-experiments/) (2023): sharing the device, semaphores, layouts and the padding pitfall.
- [A video player's Vulkan decoding guide and its discussion](https://github.com/mpv-player/mpv/discussions/13909) (2023 to 2026): driver versions and problems per vendor.
- [A Rust crate for Vulkan Video decoding into wgpu textures](https://docs.rs/crate/vk-video/0.3.0/source/README.md): H.264 only, NV12 textures.
- [A Rust media library's notes on sharing decoded Direct3D 11 frames into wgpu](https://docs.rs/crate/mediaway/0.1.3/source/docs/wgpu/roadmap.md): the Windows shared-handle route and its open questions.
- [Direct3D 12 video decoding added to FFmpeg](https://lists.mplayerhq.hu/pipermail/ffmpeg-cvslog/2023-December/140085.html) (December 2023).
- [The state of media crates in Rust, July 2026](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c): binding maintenance status.
