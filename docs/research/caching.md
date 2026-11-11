# Caching

How the editor shows a picture at once when the playhead is dragged, clicked somewhere else or stepped backwards. Build plan step 4A "Caches", and the proxies of "Thumbnails, waveforms and proxies". Getting frames out of files fast is in [decoding.md](decoding.md); this note is about not having to decode when the picture is needed.

## 1. Problem

| Need | Target |
| --- | --- |
| Scrub: show the frame under the pointer | a frame within 50 ms of the cursor stopping (Checkpoint 4A) |
| While the pointer moves | the picture keeps following it; it never freezes on an old frame |
| Jump: a click in the ruler, a marker, the start of a clip | a picture within 50 ms |
| Step one frame back (the left arrow), also held down | as quick as stepping forward |
| Play backwards at normal speed | every frame, in time |
| Memory and disk | inside the budgets from `jobs::Budgets`: Auto is a quarter of the memory (8 GB on a machine with 32) and 20 GB of disk |
| Playing forward | no slower than today |
| The interface thread | never waits for a picture |

**Where we are.** Time from a random jump to the frame on screen:

| Recording | Machine and decoding path | Median ms | Worst ms |
| --- | --- | ---: | ---: |
| H.265 1440p 60, real game recordings | test machine, GPU decoding with a copy (the editor today) | 250 to 300 | 620 to 800 |
| H.265 1440p 60, real game recordings | test machine, the fastest decoding path we can build | about 170 | 450 to 650 |
| H.264 1080p 60 | test machine | about 45 | |
| H.265 1440p 60, synthetic, key frames 2 s apart | 2-thread cloud machine, software | 785 | 1361 |

**Why.** Almost every frame of a recording is stored as the difference from the frames before it. Decoding can only start at a key frame, and game recordings have them far apart: 2 seconds is common, some recorders write one only every 5 to 10 seconds, or less often. Showing one frame means decoding every frame from the key frame before it: 60 frames on average and 120 at worst with 2-second groups at 60 frames a second, 480 at worst with 8-second groups. To do that inside 50 ms a decoder would have to run at 2,400 to 9,600 frames a second at 1440p. None does, so faster decoding alone can't reach the target. The picture has to exist before it is asked for (cached or precomputed), or a different picture has to do for a moment.

**What the editor does today** (`media::VideoDecoder::frame_at`, `engine::Preview`):

- One decoder per file. A target up to 3 s ahead is reached by decoding on; anything else seeks to the key frame before it and decodes forward.
- Only the newest request counts, but a decode that has started is always finished. While the pointer moves, the worker spends a whole jump time on a position the pointer left long ago.
- Nothing is kept. One step back decodes from the key frame again: stepping back through one 2-second group decodes about 7,000 frames to show 120.
- Playing backwards doesn't exist, for the same reason.

## 2. Approaches

### A. Key frame first, then the exact frame

On a jump, decode just the key frame before the target and show it. Then decode on to the exact frame and swap it in. If a frame nearer than the key frame is already in memory, show that instead.

- No memory, no disk, nothing built ahead: it works on a file the moment it is imported.
- The first picture is the wrong frame: on average half a key frame distance too early (1 s with 2-second groups, 4 s with 8-second groups). Good for "roughly where am I", useless for finding a cut.
- Cost: one seek and one frame decoded, and a key frame is the most expensive frame to decode. Needs a second decoder on the file, or the main one loses its place.
- The exact frame arrives no sooner than today.

### B. Frames in memory around the playhead

Keep decoded frames at preview size, found by file, stream, size and timestamp, inside a memory budget. Frames come from three places: everything decoded on the way to a target (it was decoded anyway), a stretch ahead of the playhead in its direction of travel filled while idle, and frames shown before.

- Inside the kept stretch everything is instant: stepping either way, short scrubs, going back and forth around a cut, replaying what was just watched, playing backwards.
- A jump to somewhere new is as slow as today. B needs A, C or D beside it.
- Cost: memory, and copying every decoded frame out of the decoder at preview size instead of only the one shown. With GPU decoding that means bringing every frame back from the GPU.
- Moving backwards has a floor: frames only come out of a decoder forwards, so the whole group of pictures behind the playhead must be decoded before its last frame can be shown. Playing backwards works when decoding is faster than playback and two groups fit in the budget.
- Dropping the frame used longest ago is not enough on its own. Playing backwards, the frames about to be shown are the ones decoded longest ago. The window around the playhead has to be dropped last.
- Format: YUV 4:2:0 at preview size, 1.5 bytes a pixel, against 4 for RGBA. It is what decoders produce, so nothing is converted, and the viewer needs the YUV shader from the decoding note anyway.

### C. A proxy: a small copy that is cheap to jump around in

Build a low-resolution copy of each file in the background (540p here) in which every frame is a key frame, or one in every few. Any frame is then one small frame away. The build plan's proxies with Off / On / Auto are this.

- The only approach that shows the *right* frame, anywhere in the file, in a few milliseconds, whatever the original's key frame distance. Stepping and playing backwards work everywhere too.
- The picture is soft (540p in a 720p to 1440p viewer). When the pointer rests, the exact frame from the original replaces it. With proxy mode On, it stays.
- Cost: every frame of the original is decoded once and encoded once, in the background, and the copy takes disk. It must have exactly the original's frame times, also with a variable frame rate. Until it is built, something else has to cover.
- Needs: a background job per file (the analysis pool, paused while playing), a place in the disk cache keyed by the file's fingerprint, an entry in the library ("Needs proxy"), and export must never read it.
- Encoders in the pinned FFmpeg build: `libx264`, `libx265`, `libsvtav1`, `mjpeg` and other intra-only codecs in software, and the GPU vendors' hardware H.264, H.265 and AV1 encoders. The spike uses `libx264` and `mjpeg`.

### D. A picture index: a small picture every so often

A cheaper cousin of C: one small JPEG (360p here) for every key frame, or every N frames, in one file per recording.

- One picture per key frame is very cheap to build, because only the key frames are decoded, and tiny on disk. It gives A's first picture (the same wrong frame) without A's seek, decode and second decoder.
- One picture every 15 frames is off by 0.12 s on average, but building it decodes every frame, as C does.
- The clip thumbnails in `cache::thumbs` are already a coarse form of this: 160×90, at least 2 s apart, kept in memory.

### Helpers every approach builds on

- **A key frame index.** With the key frame times known up front, the decoder seeks exactly when a key frame lies between where it is and the target, instead of today's "more than 3 s ahead". MP4 lists every frame with a key flag; MKV lists what its writer chose. Reading every packet header finds them exactly, at the cost of reading the file through.
- **Dropping a decode nobody needs.** Check for a newer request between frames. Keep going when the newer target is ahead and no key frame lies in between (the work isn't wasted); otherwise stop. On its own this starves: during a fast drag every decode is dropped and nothing is shown. It only works with a first picture from A, C or D.
- **Showing frames passed on the way.** While decoding towards a far target, put a frame on screen every 30 ms or so. Not the frame asked for, but the picture moves.
- **Leaving out frames nothing depends on** while racing to a target (`skip_frame`). FFmpeg's H.264 and H.265 decoders drop such frames before the GPU decoder is called, so this also works with hardware decoding. Its hardware-only AV1 decoder does not. The picture stays exact, as long as skipping stops just before the target. It only helps when the file has such frames: GPU recorders mostly write none.
- **Leaving out the deblocking filter** (`skip_loop_filter`). Faster in software. GPU decoders filter inside the hardware, so the setting should not reach them (the test machine's "differs" column will tell). The frames come out slightly wrong, so the result can't be shown as the exact frame.
- **Stopping at the target.** Today the decoder decodes one frame past the target to know the target was the last one before it. With frame times known, that frame isn't needed.
- **One frame out per packet in.** Software decoders with frame threads hold back as many frames as they have threads. For one-frame fetches (a proxy frame, a key frame) use one thread, or tell the decoder the stream ended after one packet.
- **Two decoders per file:** one that stays at the playhead (playing and stepping forward cost one frame), one for jumps and for filling the cache. Opening a second one takes as long as the first (see the results).

### Considered and left out

- **A full-resolution copy with every frame a key frame.** Solves everything, but takes about seven times a 540p proxy's disk at the same quality, and far more at a quality worth editing from. Export uses the originals anyway.
- **Asking people to record with closer key frames.** Good advice for the manual; not a fix for files that exist.
- **Restart points inside a group of pictures.** A decoder's state can't be saved and restored through FFmpeg, and starting from an approximate picture makes every following frame drift.
- **Lower-resolution decoding in the decoder itself.** FFmpeg's H.264, H.265 and AV1 decoders don't offer it.
- **Compressing the frames kept in memory.** That is a proxy held in memory: all of its build cost, none of its permanence.
- **Decoding several groups of pictures at once** on spare cores to fill the cache faster. Sound, but it speeds up filling, not the first picture after a jump: one group is decoded front to back. Later, if playing backwards needs it.

## 3. Prototypes

One program in `spikes/caching/` measures all four approaches and the helpers on the same clips. It decodes with FFmpeg 8.1 in software, or on the GPU with frames copied back (the editor's current path).

**Clips**, made by `make-clips.sh`: a moving test pattern with film-grain noise at GPU-recorder bit rates, P frames only, as GPU recorders write them.

| Clip | Why |
| --- | --- |
| H.264 1080p 60, key frames 2 s apart | the most common recording |
| H.265 1440p 60, key frames 2 s apart | Checkpoint 4A's clip |
| H.265 1440p 60, key frames 8 s apart | like the real recordings that jump slowly |
| AV1 1440p 60, key frames 2 s apart | newest codec from the GPU recorder; the encoder keeps its own layered frame order |
| H.264 720p 60, key frames 2 s apart (only with `SMALL=1`) | for slow machines: a file that decodes several times faster than it plays |
| Real recordings | dropped into the `recordings` folder on the test machine (never committed) |

**Measured for each clip:**

- **Jumps:** 100 random targets, the same for every approach. Time to the first picture, how far that picture is from the target, time to the exact frame.
- **Built files:** three proxies (540p H.264 with every frame a key frame; 540p JPEG frames; 540p H.264 with a key frame every 8 frames) and two picture indexes (a 360p JPEG per key frame; one every 15 frames). Build time, size, and the time to fetch one picture.
- **Helpers:** a seek alone, a jump with frames or the deblocking filter left out (and whether the picture is still exact), and what keeping every decoded frame adds per frame.
- **Scrub traces:** synthesised pointer movement, 60 positions a second, the same for every approach:

| Trace | Movement |
| --- | --- |
| slow drag | forward at twice normal speed for 4 s, then rest |
| slow drag back | backward at twice normal speed for 4 s |
| fast drag | 16 s of video a second, there, back and there again in 3 s |
| back and forth | 0.4 s either side of one spot, 1.25 times a second, for 4 s |
| step back | 40 presses of the left arrow, 8 a second, across a key frame |
| play back | every frame backwards at normal speed for 3 s, across a key frame |

A trace is replayed against a model of the preview worker: one worker, newest request wins, as in the editor. The work (seeking, decoding, scaling) is real and timed; waiting is skipped by a simulated clock. For every position the model records when the first picture for it, or for a newer position, was on screen (the **response**), whether that picture was the right frame, and how long after the last position the exact frame arrived (**settled**).

The approaches in the trace tables:

| Name | What it does |
| --- | --- |
| today | the editor's rule: decode on up to 3 s ahead, else seek; never drop a decode |
| cancelling only | the helpers alone: key frame index, dropping stale decodes, frames passed on the way |
| A | the helpers, plus the key frame (or a nearer frame in memory) at once |
| B | the helpers, plus every decoded frame kept (1 GB budget) and 1 s ahead of the playhead filled when idle |
| C first | the helpers, plus the proxy frame at once, then the exact frame |
| C only | proxy frames only, as with proxy mode On |
| B+C, A+B, B+D | combinations (D here is the index with a picture every 15 frames) |

**Limits of the spike.** "On screen" means in memory at preview size (1280×720); uploading to the GPU costs the same for every approach and is left out. One worker does everything in turn, where the editor could fetch the first picture and decode the exact frame on two threads. The proxies are built by the `ffmpeg` program, not through the library.

## 4. Result

**Software decoding on a 2-thread cloud machine** (no GPU; FFmpeg 8.1; 20-second synthetic clips; pictures at 1280×720; times in milliseconds). The machine was shared with other work during the runs. Decoding ran at between the decoding note's speed and half of it, and single numbers move by as much from run to run. What holds is the order of magnitude between approaches. The jumps, built files and helpers come from one run; the traces of the clips with key frames 2 s apart from an earlier, slower one. A fifth clip, H.264 720p, was added for this machine: it decodes several times faster than it plays here, as 1440p does on a desktop GPU, so it shows how the approaches behave when the decoder has headroom.

### Jumps

Time from a jump to the first picture, median / worst of 100 random targets:

| First picture | It is | H.264 720p | H.264 1080p | H.265 1440p | H.265 1440p, key frames 8 s apart | AV1 1440p |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| The exact frame, as today | exact | 106 / 345 | 289 / 999 | 723 / 2560 | 1406 / 7148 | 543 / 1413 |
| A: the key frame | too early | 11 / 41 | 27 / 72 | 52 / 108 | 50 / 80 | 32 / 67 |
| C: proxy, every frame a key frame | the right frame, 540p | 0.6 / 4.8 | 0.5 / 1.0 | 0.6 / 1.1 | 0.6 / 1.1 | 0.5 / 0.9 |
| C: proxy, JPEG frames | the right frame, 540p | 1.2 / 5.5 | 1.1 / 1.6 | 1.2 / 2.7 | 1.1 / 1.9 | 1.0 / 1.5 |
| C: proxy, a key frame every 8 frames | the right frame, 540p | 2.1 / 4.2 | 1.7 / 3.8 | 2.2 / 5.2 | 2.0 / 5.9 | 1.6 / 3.1 |
| D: picture per key frame | too early, as A; 360p | 0.5 / 1.6 | 0.5 / 0.6 | 0.5 / 0.8 | 0.5 / 0.8 | 0.5 / 0.8 |
| D: picture every 15 frames | a little early; 360p | 0.5 / 0.6 | 0.5 / 0.6 | 0.5 / 0.8 | 0.5 / 0.8 | 0.6 / 0.8 |

- **The exact frame** takes a tenth of a second to several seconds here, and nothing but a cache hit changes that. With key frames 8 s apart the worst jump takes 7 s.
- **A** shows a picture in 11 to 52 ms (median), but the wrong one: 0.7 s too early (median) and up to 1.9 s with key frames 2 s apart, 2.8 s and up to 7.8 s with 8 s. In software a 1440p H.265 key frame alone uses the whole 50 ms.
- **C** shows the right frame in 0.5 to 3 ms, whatever the original is. One frame of a 540p file costs almost nothing, also with a key frame only every 8 frames.
- **D** is as quick as C. With one picture per key frame it is off by as much as A; with one every 15 frames by 0.12 s (median), 0.23 s at most.

Seeking by the key frame index instead of the 3-second rule, the two taking turns on each target:

| Exact frame, median / worst | H.264 720p | H.264 1080p | H.265 1440p | H.265 1440p, key frames 8 s apart | AV1 1440p |
| --- | ---: | ---: | ---: | ---: | ---: |
| Today's rule | 106 / 345 | 289 / 999 | 723 / 2560 | 1406 / 7148 | 543 / 1413 |
| By the index | 99 / 275 | 279 / 671 | 650 / 2059 | 1304 / 7134 | 529 / 1218 |

The index takes 3 to 10 % off the median and up to a third off the worst case with key frames 2 s apart, where today's rule decodes through a key frame. It is worth having, and far from enough.

### Files built for C and D

Build speed (times faster than playing) · size (MB per minute of video):

| Built | H.264 720p | H.264 1080p | H.265 1440p | H.265 1440p, key frames 8 s apart | AV1 1440p |
| --- | ---: | ---: | ---: | ---: | ---: |
| C: proxy, every frame a key frame | 5.5× · 58 | 2.3× · 50 | 1.0× · 58 | 1.1× · 58 | 1.6× · 59 |
| C: proxy, JPEG frames | 5.3× · 101 | 3.2× · 79 | 0.9× · 88 | 1.6× · 88 | 1.7× · 88 |
| C: proxy, a key frame every 8 frames | 3.8× · 44 | 2.7× · 31 | 0.9× · 27 | 1.6× · 27 | 1.6× · 29 |
| D: picture per key frame | 110× · 0.4 | 39× · 0.4 | 26× · 0.4 | 76× · 0.1 | 79× · 0.4 |
| D: picture every 15 frames | 8.6× · 3.6 | 3.0× · 3.3 | 1.0× · 3.3 | 1.9× · 3.3 | 2.0× · 3.3 |

- Building a proxy means decoding the whole original once. On this machine that is the limit: 1440p decodes at one to two times real time, so its proxies build at one to two times real time. The test machine decodes several times faster and has about ten times the threads for encoding.
- A proxy takes 27 to 59 MB a minute as H.264 at 540p: 1.6 to 3.5 GB for an hour of recording. JPEG frames take about half as much again as H.264 with every frame a key frame, for no gain. A key frame only every 8 frames roughly halves the size and costs 1 to 2 ms a fetch.
- The index with a picture per key frame builds 26 to 110 times faster than playing even here, because only the key frames are decoded. A picture takes about 13 kB: 0.4 MB a minute with key frames 2 s apart. The dense index decodes everything, like a proxy, and only saves disk.

### Helpers

A jump (median / worst of 20, the ways taking turns on each target), decoding every frame or leaving work out. "Differs" is the mean difference in brightness from the exact picture, out of 255.

| Clip | Every frame | Leaving out frames nothing depends on | Frames it decoded | Without the deblocking filter | Differs |
| --- | ---: | ---: | ---: | ---: | ---: |
| H.264 720p | 163 / 281 | 140 / 276 | all 1099 | 129 / 233 | 1.7 |
| H.264 1080p | 320 / 751 | 359 / 648 | all 1099 | 297 / 583 | 1.5 |
| H.265 1440p | 595 / 1857 | 616 / 2000 | all 1099 | 624 / 1743 | 1.5 |
| H.265 1440p, key frames 8 s apart | 1588 / 4061 | 1584 / 4123 | all 4219 | 1403 / 3661 | 2.9 |
| AV1 1440p | 611 / 1099 | 596 / 1088 | all 1099 | 610 / 1008 | 0 |
| H.265 1440p with B frames (the decoding spike's clip) | 684 / 1230 | 521 / 893 | 506 of 968 | 666 / 1206 | 1.0 |

- **Leaving out frames nothing depends on** found nothing to leave out in the recorder-like clips: every frame is used by the next. With B frames it skipped half the frames and the jump was a quarter faster, with an identical picture. Free where it applies; game recordings mostly give it nothing.
- **Without the deblocking filter** H.264 jumps were 7 to 21 % faster, H.265 jumps between nothing and 12 %, and the AV1 decoder ignores the setting. The picture is no longer the exact frame. Not worth building.
- **A seek alone** takes under 0.1 ms in MP4 and 1 ms in MKV. All of a jump's time is decoding.
- **Opening a second decoder** took 1 to 80 ms for the MP4 clips and 225 ms for the AV1 clip in MKV, in software. A decoder for jumps should be opened once and kept, not opened per jump.
- **Key frames:** the container listed every key frame the packets have, in all clips. Reading the packet headers of 20 s took 22 to 71 ms with the file in the system's cache.

Decoding 120 frames in a row, milliseconds per frame, without and with copying each frame out at 1280×720 for the cache:

| Clip | Decoding only | And keeping it as YUV 4:2:0 | And keeping it as RGBA |
| --- | ---: | ---: | ---: |
| H.264 720p | 2.3 | 2.4 | 2.7 |
| H.264 1080p | 6.0 | 7.2 | 8.8 |
| H.265 1440p | 14.9 | 13.7 | 14.9 |
| H.265 1440p, key frames 8 s apart | 9.1 | 9.7 | 10.1 |
| AV1 1440p | 11.0 | 11.4 | 12.2 |

Keeping every frame costs at most about 1 ms a frame in software, less than the noise between runs on the 1440p clips: the copy is made while other threads decode. RGBA costs up to 1.6 ms more and takes 2.7 times the memory (3.7 MB a picture against 1.4 MB). With GPU decoding every kept frame also has to come back from the GPU; that is for the test machine to measure.

### Scrub traces

Share of playhead positions that got a picture within 50 ms · median response in ms.

The 720p clip, where decoding has headroom:

| Approach | slow drag | slow drag back | fast drag | back and forth | step back | play back |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| today | 93 % · 7.3 | 2 % · 276 | 2 % · 395 | 7 % · 169 | 28 % · 383 | 4 % · 240 |
| cancelling only | 92 % · 7.4 | 30 % · 96 | 16 % · 134 | 64 % · 29 | 22 % · 103 | 37 % · 69 |
| A | 90 % · 5.2 | 34 % · 77 | 36 % · 68 | 61 % · 32 | 42 % · 93 | 39 % · 68 |
| B | 95 % · 0.0 | 93 % · 0.8 | 46 % · 52 | 94 % · 1.4 | 100 % · 0.0 | 94 % · 0.0 |
| A+B | 99 % · 0.0 | 100 % · 0.6 | 80 % · 17 | 98 % · 0.0 | 100 % · 0.0 | 100 % · 0.0 |
| C first | 100 % · 0.8 | 100 % · 3.0 | 99 % · 4.6 | 100 % · 1.7 | 100 % · 2.0 | 100 % · 3.1 |
| B+C | 100 % · 0.0 | 100 % · 0.2 | 100 % · 3.3 | 100 % · 2.0 | 100 % · 0.0 | 100 % · 0.0 |
| C only | 100 % · 0.6 | 100 % · 0.6 | 100 % · 0.5 | 100 % · 0.6 | 100 % · 0.5 | 100 % · 0.5 |
| B+D | 92 % · 0.0 | 99 % · 1.0 | 100 % · 3.4 | 99 % · 0.5 | 100 % · 0.0 | 97 % · 0.0 |

H.265 1440p with key frames 2 s apart, which this machine decodes no faster than it plays:

| Approach | slow drag | slow drag back | fast drag | back and forth | step back | play back |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| today | 0 % · 2734 | 0 % · 1526 | 0 % · 2247 | 0 % · 1827 | 0 % · 2097 | 0 % · 861 |
| cancelling only | 7 % · 96 | 9 % · 129 | 4 % · 113 | 13 % · 93 | 8 % · 421 | 10 % · 157 |
| A | 4 % · 115 | 8 % · 160 | 0 % · 137 | 9 % · 110 | 10 % · 424 | 11 % · 143 |
| B | 7 % · 93 | 36 % · 81 | 8 % · 99 | 36 % · 73 | 78 % · 19 | 51 % · 49 |
| A+B | 59 % · 38 | 56 % · 38 | 19 % · 96 | 41 % · 65 | 85 % · 19 | 66 % · 21 |
| C first | 94 % · 13 | 83 % · 18 | 66 % · 34 | 84 % · 18 | 100 % · 8.6 | 89 % · 15 |
| B+C | 93 % · 14 | 95 % · 13 | 73 % · 29 | 86 % · 17 | 98 % · 15 | 96 % · 11 |
| C only | 100 % · 0.6 | 100 % · 0.5 | 100 % · 0.6 | 100 % · 0.5 | 100 % · 0.6 | 100 % · 0.8 |
| B+D | 38 % · 66 | 60 % · 37 | 62 % · 38 | 43 % · 59 | 80 % · 16 | 57 % · 33 |

H.265 1440p with key frames 8 s apart (a quieter run: decoding at almost twice the speed of playing):

| Approach | slow drag | slow drag back | fast drag | back and forth | step back | play back |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| today | 0 % · 4130 | 0 % · 5573 | 0 % · 4771 | 0 % · 1504 | 0 % · 6095 | 0 % · 6158 |
| cancelling only | 1 % · 578 | 5 % · 696 | 1 % · 394 | 5 % · 339 | 12 % · 1577 | 7 % · 774 |
| A | 0 % · 709 | 3 % · 412 | 1 % · 285 | 6 % · 316 | 5 % · 1491 | 4 % · 942 |
| B | 2 % · 472 | 31 % · 339 | 8 % · 203 | 13 % · 242 | 42 % · 297 | 23 % · 528 |
| A+B | 95 % · 6.7 | 72 % · 9.8 | 56 % · 35 | 47 % · 57 | 48 % · 171 | 31 % · 590 |
| C first | 99 % · 6.4 | 98 % · 7.3 | 97 % · 7.9 | 99 % · 6.0 | 100 % · 3.8 | 99 % · 7.3 |
| B+C | 99 % · 6.3 | 99 % · 6.7 | 96 % · 7.9 | 95 % · 7.7 | 100 % · 5.7 | 99 % · 7.1 |
| C only | 100 % · 0.6 | 100 % · 0.6 | 100 % · 0.6 | 100 % · 0.6 | 100 % · 0.5 | 100 % · 0.6 |
| B+D | 39 % · 62 | 57 % · 38 | 98 % · 9.2 | 44 % · 58 | 42 % · 242 | 37 % · 90 |

Share of positions that B answered with the exact frame straight from memory, and the most memory it held (1 GB budget):

| Clip | slow drag | slow drag back | fast drag | back and forth | step back | play back | Most memory MB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| H.264 720p | 94 % | 91 % | 53 % | 93 % | 98 % | 92 % | 860 |
| H.264 1080p | 0 % | 87 % | 35 % | 85 % | 95 % | 68 % | 672 |
| H.265 1440p, key frames 2 s apart | 0 % | 44 % | 7 % | 90 % | 80 % | 49 % | 523 |
| H.265 1440p, key frames 8 s apart | 0 % | 29 % | 62 % | 80 % | 38 % | 33 % | 666 |

Time from the last position of a trace to its exact frame on the 720p clip: 0 ms with B wherever the frame was kept; 119 to 316 ms after the fast drag with every approach that shows the original (one jump); 0.5 ms with C only, which never shows it.

- **Today** the worker is busy with positions the pointer has left. Stepping back costs 383 ms a press on the easy clip (median) and 2 s on 1440p. The worst response is a whole jump or two: 0.25 to 0.7 s on the easy clip, 2 to 4 s on 1440p.
- **Cancelling alone** cuts the median to about a third wherever today is slow, but a picture within 50 ms stays the exception, and frames passed on the way stand in for some exact ones.
- **A** adds little in the traces. Its key frame takes 11 to 52 ms here and is the wrong frame.
- **B** answers 91 to 98 % of the positions from memory when decoding has headroom, in everything but the fast drag. The misses are the first visit to a group of pictures: its worst response is 38 to 268 ms, where today's is 250 to 723 ms. In the fast drag half the positions are new and B has nothing for them.
- **B without headroom** (1080p and 1440p here) still serves stepping back and going back and forth from memory (80 to 95 %), because those stay inside one or two groups. In a drag the decoder never catches up with the pointer and B has nothing to keep. This is this machine's limit, not B's; the test machine has to show it for 1440p.
- **C** puts the right frame on screen within 50 ms for 97 to 100 % of the positions with headroom, and for 66 to 100 % without. The misses come from the model's single worker, which can't fetch a proxy frame while it is inside a decode step of the original; two threads remove them.
- **C only** is 100 % within 50 ms on every clip and trace, at 0.5 to 0.8 ms, because the original is never touched.
- **Key frames 8 s apart** make everything that decodes four times worse: today's median response is 1.5 to 6 s. B drops to 38 % from memory when stepping back and 33 % when playing backwards, because crossing a key frame means decoding the whole 8-second group before it first (seconds here). C doesn't notice: 97 to 100 % within 50 ms.
- **B+C** is the combination to build: C's response, and the exact frame from memory in 84 to 98 % of the positions outside the fast drag.
- **B+D** shows that a dense picture index gives a first picture as quickly as a proxy, but not the right one: only 19 % right frames in the fast drag against C's 100 %.

### Recommendation

No single approach is enough. Build them in this order; each step is useful on its own.

**1. The helpers, in the preview worker and the decoder.** No memory, no disk.

- A key frame index per file: taken from the container when the file is opened, checked against the packets by the pass that already reads the whole file for the waveform.
- Seek by the index. Drop a decode a newer position can't use; keep going when it can. Show frames passed on the way.
- Stop at the target instead of one frame after it.

This takes the worst waits out (today a stale decode holds the worker for a whole jump) but shows nothing new within 50 ms.

*Built (November 2026), except two parts.* The decoder reads the container's list of key frames when a file is opened (`media::jump`) and jumps when one lies between where it is and where it is going; the preview worker asks between frames what is wanted by now, gives up a decode the newer request can't use, stops early at a newer moment it reaches first, and finishes one that is on the way (`VideoDecoder::picture_at_while`, `engine::preview`). On a ten-minute 720p recording with key frames 2 s apart, decoded on the CPU of the cloud machine (one jump to the slow end of a group takes about 120 ms there):

| From a click to its picture, median | Before | Now |
| --- | ---: | ---: |
| A click while an earlier jump is a third done | 208 ms | 122 ms |
| A click while an earlier jump is two thirds done | 174 ms | 125 ms |
| 2.9 s ahead, past a key frame | 180 ms | 94 ms |
| One click, nothing else going on | 121 ms | 117 ms |

A second click now always costs one jump, never the rest of the first one as well. Left out: checking the container's list against the packets (the list is used as it is, with a fixed distance as the rule where it has holes), and stopping at the target instead of one frame after it, which needs each frame's length and gains one frame per jump.

*On the test machine the first version of this did not help, and dragging got worse* (11 November 2026). Three corrections followed:

- It gave up a decode for every newer request it could not serve. While the playhead is dragged such requests never stop coming, so no decode was ever finished and the picture froze until the pointer rested. Now the decode started after one was given up is always finished: a second click still does not wait for the first, and a steady stream of requests gets pictures as often as it did before.
- It jumped to any key frame on the way, however near. A jump is not free (the decoder is emptied, the file read elsewhere), and on a GPU decoder it may cost more than the frames it saves. Now a jump ahead has to save at least a second of decoding.
- The time in the viewer's corner is one picture's own decoding time, so the 500 to 800 ms seen there were never two jumps in a row: something inside one jump is slow on that machine that is not slow here. To find it, the decoder now keeps where each picture's time went (`media::Work`: the jump itself, the first frame out, the frames after it) and Diagnostics lists the last pictures that took more than 50 ms ("Pictures that took long").

The same test found that stepping with the arrow keys showed some frames a step late. Recordings that count time in milliseconds store 60 frames a second at 0, 17, 33, 50, 67, 83 ms; the frame stored at 17 was not taken for the frame on screen at 16.667 ms, so two of every six needed a second press, and stepping back onto one cost a whole jump and showed the frame before it. A frame's stored time is now allowed to be half a step of the stream's clock off (`VideoDecoder`'s `tick`).

**2. B: frames in memory.** This is the build plan's "RAM frame cache ahead of the playhead in the play direction".

- YUV 4:2:0 at the size the viewer asks for, found by file, stream, size and timestamp. A frame lasts until the next frame's timestamp, so variable frame rates need nothing special.
- Fed by everything decoded on the way to a target, and by an idle fill: 1 s ahead in the direction of travel; moving backwards, the group of pictures behind the playhead.
- Budget from `jobs::Budgets` (`ram_cache_mb`). The frame used longest ago goes first, the window around the playhead last.
- The fill uses a second decoder, so the one at the playhead keeps its place.

It makes stepping, short scrubs, going back and forth, replaying and playing backwards instant inside the kept stretch. It does nothing for a jump to somewhere new.

*Built first for frames on the graphics card (November 2026), not for memory.* The timings from the test machine (`Pictures that took long` in Diagnostics) showed that a jump there has no slow part of its own: the jump takes 0 to 4 ms and the first frame is out within 4 ms; all the time is the frames between the key frame and the target, at about 3.5 ms each (34 frames in 115 ms, 103 in 381 ms). Going forwards needs the next few frames; going back by one frame needs all of them again. So what helps most is keeping those frames, and on that machine they are decoded on the window's graphics device, where keeping one is a copy on the card that the picture shown needs anyway.

- The decoder hands over every frame it passes on the way to a picture, the one shown included, each with the time the next frame starts (`VideoDecoder::picture_at_while`'s `keep`). A frame is on screen from its own time until then.
- `engine::frames::FrameCache` keeps them by file and time, within a budget in bytes: a quarter of the card's own memory, between 256 MB and 2 GB (two seconds of 1440p at 60 frames a second is about 1 GB). The frame used longest ago goes first.
- The preview worker looks there before it touches a decoder. A kept frame is shown in well under a millisecond; the viewer's corner says "kept on the graphics card".

Not built yet: the idle fill (the group of pictures behind the playhead, so stepping back across a key frame doesn't decode that group at that moment), the same for the two ways of decoding that end in memory (planes at the viewer's size, shared between the cache and the viewer), and marking kept stretches in the timeline's cache bar.

**3. C: proxies**, the build plan's Off / On / Auto. The only way to the right frame within 50 ms anywhere in a file.

- 540p H.264 with a key frame every 8 frames (half the disk of every frame a key frame, 1 to 2 ms more per fetch), with exactly the original's frame times, in the disk cache under the file's fingerprint.
- Built on the analysis pool, paused while playing, newest files first. Decoding the original is most of the cost, so it uses the GPU decoder.
- **Auto** builds a proxy for a file when a jump in it can't make 50 ms: judged from the file's longest key frame distance and a decode timed at import, not from the codec's name. On this evidence that is every 1440p recording.
- While scrubbing, the proxy frame goes on screen first and the exact frame replaces it when the pointer rests. With proxy mode On the exact frame is skipped.

**4. A first picture for files without a proxy yet.** A: seek, decode the key frame before the target, show it, then carry on to the exact frame. On the test machine this takes 4 to 6 ms on the GPU and 32 to 42 ms in software, so nothing needs storing; the picture per key frame (D) stays unbuilt.

### How this meets 50 ms

| Situation | On screen within 50 ms | The exact frame |
| --- | --- | --- |
| Stepping, a short scrub, back and forth, playing backwards, inside the kept stretch | the exact frame, from memory (0 to 2 ms) | the same |
| A drag or a jump anywhere, proxy built | the right frame at 540p, from the proxy (0.5 to 3 ms) | from memory if kept; else one jump later (80 to 140 ms at the median, 210 to 300 ms at worst, on the test machine) |
| The same, no proxy yet | the key frame before the target, decoded on the spot (4 to 6 ms on the GPU); up to one key frame distance early | one jump later |
| Proxy mode On | the right frame at 540p | not shown |

The exact, full-resolution frame at a place never visited can't be had in 50 ms from a file with key frames seconds apart, by any approach here. The checkpoint asks for "a frame within 50 ms": the proxy frame is the right moment at a lower resolution, and it sharpens when the pointer rests.

### Budgets

Memory for B, at 60 frames a second, YUV 4:2:0:

| Picture size | MB per frame | MB per second of video | Seconds of video in 8 GB |
| --- | ---: | ---: | ---: |
| 1280×720 | 1.4 | 83 | 103 |
| 1920×1080 | 3.1 | 187 | 46 |
| 2560×1440 | 5.5 | 332 | 26 |
| 2560×1440 as RGBA, for comparison | 14.7 | 885 | 10 |

- The least that is useful is two groups of pictures: 0.3 GB at 720p with key frames 2 s apart, 1.3 GB with 8 s. The Auto budget (a quarter of memory) is far above that. A manual budget below it still works forwards; playing backwards falls back to the proxy.
- The spike's traces peaked at 0.3 to 0.9 GB with a 1 GB budget.

Disk for C and D:

| File | MB per minute | GB per hour of recording | Hours in the 20 GB Auto budget |
| --- | ---: | ---: | ---: |
| Proxy, a key frame every 8 frames | 27 to 44 | 1.6 to 2.6 | 8 to 12 |
| Proxy, every frame a key frame | 50 to 59 | 3.0 to 3.5 | about 6 |
| Picture per key frame, key frames 2 s apart | 0.4 | 0.02 | |

Proxies of files not used for longest are dropped first; they can always be built again.

### The three decoding paths

One cache with two stores. The keys and the rules are the same; only where the pixels live differs.

| Decoding path | A decoded frame is | Kept as | Keeping costs |
| --- | --- | --- | --- |
| Software | YUV planes in memory | a scaled copy in memory | one scale, done while other threads decode |
| GPU with a copy (the editor today) | a surface on the GPU | a scaled copy in memory | bringing *every* frame back from the GPU, not just the one shown. To be measured on the test machine |
| Zero-copy on the GPU | an image in the decoder's own pool | a scaled copy in a texture the cache owns | one blit on the GPU, and video memory instead of memory |

- The decoder's own images can't be the cache. Its pool is small and fixed, and holding its images stalls it. The cache always copies out.
- The memory store comes first: it serves all three paths and is all the current path can use. The GPU store arrives with the zero-copy path, with its own budget (video memory), and is the build plan's "GPU texture cache". Until then that item is small: keep the textures of the frame on screen and its neighbours, so a step doesn't upload again.
- Proxies are small enough to decode in software on one thread on every path.
- The disk render cache keyed by input hash is for rendered effects, not for source frames. Nothing here changes it.

### Playback

- Playing forward reads through the cache. The fill that keeps 1 s ahead while scrubbing keeps a shorter stretch ahead while playing, at playback priority; that is the read-ahead the audio clock needs anyway.
- Frames stay cached after they are played. "Pause returns to where playback started" then replays from memory: about 100 s at 720p in 8 GB.
- The preview size is part of the key. When Auto quality lowers the size, frames of the other size are dropped first.
- Playing backwards: from memory when decoding is faster than playback and two groups of pictures fit; else from the proxy, where every stretch of 8 frames stands alone.
- Proxy builds are background jobs and pause while playing.

### What the container can't tell us

- **Where the key frames are, for certain.** MP4 lists every frame with a key flag. MKV lists what its writer chose to: usually every video key frame, sometimes fewer, and nothing at all when a recording was cut off before the list was written. The spike's clips had every key frame listed. Reading every packet header is exact but reads the whole file.
- **Whether a key frame is a clean start.** In H.265 a key frame can be followed by frames that need pictures from before it; after a seek those are dropped. Some recorders refresh the picture in stripes and never write a key frame after the first: every jump in such a file decodes from the start. Only a proxy helps there, and Auto must notice it from the key frame distance.
- **Which frames nothing depends on.** Not in the container; each frame's header has to be read. So leaving frames out is something to try, not to plan on.
- **What a frame costs to decode.** Not known until tried. Auto should time a decode at import.
- **Frame times.** With a variable frame rate the number of frames and their times are only known after reading every packet header. The cache is keyed by timestamp, never by frame number.
- **Display order.** With B frames, files store frames in decoding order and some containers list decoding times, so an index made from the container needs the same care as the decoder's own timestamps.

### In the existing code, whatever is built

- `engine::Preview` couldn't drop a decode. (Done: it asks between frames, see step 1.)
- (Done, see step 1: it now goes by the container's key frames.) `VideoDecoder::frame_at` sought only when the target was behind or more than 3 s ahead. With key frames 2 s apart it decodes through a key frame it could have jumped to. With key frames further apart, a target more than 3 s ahead in the same group makes it seek back and decode again what it had already passed.
- `frame_at` decodes one frame past the target before it answers.
- `cache::thumbs` asks for a frame every 2 s or more, wherever the key frames are. When they don't line up, every thumbnail decodes up from the key frame before it. Taking the key frames themselves is what the "picture per key frame" row above does, 26 to 110 times faster than playing even in software.
- `app::Viewer::poll` converts every picture pixel by pixel on the interface thread before it is uploaded. Timed on its own on this machine, the conversion and one copy take about 4 ms for a 1440p picture and 2 ms at 1080p: at or above the 2 ms rule.

### On the test machine

Run on 11 November 2026 with the "Caching spike" workflow's Windows build: a current desktop GPU, a 24-thread CPU, decoding through Direct3D 11 with a copy (the editor's path at the time), pictures fitted into 1280×720. The first 20 s of each test clip and the first 60 s of two real 1440p H.265 recordings. The spike picks FFmpeg's default AV1 decoder, which is software only, so its AV1 column is a software column; the editor itself decodes AV1 on the GPU.

| Median / worst, ms | H.264 1080p | H.265 1440p | H.265 1440p, key frames 8 s apart | Real 1440p recordings | AV1 1440p, software |
| --- | ---: | ---: | ---: | ---: | ---: |
| Jump to the exact frame, today | 39 / 121 | 94 / 288 | 297 / 832 | 140 / 290 | 377 / 949 |
| The same with a key frame index | 37 / 96 | 80 / 213 | 288 / 902 | 131 / 297 | 358 / 677 |
| A: seek and one key frame | 4.1 / 14 | 5.6 / 15 | 5.7 / 14 | 5.3 / 15 | 32 / 42 |
| C: a frame from the proxy (a key frame every 8 frames) | 1.2 / 2.4 | 1.2 / 2.4 | 1.2 / 2.4 | 2.0 / 3.5 | 1.1 / 2.2 |
| Decoding one frame in a row | 0.48 | 1.09 | 1.09 | 1.05 | 7.01 |
| and keeping it at 720p, YUV 4:2:0 | 2.29 | 3.75 | 3.74 | 4.2 | 7.29 |
| Proxy build, times faster than playing | 17.5 | 8.9 | 8.9 | 9.4 | 4.9 |
| Proxy size, MB per minute | 31 | 27 | 27 | 31 | 29 |

The traces, for the files decoded on the GPU (per cent of positions answered within 50 ms, then the worst response in ms):

| Trace | Today | Cancelling only | B memory | B+C |
| --- | ---: | ---: | ---: | ---: |
| Slow drag back, H.265 1440p | 2 %, 418 | 14 %, 320 | 98 %, 134 | 100 %, 13 |
| Fast drag, H.265 1440p | 2 %, 491 | 8 %, 298 | 54 %, 242 | 100 %, 33 |
| Step back, H.265 1440p | 30 %, 467 | 30 %, 238 | 100 %, 34 | 100 %, 1.9 |
| Play backwards, H.265 1440p | 4 %, 447 | 17 %, 234 | 97 %, 146 | 100 %, 14 |
| Play backwards, key frames 8 s apart | 2 %, 1657 | 10 %, 811 | 92 %, 196 | 100 %, 13 |
| Fast drag, real recordings | 4 %, 438 | 9 %, 298 | 57 %, 245 | 100 %, 34 |
| Play backwards, real recordings | 4 %, 449 | 20 %, 238 | 93 %, 176 | 100 %, 13 |

What this settles:

- **A:** a seek and one key frame take 4 to 6 ms on the GPU and never more than 15 ms; 32 to 42 ms even in software. A can stand in for the picture index, so step 4 stores nothing. On its own A is not an answer: its picture is 0.7 to 1.2 s early at the median with key frames 2 s apart, and in the fast drag it shows the right frame in about one position in ten.
- **B:** bringing every frame back from the GPU and scaling it costs 2 to 3 ms a frame, two to three times the decode itself. That still fills the cache four times faster than playback. Inside the kept stretch 90 to 98 % of the positions come from memory; the misses are first visits (130 to 600 ms alone). The zero-copy path removes most of this cost, which is the case for its GPU store later.
- **C:** proxies build 9 times faster than playing at 1440p (an hour of recording in under 7 minutes, about 1.9 GB), 17 times at 1080p. Fetches stay at 1 to 4 ms.
- **B+C** answers within 50 ms in every position of every trace on the files decoded on the GPU, with a worst response of 1 to 34 ms, and shows the exact frame from memory in 83 to 98 % of them when going back over a stretch (stepping, dragging back, back and forth, playing backwards). With AV1 in software it is 96 to 100 % and 61 ms at worst.
- **Helpers:** dropping a stale decode is the largest gain that costs nothing: it halves the median wait in every moving trace (step back on the real recordings 256 to 82 ms; playing backwards with key frames 8 s apart 973 to 249 ms). A key frame index takes 5 to 15 % off a jump. The GPU decoder decodes every frame whatever it is asked to leave out, and turning the deblocking filter off changes nothing: neither is worth building.
- **Real recordings:** key frames exactly 2 s apart, every one listed by the container; finding them by reading the packets takes 12 to 41 ms for a minute of video.
- **Playing backwards** at 1440p with B alone: 55 to 58 pictures a second of 60, 93 to 97 % within 50 ms; with the proxy behind it, all of them.

The recommendation stands, in the same order, with step 4 reduced to "show the key frame first".

## Sources

- [A media player's manual](https://mpv.io/manual/stable/): precise seeks decode from the previous key frame; dropping frames before the seek target; backward playback by decoding forward in steps and buffering, and why that is fragile with hardware decoding.
- [A scrubbing change in an open-source editor](https://archive.blender.org/lists/bf-blender-cvs/2021-February/154053.html) (2021): key frames only while scrubbing, the exact frame on the second request, proxies for files with long key frame distances.
- FFmpeg's decoders, for which frames `skip_frame` leaves out and where: [H.264](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavcodec/h264dec.c), [H.265](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavcodec/hevc/hevcdec.c), [AV1 through the GPU](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavcodec/av1dec.c).
- [Advice on FFmpeg's user list about decoding for scrubbing](https://ffmpeg.org/pipermail/libav-user/2015-June/008233.html) (2015): skipping the loop filter, and not decoding every frame while scrubbing.
- [decoding.md](decoding.md): the decoding paths and the jump times this note starts from.
