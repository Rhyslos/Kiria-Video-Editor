# render

The timeline's picture: what it is made of at a moment, and putting it together on the graphics card.

Research and the measurements behind the choices: [docs/research/rendering.md](../../docs/research/rendering.md).

## Status

Built: the description of a frame and the compositor that draws one. Not wired into the editor yet: the viewer still draws the top clip alone. Still to come here: quick colour, effects, titles.

## Files

| File | What it holds |
|---|---|
| `src/frame.rs` | `describe`: what is seen at a moment of a timeline, as plain numbers: every video track that isn't hidden contributes its clip, bottom first, each fitted into the timeline's frame (`Area::fitted`: bars, never a stretched or cut-off picture). Layers that a frame-filling one above them hides are left out, so they are never decoded. No graphics card needed |
| `src/compositor.rs` | `Compositor`: draws layers into a `Frame` (a picture of the timeline's size, in linear light or in display values: `Working`). A layer's pixels are the decoder's planes (two or three, converted to RGB while drawing) or RGBA; it has an area in the frame (which may be turned), a part of the picture to show, a turn to show the stored picture upright, and an opacity. `Shower` draws a finished frame into the window or any 8-bit target. Plain wgpu, no interface toolkit |
| `src/layer.wgsl`, `src/show.wgsl` | The two shaders |
| `src/compositor/tests.rs` | Known colours through the compositor on whatever graphics device the machine has (a software one will do): filling, bars, mixing in linear light and in display values, parts, turned pictures, see-through RGBA, turned areas |

## Rules

- Layers come premultiplied by their opacity and are laid "over" what is below.
- In linear light, video values become light by the reference display's curve (a power of 2.4) on the way in and go back the same way when shown, so a picture nothing is done to looks exactly as it does drawn directly.
- Nothing here names a file or decodes: pixels arrive as textures on the device the compositor was made for.
