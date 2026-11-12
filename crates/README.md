# crates

The Cargo workspace members. One engine with no interface, and thin pages on top: the interface only sends commands to the engine and reads snapshots back, so slow media can never freeze a window.

| Crate | Job | Built in |
|---|---|---|
| [brand](brand/) | The app's name and identity, in one place | 0.1 (done) |
| [diag](diag/) | Explained errors, logging, crash reports | 0.2 (done) |
| [project](project/) | Project model, commands, undo/redo, autosave | 0.3 – 0.4 (done) |
| [jobs](jobs/) | Worker pools, priorities, resource budgets | 0.5 (done) |
| [ui-kit](ui-kit/) | Themes, top bar, shortcuts, shared timeline widget | 0.6 (done), 4 |
| [graphics](graphics/) | Which graphics system draws the windows: the shared settings, the start plan, the safety nets around Vulkan and around decoding on the window's device, and opening the editor's Vulkan device for video decoding | 4A |
| [video-device](video-device/) | The window's graphics device as the video decoder shares it, and the copy of a decoded picture into images the viewer draws from | 4A |
| [os](os/) | Small questions to the operating system: the keyboard layout in use, followed live | 4 |
| [app](app/) | The editor window and its five pages (binary `editor`) | 0.6 (done), 4 onwards |
| [updater](updater/) | Release checks, signatures, safe installs, rollback | 1 (done), 3 |
| [project-index](project-index/) | The launcher's list of known projects and folders (SQLite) | 2 |
| [launcher](launcher/) | Project browser and update panel (binary `launcher`) | 2 |
| [media](media/) | Probe, demux, decode, encode (FFmpeg wrapper) | 4A (probe, picture and sound decoding done) |
| [cache](cache/) | RAM, GPU and disk caches, proxies, thumbnails, waveforms | 4A (waveforms done) |
| [audio](audio/) | Real-time mixer, effects, meters, loudness | 4A, 8 |
| [render](render/) | GPU compositor (layers into a frame, a frame shown), colour, effects | 4C (compositor done), 7, 9 |
| [engine](engine/) | Playback scheduler and export pipeline | 4A (preview picture, mixing, playback done), 6 |
| [library](library/) | Media index and search | 5 |
| [ml](ml/) | Model inference (background removal) | 9 |

Every crate opts into the shared lint rules in the root `Cargo.toml` (`[lints] workspace = true`).
