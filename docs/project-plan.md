# Kiria — Project Plan

> Copied into the repository on 10 November 2026. From now on this file is the one to keep up to date; change it in the same commit as the code it describes.

## Vision and principles

A native, Rust-built video editor for two people that feels fast, is easy to learn, and tells you why when something goes wrong. It is a custom tool designed around our own workflow, not a copy of any existing editor.

Five principles decide every trade-off:

1. **Responsive first.** The UI never waits on media. Decoding, rendering, analysis and export all run off the UI thread; the interface stays at the display refresh rate even when playback has to drop quality.
2. **The machine is yours.** By default the editor uses everything available. Anyone who wants to can cap CPU threads, GPU work, RAM and disk cache by hand.
3. **Explain, don't just fail.** Every error says what happened, why, and how to fix it, with a button that applies the fix when one exists.
4. **Find anything.** One search box reaches every clip, label, effect, setting, command and help topic.
5. **Non-destructive and recoverable.** Source media is never modified. Every edit can be undone, autosave runs continuously, and a crash never costs more than a few seconds of work.

## Working rules

These apply to every page, every feature and every line of code.

1. **Uniform controls.** A tool behaves the same and uses the same shortcut on every page where it appears.
2. **One feel across pages.** Switching pages should feel like moving around one app, not opening another.
3. **Shared visuals stay put.** The top bar, page tabs, panel widths and other shared elements never move or resize between pages.
4. **No name-dropping.** The plan, the code and the app never name other companies or products; we describe features.
5. **Documented folders.** Every folder where it makes sense has a short Markdown file describing its structure and contents. These files and `CLAUDE.md` in the project root are updated with every change and are always current.
6. **Readable, clean, stable code.** The project is open source, so code must be understandable without heavy commenting: clear names, small modules, consistent patterns.
7. **Research and compare for big systems.** For large or advanced systems (playback, caching, rendering, background removal, audio engine), build competing approaches, review them against each other and pick the best. "Works" is not the bar; fast, smooth and robust is.
8. **One name, one place.** The app's name, short name, icon and identifiers live in a single file. Every window title, installer, update check and folder path reads from it, so renaming the app is a one-line change.

## Scope

Version 1 is a complete editor for cutting, grading, mixing and exporting MP4, MKV, MP3, OGG and WAV. Everything else waits until that loop is solid.

### First audience: game recordings

The editor starts out aimed at game footage, so MKV and MP4 recordings get the most attention. That shapes several early decisions:

- **High frame rates.** 60, 120 and 144 fps sources play and cut smoothly; timelines default to the recording's frame rate.
- **Variable frame rate.** Game recorders often save variable frame rate files. Import detects this and conforms the clip to a constant rate, explaining what it did, so audio never drifts.
- **Several audio tracks per file.** Recordings often carry game sound, microphone and voice chat as separate streams. Each one imports as its own timeline track, labelled by its stream name.
- **Long recordings.** Hour-long files import quickly, with thumbnails, waveforms and proxies built in the background.
- **Hardware-encoded sources.** H.264, H.265 and AV1 from graphics-card encoders are the norm and must decode in hardware.
- **Not yet:** camera log footage and HDR. Rec.709 covers version 1.

### In version 1

- **Launcher** that checks for updates, then lists, creates and organises projects.
- **Library** with type tabs, custom lists, labels, filters and a thumbnail grid (audio as waveform rows).
- **Editor** with a single or split viewport, a floating transport bar, and separately scrolling video and audio track areas with per-track and master volume.
- **Color page** for primary grading, curves, qualifiers, LUTs and scopes.
- **Audio page** with one video reference strip, all audio tracks, a mixer, compression, loudness normalisation and clarity tools.
- **Export** with presets, a queue and background rendering.
- **Global search** and **explained errors** throughout.
- **Resource controls** for CPU, GPU, RAM and disk cache.
- **Background removal** that holds up on hair, motion and difficult edges.

### Formats

| Container | Typical codecs inside | Read | Write |
| --- | --- | --- | --- |
| MP4 | H.264, H.265, AV1 video; AAC audio | v1 | v1 |
| MKV | H.264, H.265, AV1, VP9 video; Opus, Vorbis, AAC, FLAC audio | v1 | v1 |
| MP3 | MP3 audio | v1 | v1 |
| OGG | Vorbis, Opus audio | v1 | v1 |
| WAV | PCM 16/24/32-bit float | v1 | v1 |

Still images (PNG, JPEG) come in v1 as well, since titles and overlays need them. Other containers, image sequences and HDR delivery come later.

### Not in version 1

Multi-user collaboration, cloud sync, motion graphics beyond basic transforms and text, multicam, HDR grading, plugin API, mobile.

## Architecture

The editor is a Rust workspace of small crates around one UI-free engine. The UI only sends commands and reads snapshots, so a slow decode or a crashed effect can never freeze or kill the window.

### Crates

| Crate | Job |
| --- | --- |
| `launcher` | Small separate binary: update check, project list, starts the editor with a project path |
| `app` | Windows, pages, panels, input, search palette; draws with the GPU |
| `engine` | Project model, edit commands, undo/redo, playback scheduler, job system |
| `media` | Probe, demux, decode, encode, proxies, thumbnails, waveforms |
| `render` | GPU compositor and effects (transforms, color, keying, masks) in shaders |
| `audio` | Real-time mixer, track DSP, meters, loudness analysis |
| `ml` | Background segmentation and other model inference |
| `cache` | RAM frame cache, GPU texture cache, disk render cache, all budgeted |
| `library` | Media index, labels, search index (SQLite) |
| `diag` | Error types with what / why / fix, logs, crash recovery |

### Recommended stack

| Area | Choice | Why |
| --- | --- | --- |
| GPU | `wgpu` | One API over Vulkan, Metal and DirectX 12; compute shaders for effects |
| UI | `egui` on `wgpu`, custom widgets for timeline, viewport and scopes | Fast to build, easy to theme, renders into the same GPU context as the video |
| Video I/O | FFmpeg via `ffmpeg-next`, dynamically linked shared build | Covers every v1 container and codec, plus hardware decode and encode on all three GPU vendors |
| Audio decode | `symphonia` (pure Rust) for MP3, OGG, WAV | No FFI for the audio-only formats; FFmpeg handles audio inside video files |
| Audio output | `cpal` + lock-free ring buffers | Low-latency output on every OS |
| Inference | `ort` (ONNX Runtime) with CUDA, DirectML and CoreML providers | Runs segmentation models on the GPU on every platform |
| Parallel work | `rayon` thread pools with configurable size, `crossbeam` channels | Directly maps to the CPU allocation setting |
| Data | `rusqlite` for library and search, versioned RON files for the timeline | Library queries stay fast at thousands of clips; timeline files stay diffable |
| State | Persistent data structures (`imbl`) | Cheap snapshots make undo, autosave and background rendering safe |
| Search | `nucleo` fuzzy matcher | Instant ranking across clips, commands and settings |

### How playback works

The audio device clock is the master clock. The scheduler asks the cache for the frame at the current time; on a miss it decodes ahead on worker threads, composites on the GPU, and hands the UI a texture. If a frame is late, playback drops preview resolution before it drops frames, and the viewport says so.

## Performance and resources

Smooth playback comes from three layers working together: budgeted resources, layered caches, and preview quality that degrades gracefully instead of stuttering.

### Resource allocation

Every control has an **Auto** default and a manual override. Settings show live usage next to each cap so people see the effect immediately.

| Resource | What the user sets | How it is enforced |
| --- | --- | --- |
| CPU | Worker thread count, optional core pinning, process priority | Sizes the decode, render and export thread pools; one thread is always kept for the UI |
| GPU | Which GPU (on multi-GPU machines), VRAM budget, GPU work share during playback vs export | Limits frames in flight and texture cache size; separate queues for preview and background jobs |
| RAM | Frame cache size in GB | Hard cap on the RAM frame cache; least-recently-used frames are evicted first |
| Disk | Cache location and size | Render cache and proxies live there; clearing is one button |
| Background jobs | Pause while playing / always run / only when idle | Proxies, analysis and pre-render yield to playback |

Note: no OS lets an app reserve an exact GPU percentage. The GPU setting is a work budget the editor enforces on itself, and the settings page explains this.

### Caching layers

1. **Thumbnails and waveforms**, built on import and stored on disk.
2. **Proxies**: lower-resolution, edit-friendly copies generated in the background. Toggle per clip or globally; export always uses originals.
3. **RAM frame cache** of decoded frames around the playhead, filled ahead in the playback direction.
4. **GPU texture cache** for frames the compositor touches repeatedly.
5. **Render cache**: timeline sections with heavy effects (grades, background removal) are pre-rendered in the background. Each section is keyed by a hash of its inputs, so a change only re-renders what it touched. The timeline shows cached sections as a green bar and pending ones as grey.

### Preview quality settings

- Preview resolution: Full, 1/2, 1/4, or Auto (drops during playback, full when paused).
- Effects quality: Draft or Full. Draft uses cheaper keying and grading paths.
- Proxy mode: Off, On, Auto (on for files heavier than the machine can decode in real time).
- A small indicator in the viewport always shows the current quality and why it changed.

### Targets

- UI at display refresh rate under load.
- Real-time playback of a 4K H.264 timeline with a grade, on a mid-range GPU, using proxies if needed.
- Scrubbing shows a frame within 50 ms of the cursor stopping.
- Cold start to the launcher under 1 second; project open under 3 seconds for a typical project.

## Pages and UX

The app has five pages (Library, Edit, Color, Audio, Export) switched from tabs in the top bar, plus a launcher before any project opens. Pages share one project state, so switching is instant.

### Launcher

- Checks GitHub for an update first, with a short timeout; offline never blocks opening.
- App name and logo centred in the top bar. Left side: a folder tree of project folders, media folders and any other folders added. Main area: projects in a three-wide grid of thumbnails, each showing length, settings, when it was last used and any issues (missing media, saved by a newer version), with search, Open and New project. Bottom left: the app version with manual Update, Skip this version, and an Automatic updates tick box.
- New projects start on **Automatic** settings: the first video imported decides the size, frame rate and audio rate, so any resolution and frame rate works without choosing anything. Templates (such as Game clip, High frame rate, Vertical short) are optional, for fixing the settings up front, and the settings can be changed later.
- Right-click a project to rename it, move it to another folder or delete it. The files that belong to a project (autosave, backups, thumbnail, timeline layout, saved versions) go with it. Deleting asks first with the list of files and moves them to the recycle bin, so it can be undone; recordings are never touched.
- Missing-media warnings shown before a project opens.

### Library

Media is organised by **type** (Video, Audio, Image, Effect) and by any number of **labels** (B-roll, Drone, Interview, ...). Types and labels together drive filters; nothing is locked into one folder.

- **Layout**: same top bar as the editor; labels, smart filters and Import on the left; a detail panel on the right (preview, mark in/out, add to timeline, metadata, labels, lists, notes).
- **Tab bar**: Video, Audio, Images and Effects, then the user's own **custom lists** (for example "Opening sequence", "Unused drone"). Lists are created with "+ New list"; when there are too many, the bar fades out and scrolls.
- **Filter row** under the tabs: active label filters as removable chips, group by, sort, thumbnail size, and a grid / list switch.
- **Video, Images, Effects**: a thumbnail grid grouped by label, with a coloured edge per label and badges for "In timeline" and "Needs proxy".
- **Audio**: rows, not thumbnails. Each row shows a play button, the file name, a full-width waveform in the label colour, format, whether it is used in the timeline, and duration. Rows are grouped by label (Music, Voice, SFX). The detail panel shows a large waveform, loudness and true peak, and explains problems such as peaks near clipping with a one-click fix.
- Import checks every file and explains anything unsupported before it reaches the timeline.
- **Prune**: one button in the sidebar removes all media not used in any timeline from the project. It shows the list first, can be undone, and never deletes files from disk.

### Edit

The agreed layout is in the [mockups](https://claude.ai/artifact/BoWqShTxNag7Q1yD8BoKLr) (the Editor row).

- **Top bar**: project name on the left; page tabs centred over the viewport, in line with the play button, and in the same spot on every page; on the right a Search button that opens the pop-up palette (Ctrl K), the live CPU / GPU / RAM readout (opens resource settings), and a settings cog.
- **Media pool**: a thin tab on the left edge, beside the viewer (the timeline under it runs the window's whole width), slides out a compact copy of the library over the page (nothing underneath moves), with its filters, so clips can be dragged straight onto the timeline. The same drawer has a Media / Effects switch: on Effects it becomes the effects library, a full-height panel that slides over the picture (nothing underneath moves). A vertical list of groups sits on the left (Favourites, Recently used, Video transitions, Audio transitions, Titles, Motion, Keying, Stabilise, Blur and censor, Stylise, Overlays, Audio effects); picking one shows its effects on the right as larger rows with a preview thumbnail, grouped under collapsible headings. Effects are dragged onto a clip or a cut, and hovering one previews it on the selected clip. A setting switches the rows to a grid of bigger tiles. Effect settings appear in the right-hand panel and are tuned on the picture wherever possible (drag a box to aim a zoom, drag a ramp to set timing). There is no separate Effects page.
- **Viewport** in the centre. The row under the picture holds a **Display** menu (single or split view, the viewer in its own window, full screen, timeline Full or Rough cut, compact tracks, the divider between video and audio tracks, effects quality, proxies, background, overlays such as timecode, cache status, safe areas and clipped whites) beside an always-visible **Preview resolution** dropdown. Zoom sits top right.
- **Split view** shows **Source** (raw clip, no effects) next to **Program** (final result with grades, keying and effects). It is off by default and switched on from the Display menu.
- **Transport**: a low, squircle-shaped bar floating at the bottom of the viewport. Play/pause sits at the exact horizontal centre of the viewport. Left of it: In, Out, play reverse, frame back. Right of it: frame forward, play forward, stop, loop.
- **Pause** returns the playhead to where playback started, so a section can be re-watched instantly. **Stop** leaves the playhead where it is.
- **Inspector** on the right, beside the viewer (the timeline under both runs the whole width of the page); with no clip selected it describes the timeline (size, frame rate, length, tracks) with its settings a click away: collapsible sections (Transform, Crop, Quick color, Speed, Effects, Audio; the Effects section lists what is applied from the Effects library and opens each one's settings), each showing a one-line summary in its header so settings are readable without opening them. Keyframe diamonds sit next to animatable fields: hollow when the value doesn't change over time, filled on a keyframe at the playhead, hollow with a dot between keyframes; a click adds or removes the keyframe at the playhead, arrows beside it jump to the previous or next one, and the selected clip shows its keyframes as small diamonds on the timeline, one per frame with keyframes: a click goes there, a drag moves it a frame at a time (snapping to the playhead and the other diamonds; Shift-click chooses several to move together), its tooltip names the values keyed there, and its right-click menu removes them or chooses how they go on. Keyframes stay on the picture they were set on (they are kept in the clip's own time), go linearly, hold, or ease in and out, and a value changed with keyframes on becomes a keyframe at the playhead. The selected clip's picture can also be taken hold of in the viewer itself: an outline with a handle on each corner, a bar on each side and a round turn handle above it: drag to move (it snaps to the frame's centre and edges), drag a corner to resize, a side to crop (the cut-off part shows faintly while dragging), the turn handle to turn (Shift for steps of 15°), double-click to fit; the inspector's numbers follow.
- **Timeline bar** above the tracks: sequence name, current timecode, the editing tools as icons (select, blade, ripple, roll, slip, slide, text) with tooltips and shortcuts, magnetic snapping, render-cache progress and timeline zoom.
- **Timeline**: always starts at 00:00:00:00. A red edit needle, separate from the playhead, marks where cuts, splits, pastes and inserts happen. Drag it anywhere with the mouse, so you can watch one clip while cutting another; a setting (also in the Display menu) chooses what a click on the timeline moves: the playhead, as usual, or the edit needle, with the playhead then dragged by its handle; its timecode shows next to the playhead's. It works the same on every page with a timeline. A setting ("Edit at the playhead") merges the two needles for anyone who prefers a single one. Dragging in the time ruler at the top marks a region: a muted bar between two filled half-circle handles that point away from each other, while everything outside the region is darkened. The region can loop playback inside it (L), pre-render only that stretch, and be chosen as the range on the Export page; it is shared by every page with a timeline. The bottom scroll bar stays for scrolling and zooming only. Right-clicking the timeline places a timeline label (M), a named, coloured marker in a thin Labels row under the ruler for organising long edits; the region can be labelled as a whole too. Like the region and the edit needle, labels are shared by every page with a timeline. A Rough cut switch in the Display menu turns the timeline into one strip per clip, picture and sound together, for fast first cuts; switching back to Full shows every track again. There is no separate cut page. Video and audio tracks are two areas with a shared ruler, separate vertical scroll and a draggable divider. For productions with many tracks the Display menu has **compact tracks** (every track a thin strip that still shows each clip's colour and name, so twenty video and twenty audio tracks can be seen at a glance; the heights given to tracks come back when it is turned off) and can take the **divider** away (video and audio tracks then form one list that scrolls together; the divider comes back where it was). Both are ways of working, kept in settings for every project, not edits. There can be any number of tracks of each kind: higher video tracks cover lower ones, a right-click on a track's name adds, renames, hides or removes tracks, and a clip dragged up or down moves to another track of its kind, or past the last one to a new track. Putting one picture over another never costs the sound already there: a clip's or a dropped file's sound goes to an audio track with room, and a track is made when there is none. A horizontal scroll bar along the bottom outlines the visible section.
- **Clip colours**: video clips share one blue fill with a coloured top edge for their label (Drone, Interview, B-roll...); each audio track has its own colour for clips and waveforms.
- **Volume**: each audio track header has a horizontal fader, mute and solo. The **master** is one tall column to the right of the timeline, spanning both video and audio tracks. Its level meter is also the fader: a handle sits on top of the live level, with a dB scale and LUFS / true-peak readout.

### Look and feel

Two themes ship in version 1, switched in Settings. The interface itself stays neutral black and grey; colour is reserved for content and status.

| Theme | Base | Accent | Status |
| --- | --- | --- | --- |
| Ash | Soft graphite greys (base #1A1B1C) | Bone off-white | Default |
| Carbon | Warm near-black (base #0D0C0B) | Copper | Optional, in Settings |

- The accent marks the playhead, selection, active tool and primary button only.
- Green always means cached or ready; amber means a warning; red means a blocking error. The one exception is the red edit needle on the timeline.
- Inter for interface text, a monospace face for every timecode and number readout.

### Color

A prototype layout is agreed; detailed colour work waits until we reach that phase and know what we need.

- **One tool at a time.** A switcher under the picture picks Basic, Wheels, Curves, Color range, Masks or Looks; only that tool is shown. Never all tools at once.
- **Picture and one scope.** Large viewport with Compare and Display menus; a single scope on the right (waveform, RGB parade, vectorscope or histogram, chosen from a dropdown) with a one-line plain reading such as "nothing is clipped".
- **Layers** are a single dropdown ("Layer 1 · Base correction") instead of a visible stack.
- **Wheels: Simple and Advanced.** A toggle next to the layer dropdown switches the wheel panel.
  - Simple (default): white balance on the left (pick something white, Auto, temperature, tint), the four wheels in the middle, each with its own brightness slider and reset, and tone on the right (contrast, pivot, saturation, highlights, shadows).
  - Advanced: larger wheels spread across the full width, each with exact R, G, B and brightness values that can be typed in.
- **Plain names**: wheels are Shadows, Midtones, Highlights and Overall.
- **Timeline**: video tracks keep individual clips (a dot marks graded clips); all audio collapses into one view-only bar. The bar shows which clip is being graded, an "Apply to" choice (this clip, all clips with a label, whole timeline), Copy grade, Paste, Match to and Reset.
- **Transport** steps clip by clip as well as frame by frame.
- Works in both themes.

### Audio

One layout, built to scale to dozens of tracks.

- **Top left: video preview** with transport, so sound can be adjusted against what is on screen.
- **Top right: effects for the selected track.** A track dropdown plus a one-at-a-time switcher: EQ, Compressor, Clean-up, Ducking, Loudness. Each tool shows its graph, its settings and one plain sentence on what it is doing ("turns down Ingrid's loudest words by about 4 dB").
- **Middle: clarity check**, full width. It shows where tracks share the same frequencies at the playhead, highlights muddy zones, explains the problem in plain words and offers one-click fixes (lower music under dialogue, or cut music at 300 Hz).
- **Bottom: a toggle between Timeline, Mixer and Split.**
  - Timeline: one video reference row plus audio tracks with waveforms and volume lines; scrolls vertically.
  - Mixer: one vertical strip per track, each a level meter that doubles as the fader, with a box for an exact dB value, effect tags, mute and solo. Strips scroll sideways; the master stays pinned on the right.
  - Split: both at once, with a direction switch. Side by side is the default (mixer strips on the left, timeline on the right, both at full height); stacked puts a few timeline tracks above the mixer row.
- A **Show** filter (all, dialogue, music, SFX and foley, ambience) narrows both views when a project has many tracks.

### Export

A full page, reached from the Export tab. Everything is visible at once.

- **Presets** on the left: Web 1080p, Web 4K, Vertical, Archive master, Small preview, Audio only (WAV, MP3/OGG), plus saving your own.
- **Settings** in the middle: file name, folder, range (whole timeline, in to out, or the timeline region), then Video (container, codec, resolution, frame rate, quality, graphics-card encoding) and Audio (codec, bitrate, sample rate, loudness normalisation, optional separate WAV).
- **Checks** before export: missing media, loudness and peaks, unrendered effects (with a Render now button), disk space.
- **Summary** on the right in plain words: size, time on this machine, what it plays on, free disk space, then Export or Add to queue.
- **Queue** along the bottom: exports run in the background with progress, time left, pause and cancel, so editing can continue.

### Keyboard shortcuts

The first agreed shortcuts. They work the same on every page with a timeline; the rest are decided while building the app.

| Key | What it does |
| --- | --- |
| Space | Play / pause |
| S | Split at the edit needle. With a clip selected, splits only that clip's video and its linked audio; with nothing selected, splits every track. |
| Delete | Deletes the selected clip and leaves the gap. |
| Backspace | Deletes the selected clip and closes the gap: everything after it moves back to fill the space. |
| M | Places a timeline label at the edit needle. |

Shortcuts already shown in the mockups but not yet confirmed: Ctrl K search, L loop the region, R set region from selection, Shift M label the region, N magnetic snapping.

**Keyboard shortcuts page.** A visual keyboard showing every shortcut, opened from Settings, or from the search palette by typing "shortcuts", "keybinds", "hotkeys" or "keys".

- Top: a search box ("Find a key or action") and a keyboard-layout dropdown.
  - **Layout is detected automatically.** The app reads the active keyboard layout from the operating system (Windows and Linux) and draws that keyboard: the right shape (for example the tall Enter and extra key by Shift on Norwegian and other ISO keyboards, the wide Enter on US keyboards) and the right printed characters (Æ Ø Å on Norwegian, Arabic or Cyrillic letters, and so on). It follows a layout switch while the page is open. The dropdown is only a manual override.
  - **Shortcuts work on every layout.** Shortcuts are tied to the physical key position, so S splits from the same spot on every keyboard. On non-Latin layouts each key shows both its own character and the shortcut letter, so "S" can still be found.
- Coloured context chips that also act as filters: Anywhere, Edit page, Timeline, Viewer, Color page, Audio page, While dragging, Menus and dialogs. A chip appears once something lives in its context (today: Anywhere, Viewer, Timeline, While dragging).
- The keyboard: every key that does something shows small coloured dots, one per context it is used in; keys that do nothing are dimmed. Holding Shift, Ctrl or Alt redraws the keyboard for that combination.
- Pointing at a key lists what it does in each context in a panel beside the keyboard; clicking a key keeps that list shown. Searching for an action lights up its key.
- Under the keyboard: a Mouse section listing click, drag, wheel and modifier-click actions with the same context dots.
- Changing a shortcut happens here too: pick an action, press the new key, and any clash is shown before it is saved. The same data prints as a cheat sheet.
- Built from the one shortcut table every page reads, so it can never be out of date.

### Extra features

Agreed additions beyond the core pages. Each one is built in the phase of the page it belongs to.

**For game recordings**

- **Highlight finder:** scans long recordings for loud or busy moments (mic shouts, game-audio spikes, scene changes) and suggests timeline labels there. Works for any game because it only listens and looks.
- **Remove silence:** finds dead air on the mic track and cuts or shortens it, showing what it will remove first.
- **Facecam layouts:** one-click picture-in-picture, and a vertical layout with the facecam stacked above the game.
- **Slow motion and speed ramps:** clean slow motion from 120 and 144 fps sources, with smooth ramps in and out.

**Editing**

- **Nested timelines:** group clips into one block that can be moved, graded or reused as a single piece.
- **Adjustment layers:** one effect or grade above a stretch of the timeline that applies to everything below it.
- **Several timelines per project:** for example a long cut, a short cut and a vertical cut from the same media.
- **Saved versions:** named snapshots of the project to return to, beyond normal undo.
- **Templates:** reusable intros and outros that drop into any project.
- **Thumbnail grab:** save any frame as a full-quality image.

**Sharing and comfort**

- **Package project:** the project plus only the media it uses, in one folder that opens on the other person's machine without missing files.
- **Sync by sound:** line up separately recorded audio with the game recording by matching waveforms.
- **Shortcut editor:** change any shortcut on the Keyboard shortcuts page, with a printable cheat sheet.
- **Second screen, scaling and cache:** pop the viewer out to another monitor or full screen, interface scaling for high-resolution screens, label colours that stay distinct for colour-blind users, and cache cleanup per project.

### Search everywhere

The Search button in the top bar (or Ctrl K) opens a pop-up palette that searches clips, labels, timeline labels, effects, commands, settings and help. Choosing a timeline label jumps the playhead to it. Choosing a setting opens its page and highlights the field. Results show the shortcut for each command, so search also teaches the app.

### Explained errors

Every error is one structured type with four parts: what happened, why, how to fix it, and an optional one-click fix. Examples:

- "This clip plays slowly. It is 10-bit H.265 and your GPU can't decode that format. Fix: create a proxy (one click), or lower preview resolution."
- "Export can't start. Two clips are offline. Fix: relink media (opens the relink dialog with both files listed)."

Errors appear inline next to the thing they affect, never as a blocking pop-up unless data is at risk. A Diagnostics panel lists everything in the session with copyable details for bug reports.

## Effects

Version 1 ships three effect families and does each one properly: color grading, background removal, and audio clarity.

### Color grading

- All processing in 32-bit float, linear light, on the GPU. Input transform per clip (Rec.709, sRGB, camera log profiles later), output to Rec.709.
- Primary controls: exposure, contrast, pivot, white balance (temperature/tint), lift / gamma / gain / offset wheels, saturation.
- Auto white balance (the Auto button on the Color page and in Quick color). It works in linear light on a handful of frames spread across the clip, not just the current one, so a single odd frame can't throw it off. It ignores clipped highlights, near-black pixels and strongly saturated colours, then estimates the light's colour from what is left, combining a neutral-average estimate with the brightest near-neutral areas. The result is written into the ordinary temperature and tint sliders, so it can be fine-tuned or reset like any manual change. A strength slider allows a partial correction, and a "pick something white" eyedropper overrides it. Game footage is often colour-graded on purpose, so Auto never runs by itself; it is mainly for facecam and camera clips. Following the research-and-compare rule, two or three methods (simple statistical ones and a small learned model) are tested on our own facecam and game clips before one is chosen.
- Curves: RGB, per-channel, hue vs hue, hue vs saturation, luma vs saturation.
- HSL qualifier with soft edges, plus simple shape masks (ellipse, rectangle, freehand) with tracking later.
- 3D LUT import (.cube) with intensity slider.
- Scopes computed on the GPU from the real output, not the preview-scaled image.
- Copy, paste and match grades between clips.

### Background removal

The goal is clean edges on hair and fine detail, no flicker between frames, and the ability to fix the result by hand.

1. **Model**: a video matting model that uses previous frames (temporal memory) for stable edges, run through ONNX Runtime on the GPU. Evaluate two or three open models on our own footage before choosing.
2. **Refinement pass**: a second, high-resolution edge model or guided filter on the matte boundary, so edges hold at 4K.
3. **Temporal smoothing** of the matte to remove flicker, with a strength slider.
4. **Manual correction**: add/remove brushes and keyframed masks that steer the model.
5. **Controls**: edge softness, choke/expand, spill suppression, and a matte view (black and white) to check quality.
6. **Pre-processing**: mattes are computed in the background once and stored in the render cache, so playback is real-time afterwards.

Acceptance test: a set of our own difficult clips (hair against busy backgrounds, fast motion, low light) must produce no visible flicker and clean hair edges at full resolution.

### Audio clarity

- **Compressor** per track and on the master: threshold, ratio, attack, release, knee, makeup gain, gain-reduction meter, and plain presets ("Voice, gentle", "Voice, broadcast").
- **Loudness normalisation** to a target (for example -14 LUFS for web, -23 for broadcast), measured with integrated, short-term and true-peak meters. Per clip and for the whole mix.
- **Limiter** on the master to prevent clipping.
- **Clarity tools** so tracks don't turn muddy together:
  - High-pass filter per track with a sensible default for voice.
  - Simple parametric EQ with a frequency analyser overlay.
  - **Ducking**: music automatically lowers under dialogue (sidechain), with depth and speed controls.
  - **Overlap view**: highlights frequency ranges where two tracks pile up and suggests a cut on one of them. This is what makes the audio page easy to learn.
- All DSP runs on a real-time audio thread with no allocations or locks; heavy analysis (loudness scans, overlap view) runs on worker threads.

## Install and updates

The launcher owns updates: it checks GitHub Releases before opening the editor, installs a newer signed build if there is one, and falls back to the current version if anything goes wrong.

### Update flow

1. Launcher starts and asks the GitHub Releases API for the latest version on the chosen channel (Stable or Beta), with a 3-second timeout.
2. If newer, it shows the release notes and downloads in the background (or immediately, if auto-update is on).
3. It verifies the download's **ed25519 signature** against a public key built into the launcher. An unsigned or tampered build is refused, with an explanation.
4. The new version is unpacked next to the current one, never over it. The launcher switches to it on the next start.
5. If the new editor fails to start twice, the launcher rolls back and says why.
6. Offline or GitHub unreachable: the launcher opens normally and retries later.

The launcher updates itself in a separate, rarer step so it can always recover the editor.

### Packaging

| OS | Package | Notes |
| --- | --- | --- |
| Windows | MSI or setup.exe, per-user install | Code-signing certificate avoids SmartScreen warnings |
| macOS | Signed and notarised .dmg | Required to open without security prompts |
| Linux | AppImage (Flatpak later) | Bundles FFmpeg libraries |

Windows comes first and Linux second; macOS waits until later. Builds, signing and GitHub release uploads run from CI on every tagged version. `cargo-dist` can generate most of this pipeline.

### Licensing to settle early

- Kiria is licensed **GPL-3.0-or-later**, so either an LGPL or a GPL build of FFmpeg may be used, including GPL-only encoders. FFmpeg stays dynamically linked so it can be updated on its own, and its licence text ships with the app.
- H.264 and H.265 carry patent licensing. Using the operating system's and GPU's built-in encoders and decoders where possible keeps this simpler; AV1, VP9, Opus and Vorbis are royalty-free.

## Robustness

Robustness comes from isolating failures, never losing work, and testing against real media on every commit.

### Design rules

- **No work is lost.** Autosave every 30 seconds and after each significant edit to a journal file; the project file is written atomically (write new file, then rename). On restart after a crash, the launcher offers recovery.
- **Failures stay local.** Each decode, effect and model call runs inside a guarded job. A broken file or a crashing effect marks that one clip as failed, with an explanation, and the rest of the project keeps working.
- **Unsafe code only at the edges.** `unsafe` is limited to the FFmpeg and GPU FFI wrappers and the small crate of operating-system calls (`crates/os`), which are small and heavily tested.
- **Media is referenced, never modified.** Clips are stored by path plus a content fingerprint, so moved files can be relinked automatically.
- **Versioned project format** with migrations, so old projects always open in new versions.
- **Hardware fallbacks.** Hardware decode falls back to software; GPU inference falls back to CPU; each fallback is shown, not hidden.

### Testing

| Test type | What it checks |
| --- | --- |
| Unit and property tests | Timeline edit commands keep the project valid under random sequences of edits and undos |
| Golden-frame tests | Rendering a reference project produces the same frames (within tolerance) on every OS |
| Media corpus | A folder of real and broken files in every v1 format must import, play and export |
| Fuzzing | Demuxing and project-file parsing survive random and corrupted input |
| Audio tests | Loudness, compressor and limiter output match reference values |
| Benchmarks | Decode, composite and export speed tracked per commit; regressions fail CI |
| Manual smoke test | One short script per release: import, cut, grade, key, mix, export |

CI builds and tests on Windows and Linux for every pull request. The first test machines are an AMD RX 7900 XT (hardware H.264, H.265 and AV1 encode and decode) and an NVIDIA card, so both major GPU vendors are covered from the start.

### Risks

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Hardware decode differs across GPUs and drivers | Stutter or failures on some machines | Software fallback, proxies, a compatibility check on first run |
| Background removal quality falls short | Headline feature disappoints | Test models on our footage early (phase 2 spike); manual correction tools |
| Rust GUI widgets are immature for a timeline | UI work takes longer | Build the timeline as one custom GPU widget from the start |
| Codec licensing | Distribution problems | GPL-compatible FFmpeg build, hardware codecs, checked before the first public release |
| Scope creep | Never ships | Phase gates below; anything new goes to a later list |
| Color correctness | Grades look different after export | Float pipeline, golden-frame tests that compare preview and export |

## Roadmap

Build in ten steps; each ends at a gate we can test before moving on. The step-by-step version with checklists and gates lives in the [Build Plan](build-plan.md). Step 0 comes first because the project file must be versioned from day one for updates to stay safe.

Explained errors and resource limits are built into the engine from Phase 0, even though their full UI arrives later; retrofitting them is much harder.

## Decisions

- **Platforms:** Windows first, then Linux. macOS later, if ever.
- **Test machines:** an AMD RX 7900 XT and an NVIDIA card, so both GPU vendors are covered from the start.
- **Open source:** yes, under GPL-3.0-or-later. Anything distributed that is based on it must stay open under the same licence.
- **Footage:** game recordings first, so MKV and MP4. No camera log or HDR footage for now; Rec.709 is enough.
- **Name:** Kiria. It lives only in `crates/brand`, so changing it later is a one-line change.
