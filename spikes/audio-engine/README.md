# spikes/audio-engine

Compares ways of building the real-time audio engine from [docs/research/audio-engine.md](../../docs/research/audio-engine.md) on the same synthetic 32-track input. Pure Rust, no FFmpeg, no sound card needed. Throwaway code: deleted once the choice is built.

| File | What |
| --- | --- |
| `src/dsp/` | The effects every design shares: high pass and six-band EQ (`filters.rs`, on `biquad.rs`), compressor and expander (`dynamics.rs`), noise clean-up by spectral gating (`gate.rs`), ducking key and ducker (`duck.rs`), volume and pan lines (`fader.rs`), true-peak limiter (`limiter.rs`), meters and four-times oversampled true peak (`meter.rs`), the analyser's tap and worker (`mod.rs`) |
| `src/graph.rs` | The three ways of holding a track's effects (A enum, B boxed trait, C fixed strip) and the graph: dialogue tracks, ducking key, the other tracks, master |
| `src/settings.rs` | A track's settings as plain values and as a flat list of numbers |
| `src/params.rs` | The three ways settings reach the real-time thread (S1 atomics, S2 swapping pre-built strips, S3 a command queue) |
| `src/signal.rs` | The input: speech-like, music, effects and ambience signals, two of each, read by 32 tracks |
| `src/bench.rs` | Building a graph, timing a run, clean-up quality |
| `src/alloc.rs` | A global allocator that counts allocations and frees on the thread marked real-time; flushing denormals |
| `src/os.rs` | A thread's own processor time (Linux) and raising a thread's priority |
| `src/checks.rs` | Tests: the designs sound bit for bit the same and never allocate, the limiter holds -1 dBTP, the compressor's ratio, the gate keeps words and lowers pauses, every settings way delivers without allocating |
| `run.bat` | Windows: runs the measurement and writes `results.md` |

## Running it

```sh
cargo test --release
cargo run --release > results.md          # about a minute; `-- quick` for a short run
```

**Windows:** run the "Audio engine spike" workflow in GitHub Actions, download the `audio-bench-windows` artifact, unzip it and double-click `run.bat`.

## What it measures

1. Each design with every effect on every track (high pass, clean-up, EQ, compressor, ducking on the 24 tracks that aren't dialogue, fader lines, meter, an analyser tap on one track) and the master (compressor, limiter, meters), in blocks of 256 and 512: time per block (mean, median, 99th, worst by the clock and by the thread's own processor time) and allocations on the processing path.
2. What each effect costs alone on all 32 tracks.
3. The spectral gate against the expander on speech over noise at two levels.
4. What a change of settings costs on each side, for one track or all 32 changing every block.
5. Paced in real time: how long until a change is used, how late the thread wakes up, with and without busy threads on every core and with the thread's priority raised.
