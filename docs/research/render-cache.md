# Render cache

The disk render cache of the build plan's caching step (4A), and the region's pre-render (4B) that fills it.

## Problem

A stretch with many layers, effects, a nested timeline or a heavy recording can take longer to put together than a frame lasts. Then it stutters however fast decoding is. Pre-rendering puts such a stretch together once, ahead of time, and keeps the frames on disk. Playing it again only reads a picture per frame.

What it must do:

- **Hold up while playing.** Reading and decoding a kept frame must take well under a frame's time (16.7 ms at 60 fps) on one thread, while the viewer's other work goes on.
- **Not fill the disk.** A minute of 60 fps must take hundreds of megabytes, not tens of gigabytes. It gets its own budget, and what was used longest ago goes first.
- **Never show a stale frame.** Any change to what a frame shows (a clip's colour, an effect, a title's words, a moved layer) must miss the cache. A change elsewhere on the timeline must not throw away frames it doesn't touch.
- **Be exact enough for the viewer.** Export never reads it, as it never reads proxies.

## Approaches

1. **Frames as raw pictures**, compressed losslessly with a fast general compressor (LZ4, Zstandard), one file per frame or packed together.
2. **Stretches as video:** the frames encoded as a video file per stretch (H.264 with every frame a key frame or a key frame every few frames, or a lossless codec such as FFV1). A frame index beside each file says which frame is which.
3. **Frames as JPEG pictures:** each frame a picture of its own, packed one after another in a file, with an index of where each is.

Wherever the pixels go, each frame is kept by a hash of what makes its picture (its description: every layer's file, moment, place, colour, effects and blending, titles, nested timelines, adjustment layers), never by its place on the timeline. So:

- a frame moved along with its clip is still found;
- an edit misses only the frames it changes.

## Prototypes

`spikes/render-cache/` makes frames that look like a rendered game recording: a moving pattern with film grain, a facecam box and a dark title band (`make-frames.sh`). It then times each way of keeping them. `src/main.rs` covers the raw and compressed pictures and JPEG through the `image` crate (the one the app uses for clip pictures). `video.sh` covers the video codecs and JPEG through FFmpeg, one thread.

The test machine was the cloud machine: 2 cores, a slow disk, no graphics card. Times are per frame.

### 2560×1440, 30 frames

| Way | MB a frame | Pack (ms) | Write (ms) | Read and unpack (ms) |
| --- | --- | --- | --- | --- |
| Raw RGBA | 14.75 | 2.0 | 17.4 | 6.5 |
| Raw YUV 4:2:0 | 5.53 | 16.1 | 5.5 | 5.2 |
| LZ4, RGBA | 10.62 | 38.2 | 11.4 | 11.7 |
| LZ4, YUV 4:2:0 | 5.20 | 23.0 | 5.3 | 3.7 |
| Zstandard 1, YUV 4:2:0 | 4.07 | 27.0 | 4.5 | 9.7 |
| Zstandard 3, YUV 4:2:0 | 3.60 | 48.3 | 4.6 | 7.8 |
| H.264, every frame a key frame, CRF 12 | 0.96 | 61 | | 92 (1 thread) |
| H.264, a key frame every 8, CRF 14 | 0.64 | 68 | | 68 (1 thread) |
| FFV1 (lossless) | 2.64 | 52 | | 101 (1 thread) |
| JPEG, quality 3 (FFmpeg) | 0.43 | 17.6 | | 10.4 (1 thread) |

### 1280×720, 60 frames

| Way | MB a frame | Pack (ms) | Read and unpack (ms) |
| --- | --- | --- | --- |
| Raw YUV 4:2:0 | 1.38 | 3.6 | 1.6 |
| LZ4, YUV 4:2:0 | 1.17 | 6.0 | 2.0 |
| Zstandard 1, YUV 4:2:0 | 0.93 | 6.9 | 2.5 |
| H.264, every frame a key frame, CRF 12 | 0.22 | 15 | 20 (1 thread) |
| H.264, a key frame every 8, CRF 14 | 0.13 | 16 | 12.5 (1 thread) |
| FFV1 (lossless) | 0.60 | 12 | 24 (1 thread) |
| JPEG, quality 3 (FFmpeg) | 0.10 | 4.1 | 2.8 (1 thread) |
| JPEG, quality 90 (`image` crate) | 0.20 | 19.1 | 5.2 |

Film grain is noise, and no lossless compressor gains much on noise. Even the best lossless way (Zstandard) keeps about 0.9 MB a frame at 720p, which is 3.3 GB a minute at 60 fps. Raw pictures are worse.

Video is small but slow to read on one thread: 12 to 20 ms a frame at 720p. A key frame every few frames also means a frame in the middle needs the ones before it.

JPEG is the smallest that stands alone, and it is fast both ways. At 720p, quality 90 through the `image` crate takes 0.2 MB a frame (720 MB a minute at 60 fps), and reading one back takes 5 ms on this slow machine. With grain, JPEG and H.264 come out at the same picture quality (PSNR about 33 dB for both at the sizes above).

## Result

**JPEG frames, packed, at the viewer's size, keyed by a hash of the frame's description** (`cache::render`, `engine::export::prerender`):

- **One pack per pre-render.** Its frames are written as JPEGs one after another (`<n>.frames`), with an index of where each one is (`<n>.index`). The index is written whole and put in place, again every 60 frames while the pre-render runs, so what was done survives a crash.
- **Frames at most 1920×1080**, in the timeline's shape and never larger than the timeline. That is enough for the viewer, and a quarter of a 1440p frame's disk.
- **Quality 90 with the `image` crate.** The app already uses it for clip pictures, so there is no new dependency. Compressing is the slowest step (about 40 ms a 1080p frame here), but it runs while pre-rendering, not while playing.
- **The key** is FNV-1a over 128 bits of the frame description, the fingerprints of its files and the app's version. It is the same on every machine and in every compiler version, as a key kept on disk must be. A new version of the app never shows frames an older one put together.
- **Budget:** 20 GB, the packs used longest ago go first. Settings › Cache clears them.
- **The viewer** looks a frame up before decoding anything. A frame found there is drawn as one picture filling the frame, so it shows as it would and goes through the same viewer path. The export never reads the cache, and a pre-render never reads frames pre-rendered before.

Not chosen:

- **Video stretches.** Too slow to read on one thread at 1080p on the slower machine, and a stretch is all or nothing after an edit.
- **Lossless pictures.** Several gigabytes a minute for game footage.
- **FFmpeg's JPEG encoder** is faster than the `image` crate's. It would be the next step if compressing turned out to be the slow part of pre-rendering on the owner's machines.
