# cache

Every cache, each with a budget: RAM frame cache, GPU texture cache, disk render cache, proxies, thumbnails and waveforms. Least-recently-used items are dropped first.

## Status

Phase 4A in progress: waveforms are done. Frame caches, proxies and thumbnails come next.

## Files

| File | What it holds |
|---|---|
| `src/waveform.rs` | `peaks(path, stream)`: the loudest sample in each 1/100 s of a sound stream, read once on a worker (a 20-minute stream is 120,000 numbers, kept in memory) |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
