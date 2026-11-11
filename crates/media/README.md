# media

Reading and writing media files: a small, safe layer over FFmpeg. The rest of the app sees plain Rust types and explained errors; every FFmpeg call stays here (this crate is the FFmpeg wrapper where the working rules allow `unsafe`).

## Status

Phase 4A in progress: probing, fingerprints, frame timing, decoding to pictures and to sound are done. Pictures come as the decoder's YUV planes for the GPU to convert, or as RGBA. Encoding comes in Phase 6; the zero-copy GPU path from `docs/research/decoding.md` comes later.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | `describe(path)`: everything the project keeps about a file, as a `project::MediaRef` ready to import |
| `src/probe.rs` | `probe(path)`: container, streams, codecs, frame rate, length and rotation, read from headers without decoding. Names each sound stream from its title tag, else a useful handler name, else "Audio N" |
| `src/audio.rs` | `AudioDecoder`: a sound stream as interleaved stereo float samples at any rate, from any point; placed by timestamps |
| `src/decode.rs` | `VideoDecoder`: the frame on screen at a time (found by timestamp). `picture_at` gives it as the decoder's YUV planes (`Decoded::Yuv`) when the format is 8-bit 4:2:0 with a BT.601 or BT.709 matrix, else as RGBA; `frame_at` always gives RGBA, scaled to fit a size and turned upright, in the file's own colour matrix and range. GPU decoding (Direct3D 11 on Windows, VA-API on Linux; the decoder that supports the GPU is picked per codec, since for AV1 it isn't FFmpeg's default) with a CPU fallback per file; decodes onward for small steps and seeks for jumps |
| `src/yuv.rs` | `YuvFrame`: a picture's planes copied out of the decoder once, without row padding (interleaved colour as GPU decoders copy back, or planar as CPU decoders give), at the file's size or halved by averaging to suit the size asked for; `Colour` (BT.601 or BT.709 from the file, else guessed by size; limited or full range) and `Colour::rgb_rows`, the conversion the shader uses; `to_rgba`, the same conversion on the CPU for checks. Plane memory goes back to its decoder when a picture is dropped |
| `src/timing.rs` | Reads packet times at the start, middle and end (no decoding) to tell constant from variable frame rate, and the nearest standard rate to show the file at |
| `src/relink.rs` | `missing(project)`: media not on disk; `find(folder, wanted, cancelled)`: looks through a folder and its subfolders for files by fingerprint (only files of the right size are hashed), so moved and renamed files are found |
| `src/fingerprint.rs` | `fingerprint(path)`: size plus a hash of the first and last megabyte, to recognise a moved or renamed file |
| `src/ffi.rs` | The raw FFmpeg calls, each wrapped with its safety reasoning |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
| `tests/audio.rs` | Decoding the fixtures' tones: right pitch per stream, any rate, to the end, seeking |
| `tests/decode.rs` | Decoding the fixtures: frames by time, jumps, upright rotation, variable frame rate, the fallback without a GPU, planes that convert to the same picture as the RGBA path, and colours that follow the matrix and range the file names |
| `tests/probe.rs` + `tests/fixtures/` | Probing real files: a recording with three named sound streams, a rotated phone clip, an MP3, a PNG, a variable-frame-rate clip and a file that isn't media. `colour-709-full.mp4` (three squares of known colour, stored as BT.709 full range) is for the colour checks |

## FFmpeg

- Linked dynamically through `ffmpeg-sys-next` (8.1). The build reads `FFMPEG_DIR`, which `.cargo/config.toml` sets to `.ffmpeg/sdk`; `cargo xtask ffmpeg` puts the build pinned in `packaging/ffmpeg.env` there (a GPL build; the app is GPL too).
- The installers ship the same build's shared libraries and licence text.
- Probing opens the file and reads its headers, which can take tens of milliseconds: call it from a worker, never the interface thread.
