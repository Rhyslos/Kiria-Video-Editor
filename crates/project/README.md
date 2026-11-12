# project

The project model, edit commands, undo/redo and autosave.

## Status

Done (Phases 0.3 and 0.4): model, project file, commands, undo/redo, autosave journal and crash recovery.

## Files

| File | What it holds |
|---|---|
| `src/time.rs` | `Time` (exact seconds as a reduced fraction), `Rate` (frames per second as a fraction, e.g. 30000/1001) and `Speed` (how fast a clip plays, e.g. 1/2). Frame conversion, snapping, timecode |
| `src/sound.rs` | `Sound`: how a clip sounds: volume in whole thousandths of a decibel (-60 dB, the bottom, is silent; up to +12 dB), pan in millionths (all left to all right), and a fade in and a fade out as exact `Time` lengths on the timeline. `within_limits`, `check` (refuses values out of range and fades that together are longer than the clip), `fitted` (cuts fades to fit a clip's length: the fade in stays, the fade out gives way) |
| `src/look.rs` | `Look`: where a clip's picture sits in the frame (position, size, turn), which part shows (crop), how strong it is (opacity) and its quick colour (exposure, contrast, saturation, temperature, tint), all in whole millionths. `within_limits` brings values into range, `check` refuses ones outside |
| `src/ids.rs` | One id type per kind of thing (`ClipId`, `TrackId`, ...). Ids come from one counter in the project and are never reused |
| `src/model.rs` | `Project`, `Sequence` (a timeline), `Track`, `Clip`, `MediaRef` + `Fingerprint` + `MediaInfo` (the file's video and sound streams, read on import), `TimelineLabel`, `Region`, the edit needle, and the `SequenceSettings` presets (Game clip, High frame rate, Vertical short). `Project::problems()` / `validate()` check the rules |
| `src/file.rs` | The project file: save (atomic), load, the summary block, format versions and migrations. `read_info` reads only the format version and summary (the text before the project), for project lists (from format 5 the summary also lists the media files, so lists can say which are missing; format 6 adds the timeline's master volume; format 7 each clip's speed; format 8 each clip's look; format 9 each clip's sound). `back_up_before_upgrade` keeps `<file>.backup-v<old>` before an older file is upgraded; `load_newer` reads a newer file for viewing |
| `src/versions.rs` | Saved versions: named copies in `<file>.versions/`, listed newest first; restoring one is the undoable `Restore` command. `is_folder` knows such a folder by its name, so project lists leave its files out |
| `src/templates.rs` | Templates: a stretch of a timeline (`Template`: its clips on every track, placed from zero, and the media they use) saved as `<name>.template` in the app's data folder, with its own format version; `capture` cuts one out of a timeline, `InsertTemplate` drops one in, and `list`, `save`, `rename` and `delete` look after the files. See Templates below |
| `src/edits.rs` | The trim tools beyond a plain trim: ripple trim, roll, slip and slide; `RemoveRanges`, which cuts stretches out of every track and closes the gaps; and the room-making behind `InsertMedia` (pushes everything later), `OverwriteMedia` (cuts away what it covers on every track) and `DropMedia` (cuts away what it covers on the tracks the file lands on); `cut_edges_on`, the cutting at both ends of a stretch that `LayMedia` shares with them. Each checks every clip it touches before changing any |
| `src/tracks.rs` | Tracks as a whole: adding one (after the others of its kind), removing an empty one, renaming one, hiding a video track; `Sequence::next_track_name` ("V2", "A3", never a name in use) |
| `src/layers.rs` | Working in layers: `LayMedia` (a file laid over a track) and `MoveBetweenTracks` (a clip moved to another track, or to a new one). In both, sound finds a free place by itself: see Sound finds a free place below |
| `src/command.rs` | `Command`: every change (import, place, drop on a track, lay over a track, move, move to another track, trim, remove or ripple-remove, split, labels, region, edit needle, tracks, media, relink, restoring a saved version, inserting a template). `apply` returns a new snapshot or an explained error and no change |
| `src/history.rs` | `History`: unlimited undo and redo as a stack of snapshots |
| `src/document.rs` | `Document`: an open project on disk. Records every change in the autosave journal, does full saves, and recovers after a crash. Opening an older format makes a backup copy first; a newer one opens read-only (`is_read_only`: changes and saves are refused with an explanation, and nothing is written) |
| `src/document.rs` (also) | `thumbnail_path`: where the launcher's picture of a project lives (`<file>.thumbnail.png`, beside the journal). `view_path`: where the editor keeps how the timeline was laid out (`<file>.view`: track heights and such; view state, never part of the project) |
| `src/side_files.rs` | `SideFile` and `existing`: the one list of the files that belong to a project besides the project file (see below). Moving, renaming and deleting a project all go by it |
| `tests/random_edits.rs` | Property test: random edit sessions with undo/redo stay valid, and journal replay rebuilds the exact project |
| `tests/samples.rs` + `tests/samples/` | One stored project per released file format, and one stored template per template format; all must keep opening |

## Rules the model keeps

- Time is never a float. Use `Time` and `Rate`; convert to `f64` only to draw or display.
- A clip's fades fit inside it: together no longer than the clip. Edits that change a clip's length leave its fades alone; `Project::fit_fades`, run after every command, then cuts them to fit (the fade in stays, the fade out gives way).
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
| 9 | Clips gain `sound` (volume, pan, fade in, fade out) | Version number only: defaults to the sound as it is |

### Changing the format

1. Copy the current model types into `src/formats/v<N>.rs` (frozen, never edited again).
2. Change the model and raise `file::FORMAT_VERSION` to `N + 1`.
3. Add a step to `file::MIGRATIONS` that reads version `N` text into the frozen types, converts them, and writes version `N + 1` text.
4. Write the new sample with `WRITE_SAMPLES=1 cargo test -p project --test samples`. Keep every older sample: they must still open.
5. Templates hold the same `Clip`, `Track` and `MediaRef`. A new field with a default needs nothing: the stored template samples are never rewritten, and they prove that a template saved before the field still reads (clips gained `look` that way; a template saved now keeps each clip's look). If old template text no longer reads into the types (a stored template sample stops opening), raise `templates::FORMAT_VERSION` too and add a step to `templates::MIGRATIONS`, the same way; the command in step 4 then writes the new format's sample, once.

## Templates

A template is a stretch of a timeline to drop into any project: an intro, an outro, a sting between parts. Nothing about it is stored in a project file.

```ron
(
  format_version: 1,
  template: (
    length: (7, 1),
    media: [ ... ],          // the files its clips use, as a project stores them
    video_tracks: [ ... ],   // tracks from the first up, clips placed from zero
    audio_tracks: [ ... ],
  ),
)
```

- **The file** is `<name>.template` in `templates/` in the app's data folder (`templates::folder`), written like a project (temporary file, then renamed). The name is the file's name, made safe for one; names differing only in capitals count as the same. A newer or damaged file is listed with why it can't be used, never skipped silently.
- **Saving** (`Template::capture`) takes the timeline's region, or the whole timeline without one. A clip crossing an edge of the stretch is kept cut there (a cut start also moves where in the file it begins, by the clip's speed). Tracks keep their place: empty tracks after the last used one are dropped. Ids are renumbered from 1, so the same stretch gives the same template whatever project it came from. A link left with one clip is dropped. Track switches (mute, solo, lock, hide) are not kept, and clips on a hidden or muted track are saved like any others; a track's name and volume are kept, for when the track has to be made. Timeline labels are not part of a template.
- **Format history:** version 1 is the first format.

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

## Sound finds a free place

Putting one picture over another must not cost the sound that is already there. So in `LayMedia` and `MoveBetweenTracks` a sound clip never cuts or pushes anything:

- **A track is free** for a clip when no other clip lies anywhere in the clip's stretch on it. For a move, the places the moved clips are leaving do not count: the whole group is taken off the timeline first.
- **A laid file's sound streams** each take the first free audio track, counting from the one an import would give that stream (the first stream from the first audio track, the second from the second). The streams keep the file's order, top to bottom. On an empty stretch the result is what `Import` gives, names of tracks included.
- **A moved clip's linked sound** stays on its own track when that is free at the new place. Sound that can't stay looks at the tracks under its own first, then at the ones above from the top, so a track is only made when no track at all has room.
- **A track is made** when no track is free: after the other audio tracks, named by `next_track_name` ("A3").
- **What a laid picture covers** on its own video track is replaced: the clips there are cut at its edges (together with their linked clips, so linked clips stay as long as each other) and the pieces under it go. When the file has sound of its own, the sound linked to those pieces goes too, so one recording laid over another replaces it, picture and sound, and its sound usually lands where the old sound was. A file without sound (a still, a silent clip) replaces the picture only: the sound under it keeps playing. Nothing else is touched: not the other video tracks, not sound that belongs to none of the covered clips (music, a commentary), whatever track it is on.
- **The clip that is dragged** is never moved out of the way and never cuts: where it would overlap a clip on the track it was dragged to, the move is refused. The same holds for linked pictures, which stay on their tracks: which picture is on which layer decides what is seen, so that is never changed by itself.

## Autosave and recovery

- A new command is a new variant of `Command`: older journals never hold it, and the commands they do hold keep their meaning. That is why moving to a track and dropping on a track are commands of their own instead of new fields on `MoveLinked` and `OverwriteMedia`, and why `LayMedia` and `MoveBetweenTracks`, which let sound find a free place, came beside `DropMedia`, `OverwriteMedia` and `MoveToTrack` instead of changing them. The app no longer makes `OverwriteMedia`, `MoveToTrack` or a `DropMedia` that does not insert; they stay, with their tests, so that a journal written by an earlier version replays to exactly the project it described. (A field with a default would have kept old journal lines readable too, but an earlier version reading a newer journal would have ignored the field and quietly done something else; a variant it does not know is a line it skips and logs.)
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
| Move to a track (`MoveToTrack`: older journals) | The clip lands on another track of its kind, and it and its linked clips move sideways by the same amount: the linked clips stay on their own tracks. Refused, with nothing changed, if any of them would overlap a clip or start before zero, or for a track of the other kind |
| Lay over a track (`LayMedia`: a file dropped on a track, or "Overwrite at the edit point") | The file's picture goes on the chosen video track (the first when none is chosen) and replaces what it covers there; its sound finds a free place. See below |
| Move between tracks (`MoveBetweenTracks`: a clip dragged to another track, or past the last one) | The clip lands on the chosen track of its kind, or on a new track made after the others of its kind (a video track on top, an audio track at the bottom, named by `next_track_name`) when none is chosen; it and its linked clips move sideways by the same amount. Linked sound finds a free place (below); linked pictures stay on their tracks. Refused, with nothing changed and no track made, if the clip itself or a linked picture would overlap a clip, if any of them would start before zero, or for a track of the other kind |
| Drop on a track (`DropMedia`: a file dropped with Ctrl held, and older journals) | A file's picture goes on the chosen video track (the first when none is chosen) and each sound stream on its own audio track, as for Import. Inserted, everything from there on moves later on every track. Otherwise it lies over what is there: what it covers is cut away on the tracks it lands on only, so a clip dropped on a higher track leaves the picture under it. A clip that is cut is cut together with its linked clips, so linked clips stay as long as each other |
| Add track | An empty track after the others of its kind: a video track on top of the picture (`video_tracks[0]` is the lowest layer), an audio track at the bottom. A name is needed |
| Remove track | Only an empty track, and never the last of its kind: a track with clips is refused with an explanation, so removing a track never removes clips or leaves linked clips without their partners |
| Rename track | Any name of 1 to 40 characters, without spaces around it |
| Hide track | A video track's clips are not seen in the picture (`Track::hidden`); refused for audio tracks, which have mute |
| Trim (drag an edge) | Moves one edge of the clip and its linked clips; the start edge also moves where in the file it begins, so the picture stays put. Refused past the file's start or end, or to no length |
| Track mix | A track's mute, solo and volume (-60 to +12 dB) |
| Set look | Sets where one clip's picture sits, which part shows, its opacity and quick colour. Only clips on video tracks; values outside their range are refused. Both halves of a split keep it |
| Set sound | Sets the volume, pan and fades of one or more sound clips, each its own, as one step (the inspector sets a picture's linked sound streams together this way). Only clips on audio tracks; volume or pan out of range, and fades longer than the clip, are refused, and one refused clip refuses the step. A split keeps volume and pan on both halves, the fade in on the left half and the fade out on the right; a template cut out of a timeline does the same where it cuts a clip |
| Set settings | Fixes a timeline's size, frame rate and sound rate; it stops being automatic |
| Insert template | One step: everything from the chosen time on, on every track, moves later by the template's length (clips under it are split, labels move too) and the template's clips fill the room, each on the track of the same kind and number. Tracks that are missing are made after the others of their kind, as `AddTrack` does, each as loud as the template's track in that place and with its name when someone chose it ("Music") and no track of that kind has it yet; otherwise it gets the next usual name ("V2", "A3"). A template with fewer tracks than the timeline leaves the other tracks empty for its length: their clips move along like everything else. A file the project already has (same contents: size and sample hash, wherever it is now) is reused; other files are added. Every clip and link gets a new id; clips linked in the template stay linked. A timeline still on Automatic takes its settings from the template's first video. The command carries the whole template, so the journal replays it without the file |
| Import | One step: adds the file and places it at the end of the timeline, the picture on the first video track and each sound stream on its own audio track (made when missing; an unused default track takes the stream's name), all linked. Pictures last 5 seconds. On an automatic timeline the first video also sets the settings (upright size, frame rate, first sound stream's rate) |
