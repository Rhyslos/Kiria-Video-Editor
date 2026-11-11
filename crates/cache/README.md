# cache

Every cache, each with a budget: RAM frame cache, GPU texture cache, disk render cache, proxies, thumbnails and waveforms. Least-recently-used items are dropped first.

## Status

Phase 4 in progress: waveforms and clip thumbnails are done. Frame caches and proxies come next.

## Files

| File | What it holds |
|---|---|
| `src/waveform.rs` | `peaks(path, stream)`: the loudest sample in each 1/100 s of a sound stream, read once on a worker (a 20-minute stream is 120,000 numbers, kept in memory) |
| `src/thumbs.rs` | `strip(path, stream, …)`: small pictures (fitted into 160×90) every 2 s or more along a video file, at most 300 per file, for drawing inside clips |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
