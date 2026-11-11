# engine

Ties the parts together: what is on the timeline at a moment, and the workers that turn it into pictures (and later sound and exported files). The interface only sends requests and reads results back, so slow media can never freeze a window.

## Status

Phase 4A in progress: the picture at the playhead, the timeline's sound, and playback with the sound as the clock. Caching comes next; export in Phase 6.

## Files

| File | What it holds |
|---|---|
| `src/silence.rs` | `find`: the quiet stretches of a sound track from its waveform peaks (gaps count as silent), with a threshold, a minimum length and a little kept at each edge |
| `src/frames.rs` | `FrameCache`: frames kept after they were decoded, found by file and moment (a frame is on screen from its own time until the next one's), within a budget in bytes, the one used longest ago going first. `budget_on_card`: how much to keep on a graphics card of a given size. The preview worker keeps the frames decoded on the window's device here and looks here before decoding |
| `src/thumbnail.rs` | The project's picture for the launcher: one frame of the timeline (at the playhead, or the first clip's), fitted into 480×270 on black, written as `<file>.thumbnail.png`; `save_frame` writes the frame at a time at its file's full size |
| `src/timeline.rs` | `video_at`: the top visible video clip at a time and where in its file that is |
| `src/preview.rs` | `Preview`: a worker thread that decodes the picture at the playhead. Only the newest request counts (scrubbing never queues), also while a picture is being decoded: a decode the newer request can't use is given up between frames, one that is on the way to it carries on, and the decode after one that was given up is always finished, so dragging still gets pictures; decoders stay open for the four files used last; each file is decoded the fastest way that works for it and steps down when a way fails, at opening or at any picture (`ladder`: on the window's graphics device when the editor hands one over as `OnDevice`, then the GPU's decoder with a copy back, then the CPU), with the reason in the log and no error on screen unless the last way fails too; a file the window's device failed for is not tried there again in the session; `DeviceWatch` is told before the device is asked for anything and how it came back (answered, or an error), for the crash and freeze guard in `graphics`; errors come back with the picture. `Convert` says where colour is converted: pictures come as YUV planes for the viewer's shader when it can take them (in memory, or as images on the graphics card when decoded on the window's device), else as RGBA. `close` stops the worker and waits, briefly, until it has ended: for closing the app, so nothing is still decoding when the window's graphics go away |
| `src/meter.rs` | `Meter`: the master output's peak per channel and its K-weighted loudness over 0.4 s and 3 s (LUFS), fed by the audio thread and read by the interface without locks (`Reading`) |
| `src/mix.rs` | `Mixer`: the timeline's sound at any moment: audible tracks (mute, solo, volume) summed, decoders kept open per file, stream and reading rate (a clip at another speed reads its file faster or slower, tape-style), seeking only when a clip doesn't continue |
| `src/playback.rs` | `Playback`: an audio thread owns the sound card stream and the mixer, keeping a quarter-second ring buffer full; the card's callback never waits or allocates. The position is the samples actually played (the master clock); without a sound card a wall clock runs and playback is silent. The clock stands still until the first sound is ready, so the picture never runs ahead of it, and stopping never waits for the audio thread: the card goes quiet at once and the thread ends by itself. Closing the app does wait, briefly, for those threads to end (`wait_until_quiet`) |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
