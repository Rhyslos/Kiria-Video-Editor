# engine

Ties the parts together: what is on the timeline at a moment, and the workers that turn it into pictures (and later sound and exported files). The interface only sends requests and reads results back, so slow media can never freeze a window.

## Status

Phase 4A in progress: the picture at the playhead. Sound, playback scheduling and caching come next; export in Phase 6.

## Files

| File | What it holds |
|---|---|
| `src/timeline.rs` | `video_at`: the top visible video clip at a time and where in its file that is |
| `src/preview.rs` | `Preview`: a worker thread that decodes the picture at the playhead. Only the newest request counts (scrubbing never queues); decoders stay open for the four files used last; errors come back with the picture |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
