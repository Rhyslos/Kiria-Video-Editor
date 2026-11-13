# graphics

Which graphics system draws the windows: the shared Graphics settings, what a start tries, and the safety nets around Vulkan and around decoding video on the window's device. The launcher and the editor both use it, so the two behave as one app.

## Status

Done for decoding video on the window's own device (`docs/research/decoding.md`, approach C): choosing the graphics system, safely; for the editor's window with Vulkan, opening the device here with its video decode queues so that the decoder can share it; the switch and the crash guard for decoding there; and handing a picture decoded there to the window's renderer.

## How a start goes

On Windows a window is drawn through Vulkan when that works, and through Direct3D 12 otherwise. On Linux nothing is decided here: the window toolkit picks (Vulkan, then OpenGL).

1. **`prepare`** reads `graphics.ron` and the program's start marker from the data folder and calls `plan`, a pure function of the system, the setting, the marker and the kind of run.
2. **`open_window`** tries each graphics system of the plan on its own. Automatic and Vulkan try Vulkan, then Direct3D 12, then whatever the window toolkit picks by itself; Direct3D 12 skips the first. One that reports an error (no driver, no card that can draw to the window, no device) is followed by the next in the same start: the toolkit returns the error before the window is shown and can be run again. Vulkan only counts with a real graphics card; software drawing through Vulkan is passed over.

   **The editor's Vulkan device is opened here, for video decoding** (`vulkan_device`). The window toolkit would open a device that can only draw. So the Vulkan try first opens the device itself, through the renderer's lower layer: the same card rule, the same limits, plus the driver's video extensions, one queue in each family that decodes video, one helper queue in a compute family other than the window's, and two device features FFmpeg's decoder uses. The toolkit then gets that device as an existing one. If the driver is older than Vulkan 1.3, has no video decode queue for H.264, H.265 or AV1, or no spare queue for the decoder (the window's own queue is never shared: see `crates/video-device`), or if anything else fails on the way, the toolkit opens its own device exactly as before, still with Vulkan, and Diagnostics says why. All of it happens after the marker is written, so a driver that crashes on it is caught like any other failed start with Vulkan. The launcher plays no video and never asks for this.
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

## Decoding video on the window's device

When the editor's device was opened here with video decoding, the viewer's video is decoded on it (`media`, `engine`). That has its own switch and its own safety net, apart from the window's:

- **The switch.** `decode_on_window_device` in `graphics.ron`, shown in Settings under Graphics as "Decode video on the window's graphics device" (a switch with a Restart badge). On by default; a change applies from the next time the editor starts. The line under it says what this window does and why.
- **Errors the device reports** need nothing from here: that file is decoded the older way, and the log says why (`engine`).
- **A crash or a freeze** is caught by a second marker, `video-decoding.starting`, held by `DecodeGuard`. It is written before the device is first asked to decode in a session and removed once ten pictures decoded there have been drawn (`PICTURES_BEFORE_TRUSTED`), when the device reports an error instead, or when the window closes normally (after which nothing is written any more, whatever the decoding thread still does). Setting the device up for the decoder at start is guarded the same way, for the moment it takes, so a session that shows no video leaves nothing behind.
- **A decoding thread frozen in the driver** leaves the window working, so the app can be closed normally. If the device was asked for something ten seconds ago or longer (`STUCK_AFTER`) and has not answered when the window closes, the marker stays, or is written, and the next start treats it like a crash.
- **A marker found at the next start** means the session ended in between. `prepare` switches decoding on the device off in `graphics.ron` before the window opens, and the window asks: **Keep it off** or **Try it again next time** (switched on again; the marker guards that session too, and a second failure asks again). Closing without answering keeps it off. The window still opens with Vulkan: the two are decided apart.

| Found at start | Switch | Decoding on the device | Also |
|---|---|---|---|
| no marker | on | yes, when the device can | marker written around the first pictures |
| marker, its process gone | on | no | switched off and saved, then the question |
| marker, its process still running | on | no | nothing saved or asked: another copy is in its first pictures |
| anything | off | no | a stale marker is removed |

The decision is part of `plan`, next to the one about Vulkan, and automated runs follow the same facts without writing or asking.

**Handing over a picture.** `plane_textures` wraps the two images of a picture decoded on the device (`video_device::PlaneImages`) as textures of the window's renderer, without copying. The images were left ready for sampling by the copy that filled them, which is the state the renderer is told; the pair stays alive until the renderer is done with both textures, work under way on the card included, and goes back for the next picture after that. It takes about 30 microseconds.

## One file for both programs

`graphics.ron` in the data folder holds the choice: Automatic (the default), Vulkan or Direct3D 12, and the switch for decoding on the window's device. Both programs read it at start and show the same Graphics row in their Settings; a change applies to every window from the next start. The editor kept the choice in its own `settings.ron` for a short while: that value is still read when `graphics.ron` doesn't exist, and moved over at the next normal start.

## With the updater's rollback

The launcher rolls a version back after two failed editor starts in a row (`updater::health`). A crash in the Vulkan driver is one failed start: the next start avoids Vulkan, reports that it started, and the count is back at zero. The launcher has a test for this order.

## Files

| File | What it holds |
|---|---|
| `src/lib.rs` | The overview and `FRAMES_BEFORE_TRUSTED` |
| `src/settings.rs` | `GraphicsChoice` and `GraphicsSettings` (`graphics.ron`): the choice, and the switch for decoding on the window's device |
| `src/marker.rs` | `Program`, `StartMarker` (one for a start with Vulkan, one for decoding on the device) and what a start finds (`MarkerState`): nothing, a marker left behind, or one whose program is still running |
| `src/plan.rs` | `plan`: what to try in order, whether to guard Vulkan with the marker, whether to decode on the device, and what to ask |
| `src/start.rs` | `prepare`, `open_window`, the toolkit's set-up per graphics system and the choice of card |
| `src/decode_guard.rs` | `DecodeGuard`: the marker around a session's first pictures decoded on the device |
| `src/vulkan_device.rs` | Opens the window's Vulkan device with video decoding and fills in the `VideoDevice` handle for the decoder; `plan_video` decides what to add from the card's queue families, extensions and versions, or says why not; `plane_textures` hands a decoded picture's images to the renderer. The crate's only `unsafe` is here |
| `src/video.rs` | `VideoDecoding`: the handle, or the reason there is none, as Diagnostics words it |
| `src/window.rs` | `WindowGraphics`, the window's side: the first-frames check, the two questions with their answers, Settings' Graphics card (`SETTINGS`, the rows as data) and each row's control (`settings_row`), what the editor decodes on (`device_decoding`), and the lines for Diagnostics, the log and crash reports (what draws the window, "Video decoding on this device", "Viewer decodes on this device") |

## Using it

```rust,ignore
let (startup, problems) = graphics::prepare(graphics::Program::Editor, &data_dir, graphics::RunKind::Normal);
graphics::open_window(&startup, brand::APP_NAME, window_options, start, |cc, start, opened| {
    Box::new(Editor::new(cc, start, opened)) // keeps a `WindowGraphics::new(cc, opened, &mut errors)`
})?;
```

In the window: `graphics.frame(ctx, frames_drawn)` at the top of every frame, `graphics.show_warning(ctx, theme)` with the other dialogs, and in Settings a Graphics tab holding `graphics::SETTINGS`, whose rows `graphics.settings_row(row)` draws (`ui_kit::settings`): the graphics system as a segmented control and the decoding switch, each with what is in use now under it.

`--graphics-warning` on either program shows the question about Vulkan although nothing failed, for screenshots; `--decoding-warning` on the editor shows the one about decoding.

## Notes

- Diagnostics shows "Video decoding on this device: H.264, H.265, AV1" (the kinds the device's decode queue takes) or "not available (the reason)", and "Viewer decodes on this device: on, for H.264, H.265, AV1", "off, as chosen in Settings", "off, because it stopped the app last time" or "not possible (the reason)"; the log has the same lines. `graphics.device_decoding()` gives the editor the handle and the guard when decoding there is on.
- When both questions are due in one start, the one about Vulkan comes first.
- Two copies of the editor share one decoding marker. The second sees the first's marker while the first is in its first pictures and stays off the device for that session; nothing is saved or asked.
- On Linux the window toolkit still opens the device, so there is no video decoding on it yet: Linux has no start marker, and opening the device ourselves without that guard would leave a driver crash with nothing to catch it.
- The tests open a device this way on whatever Vulkan the machine has (a software driver will do) and pass without checking when there is none. One copies a stand-in picture on the card, wraps it with `plane_textures`, draws it with the shared viewer's shader and compares every pixel, then checks that the images come back for reuse once the renderer has let go.
- `WGPU_BACKEND` still works on Linux. On Windows the setting decides and the variable only affects the last resort.
- A program that has been open for a while shows the choice it read at start; a change made in the other program meanwhile shows after a restart.
