# app

The editor window and its five pages. Binary name: `editor`.

## Status

Phase 0.6 done: the window, the shared top bar, five empty pages that switch instantly, shortcuts, Settings, search for commands, inline errors and Diagnostics. Autosave runs on a worker. The pages fill up from Phase 4.

Phase 4A so far: importing media (Ctrl+I, the Import button, or dropping files on the window), the viewer with its transport, the timeline (click a clip to select it, drag to move it, drag its edge to trim; V, B, R, T, Y and U pick the Select, Blade, Ripple, Roll, Slip and Slide tools; N snapping; audio tracks show waveforms and have mute, solo and volume; Delete, Backspace and S act on the selection; drag the red needle; Ctrl+wheel zooms, \\ shows it all), playback with sound (Space plays; pausing goes back to where playback started, Stop stays), a scrub bar and preview quality (Left/Right step a frame, Home/End jump), and Timeline settings (Automatic until the first video is imported, a template, or the same as one of the project's videos). Also: the region and timeline labels (Shift-drag in the ruler, L loops, right-click menu), the project picture for the launcher, and finding missing media.

## Running

```sh
cargo run -p app                         # no project
cargo run -p app -- trip.kiria           # opens (or creates) a project
cargo run -p app -- trip.kiria --open-last-save   # sets aside unsaved work from a crash instead of recovering it
cargo run -p app -- trip.kiria --import match.mkv # imports media at start (repeatable)
cargo run -p app -- trip.kiria --playhead 12.5     # starts with the playhead there (for screenshots)
cargo run -p app -- trip.kiria --play              # starts playing once imports are in (for checks)
cargo run -p app -- --smoke-test         # open, draw 30 frames, close; exit code 1 on failure
cargo run -p app -- --responsiveness-test  # 8-second background job; fails unless 99 frames in 100 take under 16 ms of interface work (and none over 50 ms)
cargo run -p app -- --screenshot out.png # save a PNG of the window and close
```

The last three are for CI and for checking the look without a screen (`xvfb-run` on Linux).

## Files

| File | What it holds |
|---|---|
| `src/main.rs` | Start-up: options, diagnostics, settings, the window, exit code |
| `src/editor.rs` | `Editor`: the window's state, shortcuts, autosave hand-off to workers, pages, Settings and Search windows, the automated runs |
| `src/frame_grab.rs` | Save this frame as a picture (Ctrl+Shift+E): asks where, then writes the frame at the playhead at full size on a worker |
| `src/import.rs` | `Importer`: the file dialog (its own thread), reading dropped or chosen files on workers, and applying them in the order chosen; `summary` describes an imported file in one line |
| `src/timeline_panel.rs` | The open timeline as the timeline widget's data (linked clips show selected together), label colours, the selection |
| `src/media_drawer.rs` | The Edit page's media drawer: the project's files by name with a filter; double-click or + places one at the end of the timeline |
| `src/search.rs` | The search palette (Ctrl+K): commands with their shortcuts, both settings windows, timeline labels and clips; Enter picks the first |
| `src/silence_panel.rs` | The Remove silence window (Ctrl+Shift+R): track, threshold, minimum length and edge to keep; marks what it found on the timeline and cuts it all out as one step |
| `src/relink.rs` | Missing media: checked on a worker whenever the project's media changes; the banner on the Edit page and "Find them…", which searches a picked folder and relinks what it finds |
| `src/looping.rs` | Looping playback inside the region (L): where it starts and when it jumps back |
| `src/waveforms.rs` | Waveforms for the timeline: each file's sound stream read once on a worker, kept for the session |
| `src/viewer.rs` | The Edit page's viewer: asks the engine for the picture at the playhead (only when something changed) and shows the newest; the scrub bar and preview quality under it |
| `src/settings.rs` | `Settings` (theme, resource budgets, "Edit at the playhead"), saved as `settings.ron` in the app's data folder |
| `src/run_mode.rs` | Command-line options |

## Notes

- Opening a project that has unsaved work from a crash recovers it, unless `--open-last-save` is given. The launcher asks which the person wants and passes the flag.
- Resource setting changes apply on the next start.
