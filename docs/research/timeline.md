# Timeline widget

The widget every page with a timeline uses: tracks, clips, ruler, playhead, edit needle, labels, scrolling and zooming. Build plan step 4B "Timeline widget".

## 1. Problem

| Need | Target |
| --- | --- |
| One frame of the timeline on the interface thread | well under 2 ms, whatever the project's size |
| A 20-minute recording cut into hundreds of clips, or a long project with thousands | the same cost as a short one |
| Smooth zoom from the whole project down to single frames | no stutter at any zoom |
| Video and audio areas that scroll separately, a draggable divider, shared ruler | as specified in the project plan |

## 2. Approaches

**A. egui's painter, drawing only what is visible.** One custom widget allocates the whole area and handles all its own input. Each frame it finds the clips inside the visible time range and track rows, and adds rectangles and text to egui's shape list. egui tessellates them on the CPU and its wgpu renderer draws them. Clips too narrow to tell apart (a few pixels, when zoomed far out) are merged into one shape per run.

- Simple, all in safe Rust, and it uses the same text, theme and input as the rest of the interface.
- Cost grows with what is *visible*, not with the project.

**B. A custom wgpu renderer with instanced rectangles.** Clip bodies go to the GPU as one instance buffer and are drawn in one call through an egui paint callback; text stays on egui's painter.

- Fastest for very many visible shapes.
- More code (shader, buffers, paint-callback plumbing, hit testing kept in sync), and text still goes through egui.

**C. Stacking ordinary interface widgets (one per clip).** Rejected without a prototype: every clip would carry layout and interaction state, and the cost grows with the project.

## 3. Prototypes

A was built as the real widget (`crates/ui-kit/src/timeline.rs`) and measured with `crates/ui-kit/tests/timeline_cost.rs`. That test runs 60 frames of a 1920×400 timeline with 4 tracks, then lays out, paints and tessellates each frame, release build, on the 2-thread cloud machine:

| Clips in the project | View | Median ms per frame | Worst ms |
| ---: | --- | ---: | ---: |
| 100 | default zoom, following the playhead | 0.05 | 0.51 |
| 1,000 | default zoom | 0.05 | 0.21 |
| 10,000 | default zoom | 0.06 | 0.36 |
| 100 | zoomed out, all on screen | 0.11 | 0.21 |
| 1,000 | zoomed out, all on screen | 0.54 | 1.22 |
| 10,000 | zoomed out, all on screen | 0.07 | 0.22 |

Before tiny clips were merged, 10,000 clips on screen cost 2.05 ms median (4.37 worst), above the budget. Merging brought that to 0.07 ms. The 1,000-clip zoomed-out case is the costliest left: each clip is still a few pixels wide, so it gets its own shapes.

B was not built: A stays far inside the budget in every case measured, so the extra code isn't justified yet.

Measured again after tracks got heights of their own, clips their reading progress and the ruler its cache bar. The test now also passes a cache range under every clip of one track, in three states taking turns, which is the cache bar's worst case when everything is on screen: a different state on almost every pixel. Same machine, median milliseconds per frame: one run before the work, three after (the machine was shared, so single runs move by a few hundredths and the worst frames by more):

| Clips in the project | View | Before | After |
| ---: | --- | ---: | ---: |
| 100 | default zoom, following the playhead | 0.06 | 0.06 |
| 1,000 | default zoom | 0.06 | 0.06 |
| 10,000 | default zoom | 0.07 | 0.07 to 0.12 |
| 100 | zoomed out, all on screen | 0.11 | 0.12 |
| 1,000 | zoomed out, all on screen | 0.53 | 0.54 |
| 10,000 | zoomed out, all on screen | 0.10 | 0.15 |

Rows of mixed heights cost nothing that can be measured: they are laid out once a frame, per track, not per clip. The cache bar finds the ranges in view by bisection, gives ranges too thin to tell apart one shared span and draws all spans as one mesh, not a shape each.

## 4. Result

**A: egui's painter with culling and merging of tiny clips.** If waveforms and thumbnails later push the cost up, they will be drawn from cached textures (one image per visible stretch, not per clip). B remains the fallback if a future measurement shows A can't keep up.
