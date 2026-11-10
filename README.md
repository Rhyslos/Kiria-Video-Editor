# Kiria

A fast, light, native video editor, written in Rust.

Kiria is a hobby project, built with Claude and maintained by one person. It is open source and will stay open source.

> **Status: early development.** There is nothing to run yet. Progress is tracked in the [build plan](docs/build-plan.md).

## Planned for the first version

- **Five pages:** Library, Edit, Color, Audio and Export, with the same controls and shortcuts on every page.
- **Editing:** multi-track timeline with separate video and audio areas, the usual cutting tools, snapping, timeline labels and a rough-cut view.
- **Colour:** grading wheels, curves, colour range, masks, LUTs and scopes.
- **Audio:** mixer, EQ, compressor, noise clean-up, ducking and loudness normalisation.
- **Responsive:** decoding, effects and exports run in the background; when a machine can't keep up, preview quality drops before frames do.
- **Explained errors:** every error says what happened, why, and how to fix it.
- **Safe with your work:** autosave, crash recovery and undo; source files are never changed.
- **Resource controls:** limits for CPU threads, GPU work, memory and cache size.
- **Updates:** signed builds that roll back by themselves if a new version fails to start.

## Supported formats

| Type | Formats |
| --- | --- |
| Video | MP4 and MKV (H.264, H.265, AV1, VP9) |
| Audio | MP3, OGG and WAV, plus AAC, Opus and FLAC inside video files |
| Images | PNG and JPEG |

## Platforms

Windows first, then Linux.

## Building

You need [Rust](https://rustup.rs). The exact version is pinned in `rust-toolchain.toml` and installs itself on first build.

```sh
git clone https://github.com/Rhyslos/kiria-video-editor
cd kiria-video-editor
cargo build --workspace
cargo test --workspace
```

## Project layout

| Folder | Contents |
| --- | --- |
| `crates/` | The code, split into small crates (see [crates/README.md](crates/README.md)) |
| `docs/` | The [project plan](docs/project-plan.md), the [build plan](docs/build-plan.md), research notes and decisions |
| `xtask/` | Project chores, run with `cargo xtask` |

Every folder has a README describing what it contains.

## Licence

Kiria is licensed under the GNU General Public License v3.0 or later (see [LICENSE](LICENSE)). You are free to use, share and change it; anything you distribute that is based on Kiria must be shared under the same licence, with its source code.
