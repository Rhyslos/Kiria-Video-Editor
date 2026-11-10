# crates

The Cargo workspace members. One engine with no interface, and thin pages on top: the interface only sends commands to the engine and reads snapshots back, so slow media can never freeze a window.

| Crate | Job | Built in |
|---|---|---|
| [brand](brand/) | The app's name and identity, in one place | 0.1 (done) |
| [diag](diag/) | Explained errors, logging, crash reports | 0.2 |
| [project](project/) | Project model, commands, undo/redo, autosave | 0.3 – 0.4 |
| [jobs](jobs/) | Worker pools, priorities, resource budgets | 0.5 |
| [ui-kit](ui-kit/) | Themes, top bar, shortcuts, shared timeline widget | 0.6 onwards |
| [app](app/) | The editor window and its five pages (binary `editor`) | 0.6 onwards |
| [updater](updater/) | Release checks, signatures, safe installs, rollback | 1, 3 |
| [launcher](launcher/) | Project browser and update panel (binary `launcher`) | 2 |
| [media](media/) | Probe, demux, decode, encode (FFmpeg wrapper) | 4A |
| [cache](cache/) | RAM, GPU and disk caches, proxies, thumbnails, waveforms | 4A |
| [audio](audio/) | Real-time mixer, effects, meters, loudness | 4A, 8 |
| [render](render/) | GPU compositor, colour, effects | 4 onwards |
| [engine](engine/) | Playback scheduler and export pipeline | 4A, 6 |
| [library](library/) | Media index and search | 5 |
| [ml](ml/) | Model inference (background removal) | 9 |

Every crate opts into the shared lint rules in the root `Cargo.toml` (`[lints] workspace = true`).
