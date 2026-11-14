# render

Putting the timeline's picture together on the graphics card: layers drawn into a frame, and a frame shown. What the layers are at a moment is the engine's business (`engine::frame`); this crate knows nothing of projects or files.

Research and the measurements behind the choices: [docs/research/rendering.md](../../docs/research/rendering.md).

## Status

Built: the compositor, used by the preview worker (`engine::compose`) and the viewer (`ui_kit::video_picture`), and picture effects (`effects.rs`: vignette, film grain, sharpen, glow). Still to come here: more effects, titles.

## Files

| File | What it holds |
|---|---|
| `src/geometry.rs` | `Area`: a rectangle in the frame that may be turned (`fitted`: the largest upright one of a picture's shape that fits, centred; `of_part`: where a part of a picture placed, sized and turned by a `Placement` lands; `covers`: whether it hides the whole frame; `scaled`). `Colour`: quick colour as the shader's numbers, made from what the sliders say. `Part`: a part of a picture. No graphics card needed |
| `src/compositor.rs` | `Compositor`: draws layers into a `Frame` (a picture of the timeline's size, in linear light or in display values: `Working`). A layer's pixels are the decoder's planes (two or three, converted to RGB while drawing), RGBA, or a picture the compositor made itself (`Pixels::Made`: a nested timeline's picture, or what is below an adjustment layer, drawn again: its working values unpremultiplied, coloured in linear light and premultiplied again); a frame is drawn over black or, for a picture to be drawn over another, over nothing (`Background`, `draw_over`); it has an area in the frame (which may be turned), a part of the picture to show, a turn to show the stored picture upright, an opacity, a second picture to blend in with its share (frame blending: both are turned into light and mixed before anything else, whatever the frame is made in), and quick colour (`Colour`: gains for exposure, temperature and tint, contrast around middle grey, saturation; always worked in linear light). `Shower` draws a finished frame, or the part of it that is seen (`show_part`, for a zoomed-in viewer; `WHOLE` for all of it), into the window or any 8-bit target. Plain wgpu, no interface toolkit |
| `src/effects.rs`, `src/effects.wgsl` | `Effects`: picture effects as passes, each reading pictures the compositor made and writing one of the same size; `apply` runs a clip's effects one after another, taking pictures to write into and handing back those used (`Pictures`). `Effect` holds an effect's numbers as the shaders take them: `Vignette` (darker towards the edges in an ellipse of the picture's shape), `Grain` (brightness noise in cells, strongest in the middle tones, a pattern per `seed`), `Sharpen` (pushed away from a blur of the neighbours), `Glow` (what is over a threshold, blurred across and down, added back: four passes). Sizes are reference pixels (of 1080 lines of the picture), so a smaller preview looks like the export. Worked on light: a frame in display values is turned into light and back (its glow's blur is rough) |
| `src/reader.rs`, `src/planes.wgsl` | `Reader`: a finished frame read back from the card as NV12 for an encoder (BT.709, limited range). The conversion is two small passes on the card; the read waits without holding the device from the window. `black`: a black picture in the same layout |
| `src/layer.wgsl`, `src/show.wgsl` | The two shaders |
| `src/compositor/tests.rs` | Known colours through the compositor on whatever graphics device the machine has (a software one will do): filling, bars, mixing in linear light and in display values, parts, turned pictures, see-through RGBA, turned areas, two frames blended as light, a picture the compositor made drawn again as it was and coloured |

## Rules

- Layers come premultiplied by their opacity and are laid "over" what is below.
- In linear light, video values become light by the reference display's curve (a power of 2.4) on the way in and go back the same way when shown, so a picture nothing is done to looks exactly as it does drawn directly.
- Nothing here names a file or decodes: pixels arrive as textures on the device the compositor was made for.
