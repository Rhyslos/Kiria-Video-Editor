# CLAUDE.md

A fast, light, native video editor written in Rust. A hobby project, open source under GPL-3.0-or-later. Windows first, then Linux.

## Start here

1. Open [docs/build-plan.md](docs/build-plan.md). Find the first phase whose status is not "Gate passed" and take its first unticked step.
2. Read the `README.md` of every folder you will touch before writing code.
3. Finish the step (definition of done below), tick it in `docs/build-plan.md`, and update any README or this file that the change makes out of date, all in the same commit.

[docs/project-plan.md](docs/project-plan.md) says *what* the app is; [docs/build-plan.md](docs/build-plan.md) says *how* and *in what order*. If work would contradict the project plan, update the plan first and say so.

## Working rules

1. **Uniform controls.** A tool behaves the same and uses the same shortcut on every page. Shared parts (playhead, edit needle, region, timeline labels, transport, the shortcut table) are built once in `crates/ui-kit` and reused, never re-implemented per page.
2. **One feel across pages.** Switching pages feels like moving around one app.
3. **Shared visuals stay put.** Top bar, page tabs (centred over the viewport), panel widths and theme tokens come from `crates/ui-kit` only.
4. **No name-dropping.** Code, comments, docs, commit messages and interface text never name other companies' products. Describe the feature instead.
5. **Documented folders.** Every crate and every folder with more than a couple of files has a `README.md`. This file is the map. Both are updated in the same commit as the code they describe.
6. **Readable, clean, stable code.** Small modules, clear names, consistent patterns. No `unsafe` outside the future FFmpeg and GPU wrappers. No `unwrap()` or `expect()` outside tests: return an explained error.
7. **Research and compare big systems.** For playback, caching, the timeline widget, rendering, the audio engine, background removal, auto white balance and the highlight finder: write a note in `docs/research/`, prototype two or three approaches in `spikes/`, measure them on the same clips, then build the winner properly.
8. **One name, one place.** The app's name and identifiers live only in `crates/brand`. A test fails if the name is typed in any other Rust file; read `brand::APP_NAME` instead.

## Definition of done

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.
- New behaviour has tests.
- Errors use the shared what / why / fix type from `crates/diag` (once it exists), never a bare string.
- Nothing slower than about 2 ms runs on the interface thread.
- READMEs, this file and `docs/build-plan.md` are current.

## Layout

```text
Cargo.toml           workspace, shared versions and lint rules
rust-toolchain.toml  pinned Rust version
deny.toml            dependency licence and advisory checks (cargo-deny)
crates/              the workspace crates; see crates/README.md for the table
docs/                project plan, build plan, research notes, decisions
spikes/              throwaway prototypes (not built)
tests/               cross-crate tests
test-media/          shared test recordings (only the manifest is in git)
xtask/               chores: `cargo xtask check-media`
.github/workflows/   CI on Windows and Linux
```

Binaries: `cargo run -p app` (the editor, binary `editor`) and `cargo run -p launcher`.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask check-media
```

`rust-toolchain.toml` pins the Rust version, so rustup downloads it on first use. Offline, override with `RUSTUP_TOOLCHAIN=stable` for local checks; CI always uses the pinned version.

## Decisions so far

- Time is stored as exact fractions (frames over rate), never floats.
- The project file is versioned from its first commit; every format change gets a migration and a stored sample.
- FFmpeg is linked dynamically and its licence text ships with the app.
- Licence: GPL-3.0-or-later (`LICENSE`). Dependencies must have GPL-compatible licences; `deny.toml` checks this in CI. Never copy code whose licence is incompatible.
