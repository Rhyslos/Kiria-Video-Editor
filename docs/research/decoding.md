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

**Software decoding on a 2-thread cloud machine** (no GPU; FFmpeg 8.1; 20-second synthetic clips; frames not uploaded):

| Clip | Play fps | CPU % (100 = one thread) | Jump median ms | Jump worst ms | 2 streams fps | 4 streams fps |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| H.264 1080p 60 | 278 | 162 | 207 | 388 | 337 | 309 |
| H.265 1440p 60 | 93 | 175 | 785 | 1361 | 100 | 97 |
| AV1 1440p 60 | 154 | 158 | 450 | 774 | 204 | 195 |
| H.265 1440p 144 | 126 | 175 | 1124 | 2302 | 138 | 139 |

Even where playing keeps up, a jump takes 0.2 to 2.3 seconds, because every frame from the key frame before the target is decoded on the CPU. That is 4 to 46 times the 50 ms target. A desktop CPU with more cores is faster, but not by that much, so software decoding alone can't meet the scrubbing target; it stays as the fallback. Approaches B and C need the real GPU and are measured with the Windows package from the "Decoding spike" workflow.


**On the test machine** (Windows, a current desktop GPU with hardware H.264, H.265 and AV1 decoding, a 24-thread CPU; generated clips plus two real 1440p H.265 game recordings; median of the measurements below):

| Clip | Approach | Play fps | CPU % | Jump median ms | Jump worst ms |
| --- | --- | ---: | ---: | ---: | ---: |
| H.264 1080p 60 | A software | 1723 | 978 | 45 | 67 |
| H.264 1080p 60 | B hardware, Direct3D 11 | 980 | 97 | 47 | 94 |
| H.264 1080p 60 | C zero-copy, Vulkan | 1158 | 57 | 41 | 85 |
| H.265 1440p 60 | A software | 613 | 1116 | 137 | 250 |
| H.265 1440p 60 | B hardware, Direct3D 11 | 452 | 79 | 98 | 257 |
| H.265 1440p 60 | C zero-copy, Vulkan | 519 | 28 | 88 | 232 |
| H.265 1440p 144 | B hardware, Direct3D 11 | 473 | 73 | 205 | 568 |
| H.265 1440p 144 | C zero-copy, Vulkan | 556 | 26 | 189 | 505 |
| Real recordings (H.265 1440p) | B hardware, Direct3D 11 | 256–259 | 53–69 | 250–306 | 620–798 |
| Real recordings (H.265 1440p) | B hardware, Direct3D 12 | 263–264 | 50–60 | 247–304 | 614–792 |
| Real recordings (H.265 1440p) | C zero-copy, Vulkan | 526–535 | 21–30 | 169–172 | 457–651 |
| AV1 1440p 60 | A software | 190 | 182 | 348 | 650 |

What it shows:

- **C works on this card and is the best on real recordings:** twice B's playing speed, a third less waiting on a jump, half the CPU. On generated clips the gap is smaller.
- **B through Direct3D 11 is the safe fallback;** Direct3D 12 is no better and refused the H.264 clip. FFmpeg's Vulkan decoding with a copy back (B on Vulkan) failed on every clip, so it is never used.
- **AV1 never reached the GPU,** in any approach. FFmpeg's default AV1 decoder is a software one with no GPU support; the GPU path needs FFmpeg's own AV1 decoder, picked by name. The editor now picks the decoder that supports the GPU for each codec, and falls back to the CPU per file when the GPU can't decode one.
- **Software decoding is strong with many CPU threads,** even matching the GPU on H.264 jumps, at ten times the CPU.
- **No approach meets the 50 ms jump target on real H.265 recordings** (about 170 ms at best). Their key frames are far apart, so a jump decodes many frames. The frame cache (showing the nearest key frame at once, then the exact frame) has to close that gap; faster decoding alone can't.
- The real recordings use full-range colour (`yuvj420p`), which the spike's software path didn't handle; the editor converts it.

**Choice (November 2026):** the owner chose to go ahead without running the GPU measurements. The decoder tries the approaches in order and falls back per file, so a failure only costs speed, never a picture:

1. **C, zero-copy Vulkan,** when wgpu runs on Vulkan and the driver offers a video decode queue for the codec. Measured above as the fastest; built next.
2. **B, hardware decoding with a copy,** through Direct3D 11 on Windows and VA-API on Linux.
3. **A, software decoding,** always.

The order is logged with each file, so real use shows which path runs. If C turns out unreliable on some drivers, it can be switched off per driver without touching the rest. The spike stays until the GPU table is filled in.

**Building C, step 1 of 3 (built): the window draws through Vulkan on Windows.** C needs the window's own device to be a Vulkan one. Left alone, wgpu 30 collects the graphics cards of every system it was built with, sorts them by kind (separate card, built-in, other, virtual, software) and breaks ties in the order Vulkan, Direct3D 12, OpenGL. That usually ends on Vulkan already, but nothing promises it, and a machine whose Vulkan driver only offers a lesser card would end on Direct3D 12. So the editor now asks explicitly (`crates/app/src/graphics.rs`): Vulkan alone first, accepting only a real card that can draw to the window; then Direct3D 12 alone; then wgpu's own pick. A system that reports an error is followed by the next in the same start, because the GUI toolkit returns the error before the window is shown and can be run again. A driver that crashes or freezes instead is caught by a marker file: written before a start with Vulkan, removed after three drawn frames, and if found at the next start the Graphics setting becomes Direct3D 12 (which never loads the Vulkan driver) with a notice. That takes one failed start, so it comes before the updater's own rollback, which waits for two. Settings has Automatic, Vulkan and Direct3D 12; Diagnostics shows what is in use. Linux is unchanged. Step 2 is below; step 3 is creating the Vulkan device ourselves with the video queues, and decoding into it.

**Building C, step 2 of 3 (built): colour is converted on the graphics card.** Until now B and A converted every frame to RGBA and scaled it on the CPU before upload. This step moves that work to the graphics card: the decoder hands over its own YUV planes, copied out of the decoded frame once (interleaved colour from the GPU's copy back, planar from the CPU decoders; 8 bits, colour at half size), the viewer uploads them as textures, and one small shader converts to RGB (BT.601 or BT.709 as the file says, by picture size when it doesn't; limited or full range), scales and turns the picture while drawing. Formats the shader doesn't take (10-bit, 4:2:2, 4:4:4, wide-gamut matrices) still convert on the CPU, now with the file's own matrix and range too: before, the CPU path read everything as BT.601, which put high-definition colours off by up to 32 steps in 255. Step 3 is C: the same shader samples the decoder's images directly, with no copy at all.

What this step costs for one 1440p frame, after decoding, on the 2-thread cloud machine (software renderer, so the upload is an upper bound; the worker's numbers are from an optimised build, the interface thread's from a development build):

| Preview quality | Before: worker | Before: interface thread | After: worker | After: interface thread |
| --- | ---: | ---: | ---: | ---: |
| Full (2560×1440) | 1.4–1.9 ms | about 14 ms | 0.4–0.5 ms | 0.6 ms (1.0 with the driver's copy) |
| Auto, viewer 1100 wide | 2.4–2.5 ms | 0.7–0.9 ms | 0.7–0.8 ms | 0.1 ms (0.2) |
| 1/4 (640×360) | 1.1–1.2 ms | 0.3 ms | 0.9–1.1 ms | under 0.1 ms |

Decoding itself (about 11 ms a frame for this clip in software, see the first table) is unchanged and now dominates. Planes come at the file's size or halved (by averaging, once, twice or three times) to the nearest size that still fills what the viewer asked for, so little is uploaded for a small viewer and the GPU never shrinks a picture by much in one go. Lowering the preview quality no longer saves decoding work for these pictures, so Auto's lowering while playing now only applies to pictures still converted on the CPU.

## Sources

- [FFmpeg's Vulkan hardware context header](https://ffmpeg.org/doxygen/trunk/hwcontext__vulkan_8h_source.html): the fields an app fills to share its own Vulkan device, and the frame synchronisation rules.
- [Notes from integrating FFmpeg's Vulkan decoding into an engine](https://themaister.net/blog/2023/01/05/vulkan-video-shenanigans-ffmpeg-radv-integration-experiments/) (2023): sharing the device, semaphores, layouts and the padding pitfall.
- [A video player's Vulkan decoding guide and its discussion](https://github.com/mpv-player/mpv/discussions/13909) (2023 to 2026): driver versions and problems per vendor.
- [A Rust crate for Vulkan Video decoding into wgpu textures](https://docs.rs/crate/vk-video/0.3.0/source/README.md): H.264 only, NV12 textures.
- [A Rust media library's notes on sharing decoded Direct3D 11 frames into wgpu](https://docs.rs/crate/mediaway/0.1.3/source/docs/wgpu/roadmap.md): the Windows shared-handle route and its open questions.
- [Direct3D 12 video decoding added to FFmpeg](https://lists.mplayerhq.hu/pipermail/ffmpeg-cvslog/2023-December/140085.html) (December 2023).
- [The state of media crates in Rust, July 2026](https://dev.to/yeauty/the-state-of-media-processing-in-rust-2026-what-each-crate-actually-covers-2k2c): binding maintenance status.
