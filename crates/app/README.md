# app

The editor window and its five pages. Binary name: `editor`.

## Status

Phase 0.6 done: the window, the shared top bar, five empty pages that switch instantly, shortcuts, Settings, search for commands, inline errors and Diagnostics. Autosave runs on a worker. The pages fill up from Phase 4.

Phase 4A so far: importing media (Ctrl+I, the Import button, or dropping files on the window), the viewer with its transport, the timeline (click a clip to select it, drag to move it, drag its edge to trim; V, B, R, T, Y and U pick the Select, Blade, Ripple, Roll, Slip and Slide tools; N snapping; audio tracks show waveforms and have mute, solo and volume; Delete, Backspace and S act on the selection; drag the red needle; Ctrl+wheel zooms, \\ shows it all), playback with sound (Space plays; pausing goes back to where playback started, Stop stays), a scrub bar and preview quality (Left/Right step a frame, Home/End jump), and Timeline settings (Automatic until the first video is imported, a template, or the same as one of the project's videos). The handle between the viewer and the timeline shares the height between them (drag it; double-click for the usual split), and is remembered. Also: Ctrl+/ for the keyboard shortcuts page, F for a full-screen viewer (Escape returns), the region and timeline labels (Shift-drag in the ruler, L loops, right-click menu), the project picture for the launcher, and finding missing media.

## Running

```sh
cargo run -p app                         # no project
cargo run -p app -- trip.kiria           # opens (or creates) a project
cargo run -p app -- trip.kiria --open-last-save   # sets aside unsaved work from a crash instead of recovering it
cargo run -p app -- trip.kiria --import match.mkv # imports media at start (repeatable)
cargo run -p app -- trip.kiria --playhead 12.5     # starts with the playhead there (for screenshots)
cargo run -p app -- trip.kiria --play              # starts playing once imports are in (for checks)
cargo run -p app -- trip.kiria --cpu-colour        # converts the viewer's colour on the CPU, to compare with the GPU's picture
cargo run -p app -- --smoke-test         # open, draw 30 frames, close; exit code 1 on failure
cargo run -p app -- --responsiveness-test  # 8-second background job; fails unless 99 frames in 100 take under 16 ms of interface work (and none over 50 ms)
cargo run -p app -- --screenshot out.png # save a PNG of the window and close (waits for imports and the viewer's picture first)
cargo run -p app -- --graphics-warning --screenshot out.png  # with the question about a failed start with Vulkan
```

The last four are for CI and for checking the look without a screen (`xvfb-run` on Linux).

## Files

| File | What it holds |
|---|---|
| `src/main.rs` | Start-up: options, diagnostics, settings, the window (opened with the first graphics system that starts, see Graphics below), exit code |
| `src/clip_pictures.rs` | The pictures inside video clips: each file's strip read once on a worker, then uploaded 24 a frame so the interface never stalls |
| `src/editor.rs` | `Editor`: the window's state, shortcuts, autosave hand-off to workers, pages, Settings and Search windows, the automated runs |
| `src/frame_grab.rs` | Save this frame as a picture (Ctrl+Shift+E): asks where, then writes the frame at the playhead at full size on a worker |
| `src/import.rs` | `Importer`: the file dialog (its own thread), reading dropped or chosen files on workers, and applying them in the order chosen; `summary` describes an imported file in one line |
| `src/timeline_panel.rs` | The open timeline as the timeline widget's data (linked clips show selected together), label colours, the selection |
| `src/media_drawer.rs` | The Edit page's media drawer: the project's files by name with a filter; double-click or + places one at the end of the timeline; right-click to insert or overwrite at the edit point, or drag it onto the timeline (overwrite there; Ctrl inserts) |
| `src/search.rs` | The search palette (Ctrl+K): commands with their shortcuts, both settings windows, the keyboard shortcuts page, timeline labels and clips; Enter picks the first |
| `src/silence_panel.rs` | The Remove silence window (Ctrl+Shift+R): track, threshold, minimum length and edge to keep; marks what it found on the timeline and cuts it all out as one step |
| `src/relink.rs` | Missing media: checked on a worker whenever the project's media changes; the banner on the Edit page and "Find them…", which searches a picked folder and relinks what it finds |
| `src/label_editor.rs` | The window for one timeline label (double-click it): name, colour, or remove |
| `src/looping.rs` | Looping playback inside the region (L): where it starts and when it jumps back |
| `src/waveforms.rs` | Waveforms for the timeline: each file's sound stream read once on a worker, kept for the session |
| `src/versions_window.rs` | Saved versions (Ctrl+Shift+V): save the project under a name and restore any earlier one, both on workers |
| `src/timeline_panel.rs` (also) | `fold_sound`: the rough cut view (Ctrl+Shift+T) folds each video clip's linked sound into it |
| `src/viewer.rs` | The Edit page's viewer: asks the engine for the picture at the playhead (only when something changed) and shows the newest, as YUV planes the GPU converts or as an RGBA texture; the scrub bar and preview quality under it (planes come halved to suit the size wanted; on Auto, RGBA pictures are lowered further when playback can't keep up) |
| `src/video_picture.rs` | Whether this window converts colour on the GPU (`--cpu-colour` or a window without wgpu says no), and a decoded frame's planes as the viewer's picture; its tests draw the shader into a texture and compare with the CPU's conversion |
| `src/settings.rs` | `Settings` (theme, resource budgets, "Edit at the playhead", how the height is shared between the viewer and the timeline), saved as `settings.ron` in the app's data folder. The Graphics choice has its own file, shared with the launcher |
| `src/run_mode.rs` | Command-line options |

## Notes

- Opening a project that has unsaved work from a crash recovers it, unless `--open-last-save` is given. The launcher asks which the person wants and passes the flag.
- Resource and Graphics setting changes apply on the next start.

## Graphics

Which graphics system draws the window, the Graphics row in Settings and the question after a start with Vulkan that never drew a frame all come from the [graphics](../graphics/) crate, shared with the launcher. `main` calls `graphics::prepare` and `graphics::open_window`; the window keeps a `graphics::WindowGraphics`. Automated runs (`--smoke-test`, `--screenshot`, `--responsiveness-test`) write no marker and change no setting. What is in use is on the first line of Diagnostics (Settings, Diagnostics…), under the Graphics setting, in the log (`the window is open`) and in crash reports.
