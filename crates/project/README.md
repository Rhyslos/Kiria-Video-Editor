# project

The project model, edit commands, undo/redo and autosave.

## Status

Done (Phases 0.3 and 0.4): model, project file, commands, undo/redo, autosave journal and crash recovery.

## Files

| File | What it holds |
|---|---|
| `src/time.rs` | `Time` (exact seconds as a reduced fraction) and `Rate` (frames per second as a fraction, e.g. 30000/1001). Frame conversion, snapping, timecode |
| `src/ids.rs` | One id type per kind of thing (`ClipId`, `TrackId`, ...). Ids come from one counter in the project and are never reused |
| `src/model.rs` | `Project`, `Sequence` (a timeline), `Track`, `Clip`, `MediaRef` + `Fingerprint` + `MediaInfo` (the file's video and sound streams, read on import), `TimelineLabel`, `Region`, the edit needle, and the `SequenceSettings` presets (Game clip, High frame rate, Vertical short). `Project::problems()` / `validate()` check the rules |
| `src/file.rs` | The project file: save (atomic), load, the summary block, format versions and migrations. `read_info` reads only the format version and summary (the text before the project), for project lists (from format 5 the summary also lists the media files, so lists can say which are missing). `back_up_before_upgrade` keeps `<file>.backup-v<old>` before an older file is upgraded; `load_newer` reads a newer file for viewing |
| `src/edits.rs` | The trim tools beyond a plain trim: ripple trim, roll, slip and slide; and `RemoveRanges`, which cuts stretches out of every track and closes the gaps. Each checks every clip it touches before changing any |
| `src/command.rs` | `Command`: every change (import, place, move, trim, remove or ripple-remove, split, labels, region, edit needle, tracks, media, relink). `apply` returns a new snapshot or an explained error and no change |
| `src/history.rs` | `History`: unlimited undo and redo as a stack of snapshots |
| `src/document.rs` | `Document`: an open project on disk. Records every change in the autosave journal, does full saves, and recovers after a crash. Opening an older format makes a backup copy first; a newer one opens read-only (`is_read_only`: changes and saves are refused with an explanation, and nothing is written) |
| `src/document.rs` (also) | `thumbnail_path`: where the launcher's picture of a project lives (`<file>.thumbnail.png`, beside the journal) |
| `tests/random_edits.rs` | Property test: random edit sessions with undo/redo stay valid, and journal replay rebuilds the exact project |
| `tests/samples.rs` + `tests/samples/` | One stored project per released file format; all must keep opening |

## Rules the model keeps

- Time is never a float. Use `Time` and `Rate`; convert to `f64` only to draw or display.
- Clips on a track are in start order and never overlap; every clip's media exists; lengths are positive; a region ends after it starts. `Project::validate()` checks this on every save and load.
- Collections are `imbl` persistent structures: cloning a `Project` is cheap and shares everything unchanged, so undo history, autosave and background work each keep their own snapshot.

## The project file

RON text with the extension from `brand::PROJECT_FILE_EXTENSION`:

```ron
(
  format_version: 4,
  summary: Some((name: "...", settings: (...), length: (21, 1))),
  project: ( ... ),
)
```

- **Saving** writes `.name.saving` next to the file, flushes it to disk, then renames it over the old file. A crash mid-save leaves the old file intact.
- **The summary** lets the launcher show a project card without reading the whole project (`file::read_summary`).
- **A newer file** is refused with an explanation instead of being misread.

### Format history

| Version | Change | Migration |
|---|---|---|
| 1 | First format | – |
| 2 | Media entries gain `info`: the file's video and sound streams | Version number only: `info` defaults to empty |
| 3 | Timelines gain `automatic` (settings wait for the first video); the summary says so too | Version number only: `automatic` defaults to off, as older timelines had fixed settings |
| 4 | Video streams gain `variable_frame_rate` | Version number only: defaults to off |

### Changing the format

1. Copy the current model types into `src/formats/v<N>.rs` (frozen, never edited again).
2. Change the model and raise `file::FORMAT_VERSION` to `N + 1`.
3. Add a step to `file::MIGRATIONS` that reads version `N` text into the frozen types, converts them, and writes version `N + 1` text.
4. Write the new sample with `WRITE_SAMPLES=1 cargo test -p project --test samples`. Keep every older sample: they must still open.

## Autosave and recovery

- Every change, undo and redo is appended to `<project file>.journal` (one line each, flushed to disk) the moment it happens.
- Journal entries are numbered. A full save writes the project file plus the number of the last entry it includes, so recovery replays only newer entries: nothing is lost and nothing is applied twice.
- `Document::tick(now, idle)` returns a `SaveJob` when changes are 30 seconds old, or straight away when the app is idle. The job holds its own snapshot and runs on a worker (`jobs` crate); `Document::finish_save` then empties the journal if nothing changed meanwhile. `Document::save()` does the same on the calling thread.
- On open, `Document::check` reports how many unsaved changes a crash left. `Document::open` then either replays them (`Recovery::Recover`, which also saves them into the project file) or sets the journal aside as `<file>.journal.discarded` (`Recovery::OpenLastSave`). Nothing is ever deleted.
- A half-written last journal line (power cut mid-write) is ignored; entries that no longer apply are skipped and logged.

## Edit behaviour

| Command | Behaviour |
|---|---|
| Split (`S`) with a clip selected | Cuts that clip and its linked clips (its own audio); the right halves stay linked to each other |
| Split with nothing selected | Cuts every clip under the edit needle on every track |
| Remove (`Delete`) | Removes the clip and its linked clips, leaving a gap |
| Ripple remove (`Backspace`) | Same, and later clips on those tracks move back to close the gap |
| Place, move | Refused with an explanation if the clip would overlap another or start before zero |
| Move (drag) | Moves the clip and its linked clips by the same amount; refused if any would overlap or start before zero |
| Trim (drag an edge) | Moves one edge of the clip and its linked clips; the start edge also moves where in the file it begins, so the picture stays put. Refused past the file's start or end, or to no length |
| Track mix | A track's mute, solo and volume (-60 to +12 dB) |
| Set settings | Fixes a timeline's size, frame rate and sound rate; it stops being automatic |
| Import | One step: adds the file and places it at the end of the timeline, the picture on the first video track and each sound stream on its own audio track (made when missing; an unused default track takes the stream's name), all linked. Pictures last 5 seconds. On an automatic timeline the first video also sets the settings (upright size, frame rate, first sound stream's rate) |
