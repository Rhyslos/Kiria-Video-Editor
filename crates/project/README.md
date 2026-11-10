# project

The project model, edit commands, undo/redo and autosave.

## Status

Model and project file done (Phase 0.3). Edit commands, undo/redo and autosave come in Phase 0.4.

## Files

| File | What it holds |
|---|---|
| `src/time.rs` | `Time` (exact seconds as a reduced fraction) and `Rate` (frames per second as a fraction, e.g. 30000/1001). Frame conversion, snapping, timecode |
| `src/ids.rs` | One id type per kind of thing (`ClipId`, `TrackId`, ...). Ids come from one counter in the project and are never reused |
| `src/model.rs` | `Project`, `Sequence` (a timeline), `Track`, `Clip`, `MediaRef` + `Fingerprint`, `TimelineLabel`, `Region`, the edit needle. `Project::problems()` / `validate()` check the rules |
| `src/file.rs` | The project file: save (atomic), load, the summary block, format versions and migrations |
| `tests/samples.rs` + `tests/samples/` | One stored project per released file format; all must keep opening |

## Rules the model keeps

- Time is never a float. Use `Time` and `Rate`; convert to `f64` only to draw or display.
- Clips on a track are in start order and never overlap; every clip's media exists; lengths are positive; a region ends after it starts. `Project::validate()` checks this on every save and load.
- Collections are `imbl` persistent structures: cloning a `Project` is cheap and shares everything unchanged, so undo history, autosave and background work each keep their own snapshot.

## The project file

RON text with the extension from `brand::PROJECT_FILE_EXTENSION`:

```ron
(
  format_version: 1,
  summary: Some((name: "...", settings: (...), length: (21, 1))),
  project: ( ... ),
)
```

- **Saving** writes `.name.saving` next to the file, flushes it to disk, then renames it over the old file. A crash mid-save leaves the old file intact.
- **The summary** lets the launcher show a project card without reading the whole project (`file::read_summary`).
- **A newer file** is refused with an explanation instead of being misread.

### Changing the format

1. Copy the current model types into `src/formats/v<N>.rs` (frozen, never edited again).
2. Change the model and raise `file::FORMAT_VERSION` to `N + 1`.
3. Add a step to `file::MIGRATIONS` that reads version `N` text into the frozen types, converts them, and writes version `N + 1` text.
4. Write the new sample with `WRITE_SAMPLES=1 cargo test -p project --test samples`. Keep every older sample: they must still open.
