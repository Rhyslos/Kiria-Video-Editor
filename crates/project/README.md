# project

The project model, edit commands, undo/redo and autosave.

## Status

Done (Phases 0.3 and 0.4): model, project file, commands, undo/redo, autosave journal and crash recovery.

## Files

| File | What it holds |
|---|---|
| `src/time.rs` | `Time` (exact seconds as a reduced fraction), `Rate` (frames per second as a fraction, e.g. 30000/1001) and `Speed` (how fast a clip plays, e.g. 1/2). Frame conversion, snapping, timecode |
| `src/look.rs` | `Look`: where a clip's picture sits in the frame (position, size, turn), which part shows (crop), how strong it is (opacity) and its quick colour (exposure, contrast, saturation, temperature, tint), all in whole millionths. `within_limits` brings values into range, `check` refuses ones outside |
| `src/ids.rs` | One id type per kind of thing (`ClipId`, `TrackId`, ...). Ids come from one counter in the project and are never reused |
| `src/model.rs` | `Project`, `Sequence` (a timeline), `Track`, `Clip`, `MediaRef` + `Fingerprint` + `MediaInfo` (the file's video and sound streams, read on import), `TimelineLabel`, `Region`, the edit needle, and the `SequenceSettings` presets (Game clip, High frame rate, Vertical short). `Project::problems()` / `validate()` check the rules |
| `src/file.rs` | The project file: save (atomic), load, the summary block, format versions and migrations. `read_info` reads only the format version and summary (the text before the project), for project lists (from format 5 the summary also lists the media files, so lists can say which are missing; format 6 adds the timeline's master volume; format 7 each clip's speed; format 8 each clip's look). `back_up_before_upgrade` keeps `<file>.backup-v<old>` before an older file is upgraded; `load_newer` reads a newer file for viewing |
| `src/versions.rs` | Saved versions: named copies in `<file>.versions/`, listed newest first; restoring one is the undoable `Restore` command. `is_folder` knows such a folder by its name, so project lists leave its files out |
| `src/edits.rs` | The trim tools beyond a plain trim: ripple trim, roll, slip and slide; `RemoveRanges`, which cuts stretches out of every track and closes the gaps; and the room-making behind `InsertMedia` (pushes everything later), `OverwriteMedia` (cuts away what it covers on every track) and `DropMedia` (cuts away what it covers on the tracks the file lands on). Each checks every clip it touches before changing any |
| `src/tracks.rs` | Tracks as a whole: adding one (after the others of its kind), removing an empty one, renaming one, hiding a video track; `Sequence::next_track_name` ("V2", "A3", never a name in use) |
| `src/command.rs` | `Command`: every change (import, place, drop on a track, move, move to another track, trim, remove or ripple-remove, split, labels, region, edit needle, tracks, media, relink). `apply` returns a new snapshot or an explained error and no change |
| `src/history.rs` | `History`: unlimited undo and redo as a stack of snapshots |
| `src/document.rs` | `Document`: an open project on disk. Records every change in the autosave journal, does full saves, and recovers after a crash. Opening an older format makes a backup copy first; a newer one opens read-only (`is_read_only`: changes and saves are refused with an explanation, and nothing is written) |
| `src/document.rs` (also) | `thumbnail_path`: where the launcher's picture of a project lives (`<file>.thumbnail.png`, beside the journal). `view_path`: where the editor keeps how the timeline was laid out (`<file>.view`: track heights and such; view state, never part of the project) |
| `src/side_files.rs` | `SideFile` and `existing`: the one list of the files that belong to a project besides the project file (see below). Moving, renaming and deleting a project all go by it |
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
| 5 | The summary lists the media files | Version number only: empty until the project is saved again |
| 6 | Timelines gain `master_gain_db` | Version number only: defaults to 0 dB |
| 7 | Clips gain `speed` | Version number only: defaults to normal speed |
| 8 | Clips gain `look` (place, crop, opacity, quick colour) | Version number only: defaults to the picture as it is |

### Changing the format

1. Copy the current model types into `src/formats/v<N>.rs` (frozen, never edited again).
2. Change the model and raise `file::FORMAT_VERSION` to `N + 1`.
3. Add a step to `file::MIGRATIONS` that reads version `N` text into the frozen types, converts them, and writes version `N + 1` text.
4. Write the new sample with `WRITE_SAMPLES=1 cargo test -p project --test samples`. Keep every older sample: they must still open.

## Files beside the project file

A project is more than one file on disk. Everything else sits beside the project file, under a name made from its name:

| Name | What it is | Written by |
|---|---|---|
| `<file>.journal` | The autosave journal | `document.rs` |
| `<file>.journal.discarded` | Unsaved work set aside by "Open last save" | `document.rs` |
| `.<file>.saving` | A save in progress; only stays after a crash or power cut | `file.rs` |
| `<file>.backup-v<N>` | The file as it was before an upgrade from format `N` | `file.rs` |
| `<file>.thumbnail.png` | The picture on the launcher's card | the editor, on every save |
| `<file>.view` | The timeline layout | the editor |
| `<file>.versions/` | The folder of saved versions | `versions.rs` |

`side_files::existing(project_file)` returns those that are on disk, and `SideFile::path` says where each one is for a given project file. Whatever moves, renames or deletes a project uses these two and nothing else, so no file is forgotten. **A new kind of file kept beside projects must be added to `SideFile`** (the tests in `side_files.rs` compare the list with what this crate really writes beside a project).

Imported recordings are not side files: a project only points at them, and nothing that moves or deletes a project touches them. Caches (waveforms, clip thumbnails) are kept in memory per media file, so there is nothing of them on disk to list yet.

## Autosave and recovery

- A new command is a new variant of `Command`: older journals never hold it, and the commands they do hold keep their meaning. That is why moving to a track and dropping on a track are commands of their own instead of new fields on `MoveLinked` and `OverwriteMedia`.
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
| Move to a track (drag up or down) | The clip lands on another track of its kind, and it and its linked clips move sideways by the same amount: the linked clips stay on their own tracks. Refused, with nothing changed, if any of them would overlap a clip or start before zero, or for a track of the other kind |
| Drop on a track | A file's picture goes on the chosen video track (the first when none is chosen) and each sound stream on its own audio track, as for Import. Inserted, everything from there on moves later on every track. Otherwise it lies over what is there: what it covers is cut away on the tracks it lands on only, so a clip dropped on a higher track leaves the picture under it. A clip that is cut is cut together with its linked clips, so linked clips stay as long as each other |
| Add track | An empty track after the others of its kind: a video track on top of the picture (`video_tracks[0]` is the lowest layer), an audio track at the bottom. A name is needed |
| Remove track | Only an empty track, and never the last of its kind: a track with clips is refused with an explanation, so removing a track never removes clips or leaves linked clips without their partners |
| Rename track | Any name of 1 to 40 characters, without spaces around it |
| Hide track | A video track's clips are not seen in the picture (`Track::hidden`); refused for audio tracks, which have mute |
| Trim (drag an edge) | Moves one edge of the clip and its linked clips; the start edge also moves where in the file it begins, so the picture stays put. Refused past the file's start or end, or to no length |
| Track mix | A track's mute, solo and volume (-60 to +12 dB) |
| Set look | Sets where one clip's picture sits, which part shows, its opacity and quick colour. Only clips on video tracks; values outside their range are refused. Both halves of a split keep it |
| Set settings | Fixes a timeline's size, frame rate and sound rate; it stops being automatic |
| Import | One step: adds the file and places it at the end of the timeline, the picture on the first video track and each sound stream on its own audio track (made when missing; an unused default track takes the stream's name), all linked. Pictures last 5 seconds. On an automatic timeline the first video also sets the settings (upright size, frame rate, first sound stream's rate) |
