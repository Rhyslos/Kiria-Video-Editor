# graphics

Which graphics system draws the windows: the shared Graphics setting, what a start tries, and the safety net around Vulkan. The launcher and the editor both use it, so the two behave as one app.

## Status

Done for step 1 of 3 towards decoding video straight into the window's own device (`docs/research/decoding.md`): choosing the graphics system, safely. Creating the Vulkan device ourselves, with the video queues, comes next and belongs here.

## How a start goes

On Windows a window is drawn through Vulkan when that works, and through Direct3D 12 otherwise. On Linux nothing is decided here: the window toolkit picks (Vulkan, then OpenGL).

1. **`prepare`** reads `graphics.ron` and the program's start marker from the data folder and calls `plan`, a pure function of the system, the setting, the marker and the kind of run.
2. **`open_window`** tries each graphics system of the plan on its own. Automatic and Vulkan try Vulkan, then Direct3D 12, then whatever the window toolkit picks by itself; Direct3D 12 skips the first. One that reports an error (no driver, no card that can draw to the window, no device) is followed by the next in the same start: the toolkit returns the error before the window is shown and can be run again. Vulkan only counts with a real graphics card; software drawing through Vulkan is passed over.
3. **The marker** catches what reports nothing: a driver that crashes or freezes while the window opens. It is written before a start with Vulkan and removed once three frames are drawn (or the window closed normally, or Vulkan reported an error). The launcher's is `launcher-vulkan.starting`, the editor's `vulkan.starting`, so the two can start at the same time without reading each other's.
4. **A marker found at the next start** means Vulkan never drew a frame. That start uses Direct3D 12, in an instance that never loads the Vulkan driver, and saves Direct3D 12 as the setting before anything is drawn. Once the window is up it asks: **Keep Direct3D 12** or **Try Vulkan again next time** (back to Automatic; the marker guards that start too, and a second failure asks again). Closing the app without answering keeps Direct3D 12.

| Found at start | Setting | Tried | Also |
|---|---|---|---|
| no marker | Automatic or Vulkan | Vulkan, Direct3D 12, toolkit default | marker written around Vulkan |
| marker, its process gone | Automatic or Vulkan | Direct3D 12, toolkit default | Direct3D 12 saved, then the question |
| marker, its process still running | Automatic or Vulkan | Direct3D 12, toolkit default | nothing saved or asked: another copy is starting, or stuck |
| anything | Direct3D 12 | Direct3D 12, toolkit default | a stale marker is removed |
| anything, on Linux | any | toolkit default | |

Automated runs (smoke tests, screenshots, the launcher's demo) read the setting and the marker the same way but never write a marker, never change the setting and never ask.

## One setting for both programs

`graphics.ron` in the data folder holds the choice: Automatic (the default), Vulkan or Direct3D 12. Both programs read it at start and show the same Graphics row in their Settings; a change applies to every window from the next start. The editor kept the choice in its own `settings.ron` for a short while: that value is still read when `graphics.ron` doesn't exist, and moved over at the next normal start.

## With the updater's rollback

The launcher rolls a version back after two failed editor starts in a row (`updater::health`). A crash in the Vulkan driver is one failed start: the next start avoids Vulkan, reports that it started, and the count is back at zero. The launcher has a test for this order.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | The overview and `FRAMES_BEFORE_TRUSTED` |
| `src/settings.rs` | `GraphicsChoice` and `GraphicsSettings` (`graphics.ron`) |
| `src/marker.rs` | `Program`, `StartMarker` and what a start finds (`MarkerState`): nothing, a marker left behind, or one whose program is still running |
| `src/plan.rs` | `plan`: what to try in order, whether to guard Vulkan with the marker, whether to ask |
| `src/start.rs` | `prepare`, `open_window`, the toolkit's set-up per graphics system and the choice of card |
| `src/window.rs` | `WindowGraphics`, the window's side: the first-frames check, the question with its two answers, the Graphics row for Settings, and the lines for Diagnostics, the log and crash reports |

## Using it

```rust,ignore
let (startup, problems) = graphics::prepare(graphics::Program::Editor, &data_dir, graphics::RunKind::Normal);
graphics::open_window(&startup, brand::APP_NAME, window_options, start, |cc, start, opened| {
    Box::new(Editor::new(cc, start, opened)) // keeps a `WindowGraphics::new(cc, opened, &mut errors)`
})?;
```

In the window: `graphics.frame(ctx, frames_drawn)` at the top of every frame, `graphics.show_warning(ctx, theme)` with the other dialogs, and `graphics.settings_row(ui, theme)` under a "Graphics" heading in Settings.

`--graphics-warning` on either program shows the question although nothing failed, for screenshots.

## Notes

- `WGPU_BACKEND` still works on Linux. On Windows the setting decides and the variable only affects the last resort.
- A program that has been open for a while shows the choice it read at start; a change made in the other program meanwhile shows after a restart.
