# CLAUDE.md

A fast, light, native video editor written in Rust. A hobby project, open source under GPL-3.0-or-later. Windows first, then Linux.

## Start here

1. Open [docs/build-plan.md](docs/build-plan.md). Find the first phase whose status is not "Gate passed" and take its first unticked step. A gate marked *waiting for the owner* doesn't hold work back: carry on with the next phase.
2. Read the `README.md` of every folder you will touch before writing code.
3. Finish the step (definition of done below), tick it in `docs/build-plan.md`, and update any README or this file that the change makes out of date, all in the same commit.

[docs/project-plan.md](docs/project-plan.md) says *what* the app is; [docs/build-plan.md](docs/build-plan.md) says *how* and *in what order*. If work would contradict the project plan, update the plan first and say so.

## How we work together

- **Chores happen directly:** fixing CI, dependency and tooling updates, documentation, merging finished pull requests. Commit to `main` (or merge) without asking.
- **Features come as pull requests, one feature each.** The description says what the feature does, how to try it in the app step by step, and what is not done yet. Every pull request builds a test installer (the Release workflow's `windows` artifact). The owner may ask to merge first and test through the update: every push to `main` publishes an automatic beta that installed beta copies install by themselves. Merged pull requests keep their test steps. The pull requests double as a guided tour of the app.
- **Releases:** when asked to release, bump the version and `CHANGELOG.md` (a small pull request or a direct commit), then start the Release workflow with the new tag. The workflow publishes on its own once everything passes.

## Working rules

1. **Uniform controls.** A tool behaves the same and uses the same shortcut on every page. Shared parts (playhead, edit needle, region, timeline labels, transport, the shortcut table) are built once in `crates/ui-kit` and reused, never re-implemented per page.
2. **One feel across pages.** Switching pages feels like moving around one app.
3. **Shared visuals stay put.** Top bar, page tabs (centred over the viewport), panel widths and theme tokens come from `crates/ui-kit` only.
4. **No name-dropping.** Code, comments, docs, commit messages and interface text never name other companies' products. Describe the feature instead.
5. **Documented folders.** Every crate and every folder with more than a couple of files has a `README.md`. This file is the map. Both are updated in the same commit as the code they describe.
6. **Readable, clean, stable code.** Small modules, clear names, consistent patterns. No `unsafe` outside the FFmpeg and GPU wrappers (`crates/media`, `crates/video-device`, `crates/graphics/src/vulkan_device.rs`) and the operating-system calls (`crates/os`), each block with its reason. No `unwrap()` or `expect()` outside tests: return an explained error.
7. **Research and compare big systems.** For playback, caching, the timeline widget, rendering, the audio engine, background removal, auto white balance and the highlight finder: write a note in `docs/research/`, prototype two or three approaches in `spikes/`, measure them on the same clips, then build the winner properly.
8. **One name, one place.** The app's name and identifiers live only in `crates/brand`. A test fails if the name is typed in any other Rust file; read `brand::APP_NAME` instead.

## Definition of done

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.
- New behaviour has tests.
- Errors are `diag::AppError` (what, why, fix, optional one-click fix), returned as `diag::Result<T>`; wrap other errors with `.or_explain(...)`. Never a bare string.
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
xtask/               chores: `cargo xtask <ffmpeg | check-media | brand | keygen | manifest | sign | verify | icons>`
packaging/           installers: Windows (Inno Setup), Linux (AppImage), icons, FFmpeg pin
.github/workflows/   CI on Windows (Linux is paused, see below); release.yml builds, tests, signs and publishes releases; spike-*.yml build spike programs to run by hand
RELEASING.md         how to make a release; CHANGELOG.md what each release changed
```

Binaries: `cargo run -p app` (the editor, binary `editor`) and `cargo run -p launcher`.

To see a window without a screen (Linux): `xvfb-run -a -s "-screen 0 1600x1000x24" target/debug/editor --screenshot out.png`, or `target/debug/launcher --demo 14 --screenshot out.png` for the project browser with sample projects. CI also runs `--smoke-test` for both.

**Linux is paused in CI and in releases** (November 2026): only Windows is built, tested and published. The Linux steps are still in the workflows; each has a comment saying how to turn it back on. Until then nothing in CI runs `--responsiveness-test` or takes screenshots, so run both by hand with the commands above before merging interface work, and keep the code building on Linux.

## Commands

```sh
cargo xtask ffmpeg      # once: fetches the pinned FFmpeg build into .ffmpeg/ (needed to build)
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask check-media
```

Building needs libclang (for the FFmpeg bindings); on Windows set `LIBCLANG_PATH` if it isn't found. `.cargo/config.toml` points the build at `.ffmpeg/sdk`. On Windows, run `cargo xtask ffmpeg` again after `cargo clean`: it copies the DLLs beside the programs.

`rust-toolchain.toml` pins the Rust version, so rustup downloads it on first use. Offline, override with `RUSTUP_TOOLCHAIN=stable` for local checks; CI always uses the pinned version.

## Decisions so far

- Time is stored as exact fractions (frames over rate), never floats.
- The project file is versioned from its first commit; every format change gets a migration and a stored sample.
- How a project is looked at (track heights, the video/audio divider) is view state, not an edit: it lives in `<project file>.view` beside the project, never in the project file, and makes no undo step.
- Ways of looking at the timeline that are ways of working rather than about one project (the rough cut view, compact tracks, the divider between video and audio taken away, what a click on the timeline moves) are settings, the same for every project; they never change the heights or the divider's place kept in the `.view` file.
- A clip's keyframes are kept in its file's time (source time), not the timeline's, so they stay on the picture through moves, trims and speed changes; values between keyframes are worked out in whole numbers so preview and export agree exactly.
- A clip's speed ramp (speed keyframes) is kept in timeline time from the clip's start, unlike look keyframes, because a ramp is about the timeline; where the clip is in its file is the ramp added up in whole numbers, exactly to the flick (1/705 600 000 s), so preview and export agree and steady stretches land exactly on recorded frames. Setting a ramp keeps the part of the file a clip plays; its length follows, to the nearest frame.
- A project has several timelines; which one is open is view state (kept in `<project file>.view`, no undo step), while making, copying, renaming and removing one are edits. A clip plays a part of a file, another timeline (a nested timeline) or nothing (an adjustment layer): `project::ClipKind`. A timeline never plays itself, directly or through others. An adjustment layer's colour applies to the picture below it as a whole, in linear light: what is below is put together first and drawn again coloured.
- The files that belong to a project (journal, backups, thumbnail, timeline layout, saved versions) are listed in one place, `project::side_files`. Moving, renaming and deleting a project go by that list; a new kind of file kept beside projects is added there. Deleting moves files to the recycle bin and never touches recordings.
- Shortcuts can be changed (the keyboard shortcuts page), so the shortcut table holds the combinations in use and interface text asks it for key names (`keys_for`, `keys_or_name`, `tip`) instead of typing them; it writes them with the characters printed on the keyboard in use (the system's, followed live through `crates/os`, unless one is picked on the page). Settings keep only the changed ones, by key position and US key names. No two commands share a combination.
- Templates (reusable intros and outros) belong to the person, not to a project: each is a small file in `templates/` in the app's data folder with its own format version, and nothing about them is stored in a project file. Dropping one in is an ordinary edit that carries the whole template.
- Proxies belong to recordings, not projects: one per file in the app's cache folder, named by the file's fingerprint, shared by every project and dropped oldest-used first within the disk budget. They are not `project::side_files` and nothing about them is in a project file. The proxy mode (Off / On / Auto) is a setting. Export never reads proxies (its preview workers are never handed the shelf). Clearing the cache (Settings › Cache, for the open project or all) removes only proxy files from the cache folder, never recordings or project files; a proxy cleared on purpose is remembered in the cache folder and never built again by itself, only by hand.
- A project's media labels, custom lists and notes are project data (`project::Library`, format 13), changed by undoable commands like any edit. The library's SQLite index (`crates/library`) is derived: built in memory from the project when it opens and kept in step on its own thread, never written back, never a file on disk. Labels are per project; label names and colours can also be offered in every project ("everywhere"): those belong to the person, like templates, in `labels.ron` in the app's data folder with its own format version (`project::global_labels`). Putting one on a file copies it into the project's own labels (an ordinary undoable edit), so a project file never depends on that file; taking one out of the set leaves every project that used it with its own copy.
- The Library page adds files with the ordinary commands: a whole file as `PlaceMedia` / `InsertMedia`, a part marked with in and out (or at a level chosen from its loudness) as a template made on the spot and dropped in with `InsertTemplate`, so it is one undo step on the tracks the whole file would take. Marks and the levels chosen per sound stream are kept for the session unless Settings › Library says to save them in the project (`project::MediaUse`, format 15, changed by `SetMediaUses`: one undo step per change); the file itself is never changed. I and O mark in and out on every page (the region on the Edit page).
- A media path in a project file is normally a full one; a relative path means "from the project file's folder" and is made full whenever a project is opened (`project::file::resolve_media_paths`). Packages (`project::package`: the project and only the media its timelines use, in one folder) are written that way so they open wherever the folder is moved. A package holds a clean save, the launcher's picture and the timeline layout, and the saved versions only when ticked (asked each time, off at first; then the files only they use go in too); never the journal, backups, templates or proxies. Saved versions' relative paths are from the project's folder too (`versions::load`).
- Export formats are listed once, in `media::encode::formats` (containers, picture and sound codecs, which go together, each codec's encoders in the order tried: the graphics card's, the system's, then the processor's); pages list them from there and ask `engine::export::Encoders` (on a worker) what this machine can encode. Every encoder aims at a bit rate from the quality, so the size shown before export holds whichever encoder opens. Pictures are 8-bit 4:2:0 BT.709 limited range for now, with a key frame at least every two seconds, each starting a closed group, and H.264 in its High profile: what video sites ask for.
- A file's sound is timed from its picture's start (`media::audio`; a file of sound alone from its own), so sound that starts before or after the picture keeps that offset, as players play it. Samples are counted, never taken from each packet's rounded timestamp, and a jump lands on the sample a straight read reaches; the mixer jumps only on a real jump (a cut, a seek, a change of speed), so played and exported sound is the file read straight.
- The Export page's own presets belong to the person, like templates: one small file, `export-presets.ron` in the app's data folder, with its own format version and codecs written by short names of their own; nothing about them is in a project file. How fast exports went on this machine is a setting (`export_pace`), used for the time the page says one will take. A queued export is the timeline as it was when it was added; the queue belongs to the window and runs on every page, one export at a time.
- An export at another shape than the timeline's fits the timeline's picture inside with plain black bars (never cut off or stretched); "1080p" in a preset means 1080 lines on the shorter side with the timeline's shape kept.
- Sound is mixed by `crates/audio` (pure Rust, no device): a fixed strip of every effect per track in the Audio page's order, 256-frame blocks at the device's rate, run in the sound card's callback with nothing allocated, freed, locked or waited on there (a test counts allocations). Numbers travel as small commands through a wait-free queue (heard within one block); what needs memory (a track, its lines, a learned noise profile) is built off the thread and swapped in, the old sent back to be freed elsewhere. Each track's sound comes from a ring a feeder fills; a dry ring plays silence and the feeder skips as much. Every track always passes the clean-up's delay and the mix the limiter's, so the delay is fixed and taken off the clock. Track effects, role, pan and volume and pan lines (timeline time) are project data (`project::TrackSound`, format 16), every effect off by default; the loudness meter and true-peak limiter live in `audio::dsp` (`engine::loudness` and `engine::limiter` re-export them). Playback and export both mix through it: a feeder per playback (`engine::feeder`) decodes each track's clips half a second ahead into its ring, the sound card's callback (`engine::output`, cpal's `realtime` priority) runs the mixer, and export runs the same feeder and mixer offline (`engine::offline`), so export equals playback to the sample; with every effect off the sound is exactly what it was before the effects came. A nested timeline sounds exactly as it does opened, its master included: the feeder of the track its clip is on mixes it with an `OfflineMix` of its own (`OfflineMix::nested`, its fixed delay mixed ahead and thrown away; made afresh on a real jump), at the rate that plays it at the clip's speed (with a ramp, at the outer rate and read as a ramped file is), then the clip's own sound and the outer track's strip apply. While playing, the editor hands the playback the project whenever the mix changed (`Playback::follow`, a comparison per frame), so track and master changes and their undo are heard within a block; jumps (`Playback::jump`) keep the sound card open.
- Sync by sound (`engine::sync`) compares what clips play from their files and moves them with one edit (`LineUp`, one undo step; in the way of something, sound takes a free track and a picture is refused, as when a clip is dragged to another track): a clip with a picture moves by whole frames, sound alone lines up to the sample (edges on frames, its place in the file slipped by the rest). An unsure match moves nothing unless the person asks (Move anyway).
- A clip's own volume and pan lines (`project::ClipLines`, format 17) are kept at moments of its file, like look keyframes, so they stay on the sound through moves, trims and speed changes; a track's lines stay in timeline time. Lines are drawn on every timeline and edited with the lines tool (A); a change is one undo step.
- The Audio page's changes are ordinary undoable commands (`SetTrackSound`, `SetMasterSound`, `SetTrackMix`); a value being dragged is kept by the page and playback follows the project as it would be (`AudioPage::shown`), so it is heard while it moves and becomes one undo step when let go. Its bottom view (Timeline, Mixer, Split and Split's direction) and how its loudness overview counts are settings, the same for every project; the Show filter is for the session. The loudness aimed for is one setting shared by the master column, Settings and the overview. Counting over several plays or inside the region tallies readings of the meters every 100 ms of the timeline (`audio::dsp::loudness::Tally`); "every play" shows the master meter's own count, as the Edit page does. Track meters have sample peaks only; true peak is the mix's.
- The clarity check (`engine::clarity`) listens to each track as the mixer hears it (the audio crate's own strips, ducking keyed by dialogue) and never changes the sound; its fixes are ordinary undoable `SetTrackSound` edits on one track that change only the effect they name: ducking -10 dB under the dialogue (threshold 15 dB under its speaking level), a -3.5 dB cut at 300 Hz and a -2.5 dB dip at 2.5 kHz for the voice, both Q 1, put in a flat EQ band or deepening one already there. Another track covers the voice per band from 6 dB under it to 6 dB over it, weighted by the published Speech Intelligibility Index band importance; only dialogue-role tracks count as the voice, every other heard track can cover it; three tracks piling up between 200 and 500 Hz make a muddy zone. The master is left out of the check. On the Audio page it checks around the playhead (a second either side) once playback stops and the playhead and tracks have rested 400 ms, or the region when asked (Check the region); a check no longer wanted is cancelled; findings show on the shared timeline as coloured stretches (`ui_kit::TimelineSpan`, colours in `ui_kit::theme::clarity`).
- FFmpeg is linked dynamically and its licence text ships with the app.
- Every program calls `diag::start(brand::VERSION)` first and `finish()` on a clean exit (logging, crash reports, crash detection).
- Releases are tagged `vX.Y.Z` (pushed, or typed into "Run workflow"); the release workflow builds, tests, signs and publishes. Pushes to `main` publish automatic betas (`brand::VERSION` comes from `BUILD_VERSION` then). Updates are trusted only with a valid signature from the `RELEASE_SIGNING_KEY` secret.
- A release is only published after an installed copy of the previous one has updated to it by itself in the release workflow (the update drill, `RELEASING.md`). Anything that changes how updates are found, fetched, installed or started is covered by it; keep `launcher --update-step` working.
- Licence: GPL-3.0-or-later (`LICENSE`). Dependencies must have GPL-compatible licences; `deny.toml` checks this in CI. Never copy code whose licence is incompatible.
