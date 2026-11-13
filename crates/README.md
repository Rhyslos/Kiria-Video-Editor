# crates

The Cargo workspace members. One engine with no interface, and thin pages on top: the interface only sends commands to the engine and reads snapshots back, so slow media can never freeze a window.

| Crate | Job | Built in |
|---|---|---|
| [brand](brand/) | The app's name and identity, in one place | 0.1 (done) |
| [diag](diag/) | Explained errors, logging, crash reports | 0.2 (done) |
| [project](project/) | Project model, commands, undo/redo, autosave | 0.3 – 0.4 (done) |
| [jobs](jobs/) | Worker pools, priorities, resource budgets | 0.5 (done) |
| [ui-kit](ui-kit/) | Themes, top bar, shortcuts, shared timeline widget, the mixer's strips | 0.6 (done), 4, 8 |
| [graphics](graphics/) | Which graphics system draws the windows: the shared settings, the start plan, the safety nets around Vulkan and around decoding on the window's device, and opening the editor's Vulkan device for video decoding | 4A |
| [video-device](video-device/) | The window's graphics device as the video decoder shares it, and the copy of a decoded picture into images the viewer draws from | 4A |
| [os](os/) | Small questions to the operating system: the keyboard layout in use, followed live; worker threads below the interface's priority; very small numbers as zero on the sound card's thread; an allocator that counts, for tests | 4, 8 |
| [app](app/) | The editor window and its five pages (binary `editor`) | 0.6 (done), 4 onwards |
| [updater](updater/) | Release checks, signatures, safe installs, rollback | 1 (done), 3 |
| [project-index](project-index/) | The launcher's list of known projects and folders (SQLite) | 2 |
| [launcher](launcher/) | Project browser and update panel (binary `launcher`) | 2 |
| [media](media/) | Probe, demux, decode, encode (FFmpeg wrapper) | 4A and 6.1 (probe, picture and sound decoding; encoding with `media::encode`: containers, codecs and each codec's encoders in the order tried) |
| [cache](cache/) | RAM, GPU and disk caches, proxies, thumbnails, waveforms | 4A (waveforms, thumbnails, proxies done) |
| [audio](audio/) | Real-time mixer, effects, meters, loudness | 8 (effects, master and mixer built; playback and export mix through it; the Audio page built) |
| [render](render/) | GPU compositor (layers into a frame, a frame shown), colour, effects | 4C (compositor done), 7, 9 |
| [engine](engine/) | Playback scheduler, export pipeline and the clarity check | 4A (preview picture, mixing, playback done), 6 |
| [library](library/) | Media index and search: types, labels, lists, notes, use on timelines, full-text search (SQLite, in memory, built from the project); media folders watched for new recordings | 5 (5.1, 5.2 done) |
| [ml](ml/) | Model inference (background removal) | 9 |

Every crate opts into the shared lint rules in the root `Cargo.toml` (`[lints] workspace = true`).
