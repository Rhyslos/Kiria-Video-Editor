# cache

Every cache, each with a budget: RAM frame cache, GPU texture cache, disk render cache, proxies, thumbnails and waveforms. Least-recently-used items are dropped first.

## Status

Phase 4 in progress: waveforms and clip thumbnails are done. Frame caches and proxies come next; the timeline's cache bar (`ui_kit::timeline::cache_bar`) is where they will show what is ready.

## Files

| File | What it holds |
|---|---|
| `src/waveform.rs` | `peaks(path, stream)`: the loudest sample in each 1/100 s of a sound stream, read once on a worker (a 20-minute stream is 120,000 numbers, kept in memory). `peaks_as_read` hands the peaks over a few seconds of sound at a time while reading, for progress and for drawing the waveform as it fills in, and can be stopped |
| `src/thumbs.rs` | `strip(path, stream, …)`: small pictures (fitted into 160×90) every 2 s or more along a video file, at most 300 per file, for drawing inside clips; tells its progress after each picture |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
