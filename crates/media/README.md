# media

Reading and writing media files: a small, safe layer over FFmpeg. The rest of the app sees plain Rust types and explained errors; every FFmpeg call stays here (this crate is the FFmpeg wrapper where the working rules allow `unsafe`).

## Status

Phase 4A in progress: probing and fingerprints are done. Decoding (see `docs/research/decoding.md`) and encoding come next.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | `describe(path)`: everything the project keeps about a file, as a `project::MediaRef` ready to import |
| `src/probe.rs` | `probe(path)`: container, streams, codecs, frame rate, length and rotation, read from headers without decoding. Names each sound stream from its title tag, else a useful handler name, else "Audio N" |
| `src/fingerprint.rs` | `fingerprint(path)`: size plus a hash of the first and last megabyte, to recognise a moved or renamed file |
| `src/ffi.rs` | The raw FFmpeg calls, each wrapped with its safety reasoning |
| `build.rs` | Linux debug builds: the tests find the fetched FFmpeg libraries |
| `tests/probe.rs` + `tests/fixtures/` | Probing real files: a recording with three named sound streams, a rotated phone clip, an MP3, a PNG and a file that isn't media |

## FFmpeg

- Linked dynamically through `ffmpeg-sys-next` (8.1). The build reads `FFMPEG_DIR`, which `.cargo/config.toml` sets to `.ffmpeg/sdk`; `cargo xtask ffmpeg` puts the build pinned in `packaging/ffmpeg.env` there (a GPL build; the app is GPL too).
- The installers ship the same build's shared libraries and licence text.
- Probing opens the file and reads its headers, which can take tens of milliseconds: call it from a worker, never the interface thread.
