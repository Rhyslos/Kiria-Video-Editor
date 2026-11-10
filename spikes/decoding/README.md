# spikes/decoding

Measures the three decoding approaches from [docs/research/decoding.md](../../docs/research/decoding.md) on the same clips. Throwaway code: deleted once the choice is made.

| File | What |
| --- | --- |
| `src/decoder.rs` | FFmpeg decoding: software, hardware with frames copied back to memory, or Vulkan on wgpu's device |
| `src/vulkan_direct.rs` | Approach C: opens wgpu's Vulkan device with video queues and hands it to FFmpeg; FFmpeg's Vulkan structures declared by hand, with a layout test |
| `src/gpu.rs` | Uploads frames in memory to wgpu textures, as the editor would |
| `src/main.rs` | Runs every approach on every clip and prints a Markdown table |
| `make-clips.sh` | Makes the synthetic test clips (Linux) |
| `run.ps1`, `run.bat` | Windows: makes the clips, measures, writes `results.md` |

## Running it

**Windows:** run the "Decoding spike" workflow in GitHub Actions, download the `decode-bench-windows` artifact, unzip it, put a few of your own recordings in `recordings` if you like, and double-click `run.bat`. It takes about 10 minutes the first time (making the clips) and writes `results.md`.

**Linux:** install FFmpeg 8.1 development files (or unpack the build pinned in `packaging/ffmpeg.env` and set `FFMPEG_DIR` to it), then:

```sh
./make-clips.sh clips
cargo run --release -- clips/*
```

`cargo test --release` checks the hand-written FFmpeg structure layouts.

Approaches: `software`, `hw-copy:<type>` (`d3d11va`, `d3d12va`, `vaapi`, `vulkan`), `vulkan-direct`. Pick some with `--approach`; the default runs all that fit the system.
