# Rendering

Turning what is on the timeline at a moment into one picture: every visible clip placed, cropped, scaled, coloured and laid over the ones below it. The viewer shows that picture; export, project pictures and the Color page's scopes will read the same one. Build plan steps 4C "Inspector", "Split view", "Facecam layouts", titles, speed ramps with frame blending, nested timelines, and everything in phases 6, 7 and 9 that touches pixels.

Status: the prototypes are written (`spikes/rendering/`, section 3) and wait to be run on the test machine. Nothing is chosen or built yet.

## 1. Problem

Today the viewer draws one clip: the top visible clip's frame, as it comes from the decoder, converted from YUV to RGB by a small shader while the interface is drawn (`ui_kit::yuv_renderer`). Nothing can be moved, cropped or laid over anything else, the picture has the file's shape rather than the timeline's, and nothing but the viewer can have it.

| Need | Target |
| --- | --- |
| The timeline's picture at preview size, typical edit (one recording, a facecam on top, a title) | composed in well under one frame at 60 a second (16.7 ms), leaving most of it to decoding |
| The same at 144 frames a second (6.9 ms a frame) | still in time, or the preview lowers its size before it drops frames, as now |
| The interface while a picture is composed | never waits: at most about 2 ms of its own thread per frame (working rule, CLAUDE.md) |
| A picture that needs nothing done to it (one clip, filling the frame) | costs next to nothing more than today |
| Preview and export | the same pixels, within a small tolerance (the golden-frame tests of phases 6 and 7) |
| Decoded pictures that are already on the graphics card | stay there: no trip through memory on the way to the screen |
| A machine with no usable graphics card | still shows the picture, slowly (software drawing), and the tests run there |

What a picture is made of, for the Edit page:

- **Layers.** Every visible video track's clip at that moment, bottom track first. Each has a place (position, scale, rotation about an anchor), a crop, an opacity, and quick colour (exposure, contrast, saturation, temperature, tint). Later: effects from the library, a grade (phase 7), a matte (phase 9).
- **The frame.** The timeline's own size (1920×1080, 2560×1440, a vertical 1080×1920, …), not any file's. A clip that doesn't fill it shows black around it unless something lies below.
- **Generated layers.** Titles, solid colours, adjustment layers, another timeline (nested).
- **Two moments of one clip** blended, for slow motion with frame blending.

Not this system's job: decoding and keeping frames (`docs/research/decoding.md`, `caching.md`), which hand it pictures; and encoding (phase 6), which takes them from it.

## 2. Approaches

All of them use wgpu, the library the interface is already drawn with, and shaders in WGSL. A compositor on the CPU was not considered: at 1440p a single full-frame pass in floating point is tens of milliseconds on a CPU. The software drawing drivers (the one Windows ships, llvmpipe on Linux) run the same shaders for machines without a card and for the tests.

The questions that can go more than one way:

### Where the work is done

**A. In the viewer, while the interface is drawn.** Each layer is a rectangle drawn straight onto the window inside the interface's own drawing pass, as the single picture is today.

- Nothing in between: the cheapest possible for simple cases.
- No picture exists anywhere but on the window, at the viewer's size. Export, scopes and project pictures can't have it. Anything that needs the whole composed picture as an input (a blur, a grade over several layers, an adjustment layer, frame blending) can't be done. Blending happens in the window's display colours, which is wrong for scaling and for mixing.
- Kept as the measure of "costs nothing": the others are compared against it.

**B. Into a picture of its own, from the interface thread.** A compositor draws the layers into a texture of the preview's size; the viewer then shows that texture. The interface thread builds the drawing commands when it has a new set of layers (in the paint callback's prepare step, where the single picture's planes are uploaded today).

- A real picture, in any format, that everything can read.
- The interface thread does the work of setting it up for every frame: finding or making textures, filling uniform buffers, building commands. Shaders compiled the first time they are needed stall it. How much is to be measured.

**C. Into a picture of its own, from a worker thread, on the window's device.** The same compositor, but run by a worker (wgpu's device and queue can be used from any thread). The interface is handed a finished texture and only draws one rectangle.

- The interface thread's cost is one rectangle, whatever the picture is made of.
- The worker that decodes can compose: the decoded pictures are already in its hands (on the graphics card for the zero-copy way), and a picture can be composed before the interface asks for it, which is what playback ahead of time and export need.
- Two threads use one queue. wgpu serialises that; whether the interface's frames are disturbed is to be measured.

**D. On a second device.** A compositor with its own device and queue, so the interface's queue is never touched.

- Textures can't be shared between two devices without the operating system's sharing extensions, so every decoded picture and every composed one would be copied across, through memory or through shared handles.
- Rejected without a prototype: it undoes the zero-copy path that was built for decoding, to avoid a contention that C may not have.

### What the picture is made in

**Linear light in 16-bit floating point (`Rgba16Float`).** Layers are converted from their video colours to linear light when they are read, mixed and scaled there, and converted to display colours once, when the viewer (or the encoder) reads the result. This is what the project plan asks for from phase 7 on, and it makes scaling, opacity and blending correct. Eight bytes a pixel: 29 MB for a 1440p picture, 66 MB at 4K.

**Display colours in 8 bits (`Rgba8UnormSrgb` or plain 8-bit).** Half the memory traffic and available everywhere. Mixing in display colours darkens edges and blends; a later grade would work on 8-bit values and band.

The cost of the first over the second is what to measure. If it is small on a real card, floating point from the start avoids building everything twice. On software drawing it will not be small; the preview size is lowered there, as it already is.

### One pass or one pass per layer

**Painter's order in one pass:** every layer is a rectangle with its own textures and uniforms, drawn bottom to top into the same target with blending. One pass, as many draw calls as layers.

**A pass per layer or effect:** needed anyway the moment a layer has an effect that reads its surroundings, or an adjustment layer reads what is below it. More targets, more memory traffic.

Not either-or: a graph that draws runs of plain layers in one pass and breaks into a new target only where something needs it. The measurement is what a break costs, to know how hard to try to avoid one.

### Sources

- wgpu's own documentation on threads (devices, queues and resources are `Send` and `Sync` on native targets), render passes, texture formats and timestamp queries.
- The decoding note in this folder, for how decoded pictures arrive: two plane images on the window's device, planes in memory, or RGBA.
- Recommendation ITU-R BT.709 and BT.1886 for the video colours' conversion to and from linear light; IEC 61966-2-1 for the display's.
- Porter and Duff, "Compositing Digital Images" (1984), for the "over" operation on premultiplied colours.

## 3. Prototypes

`spikes/rendering/` is one program without a window, so it runs the same on a machine with only software drawing and on the test machine (a real card). The "Rendering spike" workflow builds it for Windows.

It makes its own layers (planes like a decoder's, at 1080p, 1440p and 4K: luma and interleaved colour at half size, as the zero-copy path hands them over) and composes 1, 2 and 4 of them, each with a place, a crop and an opacity, into targets of 1080p, 1440p and 4K.

| Measured | How | Decides |
| --- | --- | --- |
| A: layers straight into an 8-bit target the size of a viewer | time on the card per picture (timestamp queries where the card has them, otherwise waiting for the queue) | the "costs nothing" baseline |
| The same layers into `Rgba16Float` in linear light, then one pass to display colours | the same | what floating point costs |
| The same in 8-bit display colours | the same | what it would save |
| One pass for all layers against a new target per layer | the same | what a break in the graph costs |
| B against C: a loop standing in for the interface (a small pass 60 and 144 times a second on the same device), while pictures are composed from that thread, or from another | the loop's frame times: median, worst, how many late | where the compositor runs |
| The interface thread's own time per composed picture in B (building commands, uniforms, bind groups) | wall clock on that thread | whether B can stay under 2 ms |
| The first use of each shader | wall clock | whether pipelines must be made ahead of time, off the interface thread |

Before any timing the program draws a grey picture, and white over it at half strength, through every way, and stops if the grey comes out wrong. That check already shows the difference the format makes: half white over mid grey is 192 of 255 mixed in display colours and 205 mixed in linear light.

With software drawing the numbers only say that the program works (about 90 ms for one 1080p layer into a floating-point frame on two CPU cores, against 35 ms straight to the window). The choice is made from the test machine's numbers.

What does not wait for the measurements, because every approach needs it: where a clip keeps its place, crop, opacity and quick colour in the project (a new format version with its migration), the function that turns the timeline at a moment into a list of layers (pure, tested without a card), and the inspector that edits those values.

## 4. Result

Still to come.
