# app

The editor window and its five pages. Binary name: `editor`.

## Status

Phase 0.6 done: the window, the shared top bar, five empty pages that switch instantly, shortcuts, Settings, search for commands, inline errors and Diagnostics. Autosave runs on a worker. The pages fill up from Phase 4.

Phase 4A so far: importing media (Ctrl+I, the Import button, or dropping files on the window), the viewer with its transport, the timeline (click a clip to select it, drag to move it, drag its edge to trim; V, B, R, T, Y and U pick the Select, Blade, Ripple, Roll, Slip and Slide tools; N snapping; audio tracks show waveforms and have mute, solo and volume; Delete, Backspace and S act on the selection; drag the red needle; Ctrl+wheel zooms at the pointer, = and - zoom in and out, \\ shows it all, Shift+wheel pans, the middle button drags the view), playback with sound (Space plays; pausing goes back to where playback started, Stop stays), a scrub bar and preview quality (Left/Right step a frame, Home/End jump), and Timeline settings (Automatic until the first video is imported, a template, or the same as one of the project's videos). Every track's height can be changed (drag the edge of its name, double-click for the usual height, Alt+wheel or Alt with = and - for every track of an area; Alt+\\ puts them back); heights and the divider between video and audio are kept per project. The handle between the viewer and the timeline shares the height between them (drag it; double-click for the usual split), and is remembered. Templates (Ctrl+T, or the Templates… button) keep an intro or outro to drop into any project. Also: Ctrl+/ for the keyboard shortcuts page (every shortcut can be changed there; changes are kept in settings and used by the launcher too; "Cheat sheet…" saves a page to print), F for a full-screen viewer (Escape returns), the region and timeline labels (Shift-drag in the ruler, L loops, right-click menu), the project picture for the launcher, and finding missing media.

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
cargo run -p app -- trip.kiria --import match.mkv --hold-reading 0.4 --screenshot out.png  # with waveforms and clip pictures stopped 40 % read, to see reading in progress
cargo run -p app -- trip.kiria --action Templates --screenshot out.png  # with a window opened by a command (any name from the shortcut table: Versions, RemoveSilence, …)
cargo run -p app -- trip.kiria --pointer 640,420 --screenshot out.png  # with the pointer resting there: hover marks and tooltips
cargo run -p app -- trip.kiria --pointer 640,420 --drag-to 640,480 --screenshot out.png  # while the left button drags from the first place to the second
cargo run -p app -- trip.kiria --pointer 640,420 --drag-to 640,480 --release --screenshot out.png  # after that drag was let go: what it did
cargo run -p app -- trip.kiria --pointer 60,700 --right-click --screenshot out.png  # with the menu a right-click opens there
cargo run -p app -- trip.kiria --pointer 60,700 --double-click --screenshot out.png  # after a double-click there (a track's name being typed)
cargo run -p app -- --shortcuts-page clash --screenshot out.png  # with the keyboard shortcuts page open: `filter` (one chip picked), `rebinding` (waiting for keys) or `clash` (keys another command has)
cargo run -p app -- --cheat-sheet keys.html  # write the printable cheat sheet for the shortcuts in settings and exit, without a window
```

The last ten are for CI and for checking the look without a screen (`xvfb-run` on Linux).

## Files

| File | What it holds |
|---|---|
| `src/main.rs` | Start-up: options, diagnostics, settings, the window (opened with the first graphics system that starts, see Graphics below), exit code |
| `src/clip_pictures.rs` | The pictures inside video clips: each file's strip read once on a worker, then uploaded 24 a frame so the interface never stalls; says how far the reading is |
| `src/reading.rs` | How far background reading has come, the way the timeline shows it: `Pace` turns a job's progress into waiting or reading with the time left (only once it can be told steadily) |
| `src/editor.rs` | `Editor`: the window's state, shortcuts, autosave hand-off to workers, pages, Settings and Search windows, the automated runs; turns what was done on the timeline into commands (moving a clip, also to another track; adding, removing, renaming and hiding tracks; dropping a file on a track) |
| `src/frame_grab.rs` | Save this frame as a picture (Ctrl+Shift+E): asks where, then writes the frame at the playhead at full size on a worker |
| `src/import.rs` | `Importer`: the file dialog (its own thread), reading dropped or chosen files on workers, and applying them in the order chosen; `summary` describes an imported file in one line |
| `src/timeline_panel.rs` | The open timeline as the timeline widget's data (linked clips show selected together), label colours, the selection; which clips are still waiting for their waveform or pictures, and how much of each track is ready (`TrackPaces` adds the time left); the stretches the cache bar marks (today: where waveforms and clip pictures are still being read) |
| `src/media_drawer.rs` | The Edit page's media drawer: the project's files by name with a filter; double-click or + places one at the end of the timeline; right-click to insert or overwrite at the edit point, or drag it onto the timeline: the picture goes on the video track under the pointer, over what is on the tracks it lands on (Ctrl inserts, pushing every track later) |
| `src/search.rs` | The search palette (Ctrl+K): every command with the shortcut it has now, both settings windows, the keyboard shortcuts page, timeline labels and clips; Enter picks the first |
| `src/silence_panel.rs` | The Remove silence window (Ctrl+Shift+R): track, threshold, minimum length and edge to keep; marks what it found on the timeline and cuts it all out as one step |
| `src/relink.rs` | Missing media: checked on a worker whenever the project's media changes; the banner on the Edit page and "Find them…", which searches a picked folder and relinks what it finds |
| `src/label_editor.rs` | The window for one timeline label (double-click it): name, colour, or remove |
| `src/looping.rs` | Looping playback inside the region (L): where it starts and when it jumps back |
| `src/waveforms.rs` | Waveforms for the timeline: each file's sound stream read once on a worker, kept for the session. While a stream is being read it gives the peaks read so far (copied a few times a second from a list shared with the worker, never waiting for it), how far the reading is and the time left; a reading still under way is stopped when its file is forgotten or the app closes |
| `src/view_state.rs` | How a project's timeline was laid out when last open (track heights, the divider between video and audio), kept in `<project file>.view` beside the project: read on opening, written on a worker a moment after the layout stops changing and on closing. Not part of the project file or its undo history |
| `src/versions_window.rs` | Saved versions (Ctrl+Shift+V): save the project under a name and restore any earlier one, both on workers |
| `src/templates_window.rs` | Templates (Ctrl+T, or Ctrl+Shift+S to save one): save the region or the whole timeline under a name; drop a saved one in at the start, the edit point or the end as one undoable step; rename and delete (both ask in the shared dialog). Lists each template's length and clips, marks ones whose media can't be found and says why a file can't be used; when something asked there fails, the window says so itself as well (the page's error strip may be under it). Templates live in `templates/` in the app's data folder, shared by every project; listing, saving, renaming and deleting run on workers |
| `src/timeline_panel.rs` (also) | `fold_sound`: the rough cut view (Ctrl+Shift+T) folds each video clip's linked sound into it. `track_ids`: the tracks there are, so the heights of removed ones are forgotten |
| `src/export_page.rs` | The Export page in its first form: the file (named after the project, beside it, until one is chosen), the whole timeline or its region, a quality with the size it will come to, the graphics card's encoder or the processor's; then progress in a sentence with Cancel, and where the video was saved with "Show in folder". `stop` gives an export up when the app closes |
| `src/inspector.rs` | The Edit page's inspector, on the right: the selected clip's Transform (position in frame pixels, size in percent of fitted, turn, opacity, Fit and Fill), Crop, Quick color (exposure, contrast, saturation, temperature, tint) and Speed, each section folding away with a one-line summary and a Reset. A value being dragged is shown in the viewer at once (`shown`: the project as it would be) and becomes one undo step (`SetLook`) when the hand is off it |
| `src/viewer.rs` | The Edit page's viewer: asks the engine for the picture at the playhead (only when something changed) and shows the newest. In a window drawn on a graphics card that is the timeline's whole picture, every visible layer put together by the worker into one frame ("put together on the GPU" in the corner), which is held while it is on show; otherwise the top clip alone, as YUV planes the GPU converts (from memory, or straight from the graphics card for video decoded on the window's device) or as an RGBA texture; the corner says how each picture was decoded ("hardware (vulkan, stays on the GPU)" for the window's device); counts pictures drawn for the crash guard, and goes back to the older ways for the session if the renderer can't take a picture; the scrub bar and preview quality under it (planes come halved to suit the size wanted; on Auto, RGBA pictures are lowered further when playback can't keep up) |
| `src/slow_pictures.rs` | The last few pictures that took more than 50 ms, each in words with where its time went (the jump, the first frame, the frames after it, handing over), for the Diagnostics line "Pictures that took long" |
| `src/video_picture.rs` | Whether this window converts colour on the GPU (`--cpu-colour` or a window without wgpu says no), and a decoded frame's planes as the viewer's picture; its tests draw the shader into a texture and compare with the CPU's conversion |
| `src/device_decoding.rs` | Decoding on the window's graphics device, wired up (`wrap` hands the worker a picture's images as textures to put together from): the device `graphics` opened becomes the decoder's (`media::WindowDevice`), with the guard around setting it up, its guard becomes the preview worker's watcher, and a decoded picture's images become the viewer's textures. `None` when the device can't, the switch is off, or colour is converted on the CPU |
| `src/settings.rs` | `Settings` (theme, resource budgets, "Edit at the playhead", how the height is shared between the viewer and the timeline, the shortcuts that were changed, the keyboard the shortcuts page draws), saved as `settings.ron` in the app's data folder. The Graphics choice has its own file, shared with the launcher |
| `src/cheat_sheet.rs` | The printable cheat sheet (the shortcuts page's "Cheat sheet…"): asks where, then writes the page `ui-kit` makes from the shortcut table on a worker |
| `src/run_mode.rs` | Command-line options |
| `src/fake_pointer.rs` | A scripted pointer for automated runs (`--pointer`, `--drag-to`): rests somewhere, or drags from there and keeps holding, so screenshots can show hover marks, tooltips and drags; `--release` lets go at the end of the drag, `--right-click` clicks the right button and `--double-click` the left one twice, for what a drag did, for menus and for what a double-click opens |

## Notes

- Opening a project that has unsaved work from a crash recovers it, unless `--open-last-save` is given. The launcher asks which the person wants and passes the flag.
- Resource and Graphics setting changes apply on the next start.
- The shortcut table is made once at start from the usual shortcuts and the changed ones in settings (`ShortcutTable::restored`). The shortcuts page changes that same table, so menus, tooltips, hints and search follow at once; the launcher reads the changes from the same settings file at its next start. While the page waits for new keys the window runs no commands from the keyboard.
- `--decoding-warning` shows the question about video decoding that stopped the app although nothing did, for screenshots.

## Graphics

Which graphics system draws the window, the Graphics row in Settings and the question after a start with Vulkan that never drew a frame all come from the [graphics](../graphics/) crate, shared with the launcher. `main` calls `graphics::prepare` and `graphics::open_window`; the window keeps a `graphics::WindowGraphics`. Automated runs (`--smoke-test`, `--screenshot`, `--responsiveness-test`) write no marker and change no setting. What is in use is on the first line of Diagnostics (Settings, Diagnostics…), under the Graphics setting, in the log (`the window is open`) and in crash reports.
