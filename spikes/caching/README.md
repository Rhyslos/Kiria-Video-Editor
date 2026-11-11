# spikes/caching

Measures the ways to make scrubbing and jumping feel instant from [docs/research/caching.md](../../docs/research/caching.md) on the same clips. Throwaway code: deleted once the choice is built.

| File | What |
| --- | --- |
| `src/decoder.rs` | FFmpeg decoding (started from `spikes/decoding`): software, or the GPU's decoder with frames copied back, the editor's current path. Seeks, decodes one key frame alone, copies frames out at preview size, lists key frames, can leave out frames nothing depends on |
| `src/cache.rs` | Approach B: pictures in memory by time, with a byte budget; the one used longest ago goes first, the window around the playhead last |
| `src/build.rs` | Approaches C and D: builds proxies and picture indexes with the `ffmpeg` program and times it |
| `src/trace.rs` | The scrub traces: slow drag, slow drag back, fast drag, back and forth, stepping back, playing backwards |
| `src/scrub.rs` | The preview worker, modelled: answers a trace with pictures using one approach. The work is real and timed, the waiting is skipped |
| `src/main.rs` | Runs everything on every clip and prints Markdown tables |
| `make-clips.sh` | Makes the synthetic test clips (Linux) |
| `run.ps1`, `run.bat` | Windows: makes the clips, measures, writes `results.md` |

## What it measures

For every clip:

- **Jumps:** 100 random targets, the same for every approach. Time to the first picture, how far that picture is from the target, and time to the exact frame.
- **Files built for C and D:** build time, size, and the time to fetch one picture.
- **Decoder tricks:** a seek alone, racing to a target while leaving out frames nothing depends on or the deblocking filter, and what keeping every decoded frame adds.
- **Scrub traces:** each approach replays the same pointer movement. Response is the time from a new playhead position to the first picture for it or for a newer one. "Right frame" counts proxy pictures too, "exact frame" only the original, "from memory" is approach B's hit rate, and "settled" is the time from the last position to its exact frame.

The approaches in the trace table:

| Name | What it does |
| --- | --- |
| today | The editor's rule: decode on when the target is up to 3 s ahead, else seek; a decode is never dropped. (Without the editor's one-frame look-ahead.) |
| cancelling only | Seeks by the key frame index, drops a decode a newer position can't use, shows frames passed on the way |
| A key frame first | The above, plus the key frame before the target at once, from a second decoder |
| B memory | Cancelling, plus every decoded frame kept in memory and the stretch ahead of the playhead filled when idle |
| A+B | B, plus the key frame, or a nearer frame in memory, at once |
| C proxy first | Cancelling, plus the same frame from the proxy at once |
| B+C | B, plus the proxy frame at once |
| C proxy only | Only proxy pictures, never the original |
| B+D | B, plus the nearest picture from the picture index at once |

"Shown" means the picture is in memory at preview size. Uploading it to the GPU costs the same for every approach and is left out.

## Running it

**Windows:** run the "Caching spike" workflow in GitHub Actions, download the `cache-bench-windows` artifact, unzip it, put two or three of your own recordings in `recordings` if you like, and double-click `run.bat`. It takes about 10 minutes a clip and writes `results.md`. Only the first minute of each recording is used.

**Linux:** install FFmpeg 8.1 development files (or unpack the build pinned in `packaging/ffmpeg.env` and set `FFMPEG_DIR` to it), put its `ffmpeg` program on the `PATH`, then:

```sh
./make-clips.sh clips
cargo run --release -- clips/*
```

On a slow machine, `SMALL=1 ./make-clips.sh clips` adds a 720p clip that decodes several times faster than it plays, as 1440p does on a desktop GPU. Without that headroom nothing that decodes can follow a drag, and the cache has nothing to show for itself.

`cargo test --release` checks the cache, the traces and the summary arithmetic.

Options: `--decode software` or an FFmpeg device type (`d3d11va`, `vaapi`; the default is `d3d11va` on Windows, `software` elsewhere; give it twice to measure both), `--jumps N`, `--preview WxH` (1280x720), `--cache-mb N` (1024), `--span SECONDS` (60), `--work FOLDER` for the built files, `--ffmpeg PROGRAM`, `--quick` for a short smoke run, `--no-traces` to leave out the scrub traces (most of the running time).
