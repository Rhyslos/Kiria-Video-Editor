# spikes/rendering

Measures the ways to put the timeline's picture together from [docs/research/rendering.md](../../docs/research/rendering.md), on this machine's graphics device. Throwaway code: deleted once the choice is built in `crates/render`.

One program without a window, `render-bench`. It needs no video files: the layers are stand-ins for decoded pictures, in the shape the zero-copy decoding path hands them over (brightness at full size, the two colour samples interleaved at half size).

| File | What |
| --- | --- |
| `src/gpu.rs` | Opens the device (Vulkan first, as the editor does; anything else when there is none), textures, waiting, and the card's own clock (timestamp queries) |
| `src/compose.wgsl`, `src/whole.wgsl` | The two shaders: a decoded picture placed into the frame (converted to RGB, cropped, scaled, turned, with an opacity, optionally into linear light), and a whole picture drawn over a whole target |
| `src/compose.rs` | The compositor in each way: straight to the window (approach A), into an 8-bit frame, into a floating-point frame in linear light, and that with a target of its own for every layer |
| `src/measure.rs` | What one picture costs each way |
| `src/contend.rs` | A stand-in for the interface drawing 60 or 144 frames a second while pictures are composed by nobody, by the same thread (approach B) or by a worker on the same device (approach C) |
| `src/main.rs` | Runs everything and prints Markdown |
| `run.ps1`, `run.bat` | Windows: measures and writes `results.md` |

## What it measures

- **Check.** A grey picture, and white laid over it at half strength, through every way. The program stops if the grey comes out wrong.
- **What a picture costs.** Layers of 1080p, 1440p and 4K into frames of 1080p, 1440p and 4K; one layer (a recording), two (with a facecam: scaled and cropped), four (with a full picture mixed in at half strength and a smaller one turned). For each way: the asking thread's own time, the time on the card by its own clock, the time from asking to finished, the time per picture back to back, and the first picture's time.
- **While the interface draws.** The stand-in interface's frames: how many were late, the interface thread's own time per frame, and when the card finished each frame.

What it leaves out: the interface's own drawing (a few thousand small triangles a frame) and a real window's waiting for the screen. Both are the same for every approach.

## Running it

```sh
cargo run --release                       # everything, about two minutes on a graphics card
cargo run --release -- --quick            # a short run that shows it works
cargo run --release -- --backend dx12     # another graphics system (vulkan, dx12, gl, any)
```

On Windows without a Rust toolchain: the "Rendering spike" workflow builds a zip; unpack it and double-click `run.bat`.

On a machine without a graphics card the software driver is measured instead. Those numbers only show that the program works.
