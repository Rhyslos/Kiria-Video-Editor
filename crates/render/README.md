# render

Putting the timeline's picture together on the graphics card: layers drawn into a frame, and a frame shown. What the layers are at a moment is the engine's business (`engine::frame`); this crate knows nothing of projects or files.

Research and the measurements behind the choices: [docs/research/rendering.md](../../docs/research/rendering.md).

## Status

Built: the compositor, used by the preview worker (`engine::compose`) and the viewer (`ui_kit::video_picture`). Still to come here: effects, titles.

## Files

| File | What it holds |
|---|---|
| `src/geometry.rs` | `Area`: a rectangle in the frame that may be turned (`fitted`: the largest upright one of a picture's shape that fits, centred; `of_part`: where a part of a picture placed, sized and turned by a `Placement` lands; `covers`: whether it hides the whole frame; `scaled`). `Colour`: quick colour as the shader's numbers, made from what the sliders say. `Part`: a part of a picture. No graphics card needed |
| `src/compositor.rs` | `Compositor`: draws layers into a `Frame` (a picture of the timeline's size, in linear light or in display values: `Working`). A layer's pixels are the decoder's planes (two or three, converted to RGB while drawing) or RGBA; it has an area in the frame (which may be turned), a part of the picture to show, a turn to show the stored picture upright, an opacity, and quick colour (`Colour`: gains for exposure, temperature and tint, contrast around middle grey, saturation; always worked in linear light). `Shower` draws a finished frame into the window or any 8-bit target. Plain wgpu, no interface toolkit |
| `src/layer.wgsl`, `src/show.wgsl` | The two shaders |
| `src/compositor/tests.rs` | Known colours through the compositor on whatever graphics device the machine has (a software one will do): filling, bars, mixing in linear light and in display values, parts, turned pictures, see-through RGBA, turned areas |

## Rules

- Layers come premultiplied by their opacity and are laid "over" what is below.
- In linear light, video values become light by the reference display's curve (a power of 2.4) on the way in and go back the same way when shown, so a picture nothing is done to looks exactly as it does drawn directly.
- Nothing here names a file or decodes: pixels arrive as textures on the device the compositor was made for.
