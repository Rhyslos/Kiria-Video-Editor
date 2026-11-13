# audio

The real-time audio engine: effects on every track, the master, and the mixer that runs them in the sound card's callback without allocating, locking or waiting. Pure Rust: no sound card, no files, no FFmpeg, so it runs the same live and in an export and is tested without a device.

## Status

Built (8.1, first part): every effect, the master, the mixer, its controller, the input rings, the fixed-block adapter, the meters and the analyser, all tested. Not yet used by `engine`: playback and export still mix with `engine::mix` until the integration (the second part of 8.1, see "How the engine will use it"). The design and how it was chosen are in [docs/research/audio-engine.md](../../docs/research/audio-engine.md) (measured in `spikes/audio-engine`).

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | The map, the re-exports and `BLOCK` (256 frames: every block the mixer makes) |
| `src/dsp/mod.rs` | `to_db`, `from_db`, `smoothing` (a one-pole smoother's coefficient) |
| `src/dsp/biquad.rs` | `Coefs` (high pass, band pass, peak, shelves, K-weighting; worked out in double precision) and `response_db`, the gain at a frequency (the page draws the EQ's curve with it); `Biquad`, one filter for both channels |
| `src/dsp/filters.rs` | `HighPass` (24 dB an octave, two Butterworth sections) and `Eq` (six `Band`s: low shelf, four peaks, high shelf, each on or off; `FLAT` is where they start) |
| `src/dsp/dynamics.rs` | `Compressor` (feed-forward, linked stereo, peak detection, soft knee, attack and release in dB, makeup, `reduction_db` for the meter), `CompressorSettings::curve` (the static curve the page draws) and the plain `Preset`s: Gentle, Voice, Music, Strong |
| `src/dsp/cleanup.rs` | Noise clean-up: `SpectralGate` (1024-sample windows every 256; frequencies not clearly above the noise go down, smoothly over time and across neighbours; always delays by `LATENCY`, 1024 frames, on or off; off and fully open it skips the FFT). `NoiseProfile`: the noise in 64 bands by frequency, per hertz, so it works at any rate; `learn` (a stretch of noise) and `learn_quietest` (the quietest quarter of a longer stretch) are for a worker; `bins` works out the gate's `BinNoise` for a rate, off the real-time thread |
| `src/dsp/duck.rs` | Ducking: `Key` (the dialogue's speech band, 250 Hz to a few kHz, its level per sample) and `Ducker` (per ducked track: threshold, hold, down and up speeds, depth; turned off, it comes back up at its up speed) |
| `src/dsp/level.rs` | The Loudness tool, `Leveller`: keeps a track near a loudness (K-weighted, 0.4 s), slowly, within a largest boost and cut; pauses (a quick 50 ms measure under -50 LUFS or 15 dB under the slow one) hold the gain |
| `src/dsp/lines.rs` | Volume and pan: `Line` (points by sample from the timeline's start, straight between), `Lines` (volume in dB added to the fader's; pan, which takes the fader's place while it has points), `pan_gains` (equal power; the centre leaves both sides as they are) and `Fader` (volume, pan, heard or not; a change glides over one block). Lines are worked out every `GRID` (64) samples from the timeline's start and glide between, so where a block starts never changes the sound |
| `src/dsp/bands.rs` | The 64 bands by frequency (20 Hz to 20 kHz, a sixth of an octave each) noise profiles and the analyser use |
| `src/dsp/loudness.rs` | Loudness and true peak as the broadcast loudness recommendation says (K-weighting, gating, four-times oversampled peaks). The same code as `engine::loudness` (moved here, see Decisions) |
| `src/dsp/limiter.rs` | The master's true-peak limiter (1.5 ms look-ahead, 80 ms release; the same code as export's `engine::limiter`, moved here), plus `set_ceiling` (infinite: only the delay, so it can be turned on and off while playing) and `take_recent_db` (the meter's gain reduction) |
| `src/settings.rs` | `TrackSettings` and `MasterSettings` (plain values), `TrackChange` and `MasterChange` (one section each; `changes_from` lists the sections that differ, `apply` applies one), built from the project with `from_project`; `lines_from_project` and `noise_from_project` / `noise_to_project` for what needs memory; `sample_at` (a moment to the nearest sample, exactly) |
| `src/strip.rs` | `Strip`: a track's effects in the Audio page's order (high pass, clean-up, EQ, the analyser's tap, compressor, Loudness, ducking, fader, meter), each on or off; `apply` never allocates; `take_state` carries the running state over to a rebuilt strip |
| `src/master.rs` | `Master`: the mix's volume (gliding), compressor, limiter and meter |
| `src/mixer.rs` | `mixer(rate, slots, master)` makes the `Mixer` (the real-time end), its `Controller` and the `Analyser`. `Mixer::process` makes one block: swaps, then commands, dialogue tracks (the ducking key) first, then the rest ducked under them, mute and solo, the master. `latency` (the fixed delay: clean-up plus limiter), `position`, `dry_frames` |
| `src/control.rs` | `Command` (a track's or the master's section of settings, which track the analyser listens to, `Locate`), `Swap` and `Returned` (what needs memory, in and back out), `Controller` (the interface's end: `change_track`, `change_master`, `add_track`, `rebuild_track`, `remove_track`, `set_lines`, `set_noise`, `locate` / `located`, `collect` frees what came back). Three wait-free queues of `QUEUE` (1024) items, made once |
| `src/feed.rs` | A track's input ring: `FeedWriter` (the feeder's end: `write`, `room`, `take_owed`) and `FeedReader` (the mixer's: a dry ring is silence, owed to the feeder, which skips as much so a late file never drifts). How a jump works is in its notes |
| `src/block.rs` | `BlockAdapter`: any device buffer size and channel count in, 256-frame blocks through the mixer |
| `src/readout.rs` | `TrackReadout` (peaks held until taken, momentary and short-term loudness, how far the compressor, clean-up and ducking turn it down, the Loudness tool's gain) and `MasterReadout` (loudness, true peak, the compressor's and limiter's gain reduction, reset), all atomics |
| `src/meter.rs` | `TrackMeter`: a track's peaks and momentary and short-term loudness (no true peak: cheaper) |
| `src/analyser.rs` | The EQ's analyser: `Tap` (the mixer's end, never waits: a full ring skips the block), `Analyser::step` (the worker's end: 2048-sample windows, the loudest frequency of each of the 64 bands, falling back about 20 dB a second) and `Spectrum` (what the interface reads) |
| `src/test_signal.rs` | Made-up sound for tests and the timing run: `Rng`, `speech` (a speech-like buzz in syllables and phrases, and where it speaks), `sine` |
| `tests/mixer.rs` | The mixer as the engine will use it: the fixed delay, a command heard within one block, mute and solo, ducking, dry rings, jumps, swaps coming back, a rebuilt track without a jump, the same sound through device buffers of 128, 441, 480 and 1024 frames, the limiter's ceiling, the analyser |
| `tests/no_alloc.rs` | 32 tracks with every effect, a command every block and swaps, under an allocator that counts (`os::alloc_count`): no allocation or free on the mixing thread |
| `tests/stress.rs` | 32 tracks with every effect: the time per block, printed; lenient on CI. The ten-second release run is by hand: `cargo test -p audio --release --test stress -- --ignored --nocapture` |

## How it fits together

```text
feeder thread ──FeedWriter──► ring per track ──FeedReader──┐
interface ──Controller──► commands, swaps ─────────────────┤
                       ◄── returned (freed here) ──────────┤
                                                    Mixer::process (256 frames)
                                                      ├─ dialogue strips → key
                                                      ├─ other strips (ducked)
                                                      └─ master → BlockAdapter → device
readouts (atomics) ◄────────────────────────────────── meters
Analyser::step (worker) ◄── Tap (after the chosen track's EQ)
```

## How the engine will use it (not done yet)

- **Playback:** build the mixer before the stream starts (`mixer(rate, tracks, MasterSettings::from_project(seq))`, then `add_track` for every audio track with `TrackSettings::from_project`, `lines_from_project` and `noise_from_project`). The cpal callback owns the `Mixer` and a `BlockAdapter`, calls `os::float::denormals_as_zero()` and `adapter.render(&mut mixer, data, channels)`. The audio thread becomes the feeder: today's per-clip work (decoding, clip volume and pan, fades, speed, ramps, nested timelines) per track into each `FeedWriter`, about half a second ahead, skipping `take_owed()` frames. The clock counts frames played minus `Mixer::latency()`.
- **Edits while playing:** compare the new project's `TrackSettings` with the old (`changes_from`) and send the changes; lines and noise go through `set_lines` / `set_noise`; track volume, mute and solo leave `engine::mix` and travel as changes. Call `Controller::collect` every frame.
- **Jumps:** stop feeding, `locate(at)`, wait for `located()`, then feed from `at`, skipping what is owed.
- **Export:** the same feeder and mixer on export's thread, block after block, without a device.
- **One loudness meter and limiter:** `engine::loudness` and `engine::limiter` become `pub use audio::dsp::loudness::*;` and `pub use audio::dsp::limiter::*;`, and `engine::meter` reads `MasterReadout`.

## Numbers

On the two-core cloud machine (load average about 5.7 from other work while measured), 32 tracks with every effect on and the master's compressor and limiter, 256-frame blocks at 48 kHz (5.33 ms each), release build: mean 1.34 to 1.40 ms a block (25 to 26 %), median 0.89 to 0.91 ms; the 99th percentile (5.1 ms) and the worst (9 to 18 ms) are the machine taking the thread away, as in the spike. Test build: mean 1.57 ms. No allocation or free on the mixing thread. The fixed delay at 48 kHz is 1111 frames (23.1 ms: the clean-up's 1024 and the limiter's 87). Clean-up on speech over hiss: pauses down 14.4 dB with words down 0.06 dB (hiss peaking at -50 dBFS), 15.9 and 0.95 dB (-34 dBFS).
