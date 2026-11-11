# engine

Ties the parts together: what is on the timeline at a moment, and the workers that turn it into pictures (and later sound and exported files). The interface only sends requests and reads results back, so slow media can never freeze a window.

## Status

Phase 4A in progress: the picture at the playhead, the timeline's sound, and playback with the sound as the clock. Caching comes next; export in Phase 6.

## Files

| File | What it holds |
|---|---|
| `src/thumbnail.rs` | The project's picture for the launcher: one frame of the timeline (at the playhead, or the first clip's), fitted into 480×270 on black, written as `<file>.thumbnail.png`; `save_frame` writes the frame at a time at its file's full size |
| `src/timeline.rs` | `video_at`: the top visible video clip at a time and where in its file that is |
| `src/preview.rs` | `Preview`: a worker thread that decodes the picture at the playhead. Only the newest request counts (scrubbing never queues); decoders stay open for the four files used last; errors come back with the picture |
| `src/mix.rs` | `Mixer`: the timeline's sound at any moment: audible tracks (mute, solo, volume) summed, decoders kept open per file and stream, seeking only when a clip doesn't continue |
| `src/playback.rs` | `Playback`: an audio thread owns the sound card stream and the mixer, keeping a quarter-second ring buffer full; the card's callback never waits or allocates. The position is the samples actually played (the master clock); without a sound card a wall clock runs and playback is silent |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
