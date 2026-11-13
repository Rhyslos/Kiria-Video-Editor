# project

The project model, edit commands, undo/redo and autosave.

## Status

Done (Phases 0.3 and 0.4): model, project file, commands, undo/redo, autosave journal and crash recovery.

## Files

| File | What it holds |
|---|---|
| `src/time.rs` | `Time` (exact seconds as a reduced fraction), `Rate` (frames per second as a fraction, e.g. 30000/1001) and `Speed` (how fast a clip plays, e.g. 1/2). Frame conversion, snapping, timecode |
| `src/ramp.rs` | Speed ramps: `SpeedKey` (a speed at a moment of the clip on the timeline, from its start, with how it goes on: linear, hold, smooth) and `Ramp`, a clip's keys in order. `source_offset` adds the speed up from the clip's start to a moment, exactly to the flick (1/705 600 000 s, a whole number of which every common frame and sound rate has), with no floats; `speed_at`, `shifted` (trims, splits, templates), `set` / `remove` / `set_ease` for the diamond, `summary` ("Ramp 100→25→100%"), `slow_motion` (the inspector's ramp buttons) and `clean_slow` (the speed that shows every recorded frame of a high-frame-rate file once: 50% for 120 fps on 60, 5/12 for 144). See Speed ramps below |
| `src/sound.rs` | `Sound`: how a clip sounds: volume in whole thousandths of a decibel (-60 dB, the bottom, is silent; up to +12 dB), pan in millionths (all left to all right), and a fade in and a fade out as exact `Time` lengths on the timeline. `within_limits`, `check` (refuses values out of range and fades that together are longer than the clip), `fitted` (cuts fades to fit a clip's length: the fade in stays, the fade out gives way) |
| `src/track_sound.rs` | Format 16: `TrackSound`, how an audio track sounds besides its volume, mute and solo: its `TrackRole` (Other, Dialogue, Music, Effects, Ambience; ducking listens to dialogue), pan, `HighPass`, `Eq` (six bands), `Compressor`, `Cleanup` (with its learned `NoiseProfile`, 64 levels), `Ducking`, `Loudness` (the Loudness tool), and volume and pan lines (`LinePoint`s in timeline time; the volume line is added to the track's volume, the pan line is the pan while it has points). `MasterSound`: the mix's compressor and true-peak `Limiter`. Whole numbers throughout; every default is off, so older projects sound as they did. `check` refuses values out of range, explained. See Track sound below |
| `src/look.rs` | `Look`: where a clip's picture sits in the frame (position, size, turn), which part shows (crop), how strong it is (opacity) and its quick colour (exposure, contrast, saturation, temperature, tint), all in whole millionths. `within_limits` brings values into range, `check` refuses ones outside |
| `src/keyframes.rs` | Keyframes: how numbers of a clip's look change over time. `LookField` names each number (and `GROUPS` keeps a place's X and Y together), `Ease` (Linear, Hold, Smooth) says how a value goes from one keyframe to the next, `Keyframe` is a value at a moment of the clip's file, `Keyframes` a clip's lists by number. `value_at` and `Keyframes::apply` work out the values at a moment in whole numbers (no floats, no allocation), `put` sets a pasted keyframe in place of any in its frame, `cut_to` keeps what matters to a stretch with its ends held (splits and templates), `problems` checks them. On `Clip`: `look_at` (the look at a moment of the timeline), `timeline_at`, `moment` (the playhead brought onto the clip), `frame_in_file` (what "a keyframe here" means) and `keyframe_frames`. See Keyframes below |
| `src/animate.rs` | The commands that work on a look at a moment: `SetLookAt` (what the inspector and the handles on the picture send: a number with keyframes gets one at the playhead, one without is set for the whole clip), `Keyframe` (`KeyframeChange`: add, remove, or set the ease of the keyframe at the playhead) `ResetLook` (a section's Reset: numbers back as they come, their keyframes gone) and `ChangeKeyframes` (`KeyframesChange`: what the timeline's keyframe marks do to every keyframe of their frames, several at once in one step: move them by whole frames, remove them, or set their ease) |
| `src/clipboard.rs` | Copying and pasting keyframes: `CopiedKeyframes` (`copy`: every keyframe of the chosen marks' frames, each mark's place on the timeline after the first; `says` names its numbers, `landing` where each mark lands on a clip) and `PasteKeyframes`, which puts them at the playhead on a picture clip. See Keyframes below |
| `src/ids.rs` | One id type per kind of thing (`ClipId`, `TrackId`, `MediaLabelId`, `MediaListId`, ...). Ids come from one counter in the project and are never reused |
| `src/model.rs` | `Project`, `Sequence` (a timeline), `Track`, `Clip` (with `ClipKind`: a part of a file, another timeline, or an adjustment layer; `Project::source_length` is how long what it plays is), `MediaRef` + `Fingerprint` + `MediaInfo` (the file's video and sound streams, read on import), `TimelineLabel`, `Region`, the edit needle, and the `SequenceSettings` presets (Game clip, High frame rate, Vertical short). `Project::problems()` / `validate()` check the rules |
| `src/library.rs` | `Library` (format 13): how the project's media is organised. `MediaLabel` (name, colour) and which files have each (`tagged`), `MediaList` (a custom list of files, in order) and a note per file. The commands `AddMediaLabel`, `EditMediaLabel`, `RemoveMediaLabel`, `SetMediaLabel`, `SetMediaNote`, `AddMediaList`, `RenameMediaList`, `RemoveMediaList` and `SetListMember` work here: names are trimmed, 1 to 40 characters and unique among labels (or lists) whatever their capitals; notes are trimmed and up to 4 000 characters, an empty one removed; removing a label takes it off every file, removing a list leaves its files in the project. `MediaUse` (format 15): how a file is to be added from the Library page (in and out marks, a level per sound stream), set with `SetMediaUses` (several files in one step; an empty use is removed; marks inside the file, in before out, levels within the volume's range); `forget` drops everything kept about a file leaving the project. The `library` crate's search index is built from this and never written back |
| `src/global_labels.rs` | `GlobalLabels`: label names and colours offered in every project ("everywhere"), kept in `labels.ron` in the app's data folder (format 1: `format_version` and `labels`, each a name and a colour; a sample in `tests/samples/labels-v1.ron`). `load` and `parse` (no file: none; a newer or damaged file is refused and left alone), `save` (atomic, on a worker), `add` (a name once, whatever its capitals), `remove`, `find`. Nothing here is in a project file: putting such a label on a file copies it into the project's labels with `AddMediaLabel` |
| `src/file.rs` | The project file: save (atomic), load, the summary block, format versions and migrations. `read_info` reads only the format version and summary (the text before the project), for project lists (from format 5 the summary also lists the media files, so lists can say which are missing; format 6 adds the timeline's master volume; format 7 each clip's speed; format 8 each clip's look; format 9 each clip's sound; format 10 each clip's keyframes; format 11 each clip's speed ramp and frame blending; format 12 each clip's kind: file, nested timeline or adjustment layer; format 13 the media's labels, lists and notes; format 14 the length a file imported with its end missing says it has; format 15 the Library's marks and levels). `back_up_before_upgrade` keeps `<file>.backup-v<old>` before an older file is upgraded; `load_newer` reads a newer file for viewing |
| `src/versions.rs` | Saved versions: named copies in `<file>.versions/`, listed newest first; restoring one is the undoable `Restore` command. `load` makes a version's relative media paths (a package's) full from the project's folder. `is_folder` knows such a folder by its name, so project lists leave its files out |
| `src/templates.rs` | Templates: a stretch of a timeline (`Template`: its clips on every track, placed from zero, and the media they use) saved as `<name>.template` in the app's data folder, with its own format version; `capture` cuts one out of a timeline, `InsertTemplate` drops one in, and `list`, `save`, `rename` and `delete` look after the files. See Templates below |
| `src/edits.rs` | The trim tools beyond a plain trim: ripple trim, roll, slip and slide; `RemoveRanges`, which cuts stretches out of every track and closes the gaps; and the room-making behind `InsertMedia` (pushes everything later), `OverwriteMedia` (cuts away what it covers on every track) and `DropMedia` (cuts away what it covers on the tracks the file lands on); `cut_edges_on`, the cutting at both ends of a stretch that `LayMedia` shares with them. Each checks every clip it touches before changing any |
| `src/timelines.rs` | Several timelines, nested timelines and adjustment layers: `AddSequence`, `DuplicateSequence`, `RenameSequence`, `RemoveSequence`, `NestClips`, `PlaceSequence` and `AddAdjustment` (see Timelines below); `Project::plays` (whether one timeline plays another, directly or through others), `sequences_playing`, `free_sequence_name` ("Timeline 2"), and the loop check behind `problems` |
| `src/tracks.rs` | Tracks as a whole: adding one (after the others of its kind), removing an empty one, renaming one, hiding a video track; `Sequence::next_track_name` ("V2", "A3", never a name in use) |
| `src/layers.rs` | Working in layers: `LayMedia` (a file laid over a track) and `MoveBetweenTracks` (a clip moved to another track, or to a new one). In both, sound finds a free place by itself: see Sound finds a free place below |
| `src/command.rs` | `Command`: every change (import, track and master sound (format 16), place, prune, drop on a track, lay over a track, move, move to another track, trim, remove or ripple-remove, split, labels, region, edit needle, tracks, media, relink, restoring a saved version, inserting a template, media labels, lists and notes). `apply` returns a new snapshot or an explained error and no change |
| `src/history.rs` | `History`: unlimited undo and redo as a stack of snapshots. `open` opens another timeline without an undo step; undo and redo keep it open while it exists in the state they reach |
| `src/journal_sync.rs` | `JournalSync`: puts the journal on the disk itself (`sync_data`) on a thread of its own. `request` hands it the journal and returns at once; requests that come while the disk is busy go together; a failure is returned by the next request |
| `src/document.rs` | `Document`: an open project on disk. Records every change in the autosave journal, does full saves, and recovers after a crash. Opening an older format makes a backup copy first; a newer one opens read-only (`is_read_only`: changes and saves are refused with an explanation, and nothing is written) |
| `src/document.rs` (also) | `thumbnail_path`: where the launcher's picture of a project lives (`<file>.thumbnail.png`, beside the journal). `view_path`: where the editor keeps how the timeline was laid out (`<file>.view`: track heights and such; view state, never part of the project) |
| `src/prune.rs` | Prune: `used_media` (every file a clip on some timeline plays, nested timelines and timelines placed nowhere included), `unused_media` (the rest, in the order they were added) and `RemoveMedia`, which takes chosen unused files out of the project with their labels, places in lists and notes as one step. Refused for a file a timeline uses. Files on disk are never touched |
| `src/package.rs` | Package project: `plan` (the new folder, `<chosen>/<project name>` or "name 2", the files to copy with their sizes, the packaged project with only used media and paths `Media/<file name>`, files that can't be found, files left out as unused) and `run` (copies a piece at a time, says how many bytes are done, stops when cancelled; what it made is taken away again on a cancel or a failure; the originals are only read). See The package below |
| `src/side_files.rs` | `SideFile` and `existing`: the one list of the files that belong to a project besides the project file (see below). Moving, renaming and deleting a project all go by it |
| `tests/random_edits.rs` | Property test: random edit sessions with undo/redo stay valid, and journal replay rebuilds the exact project |
| `tests/samples.rs` + `tests/samples/` | One stored project per released file format, one stored template per template format, and one labels file (`labels-v<N>.ron`) per format of the labels for every project; all must keep opening |

## Rules the model keeps

- Time is never a float. Use `Time` and `Rate`; convert to `f64` only to draw or display.
- A clip's fades fit inside it: together no longer than the clip. Edits that change a clip's length leave its fades alone; `Project::fit_fades`, run after every command, then cuts them to fit (the fade in stays, the fade out gives way).
- Clips on a track are in start order and never overlap; every clip's media exists; lengths are positive; a region ends after it starts. `Project::validate()` checks this on every save and load.
- A clip's keyframes are in time order, never two at one moment, with values inside their number's range; only clips on video tracks have them.
- A clip plays a part of a file, another timeline of the project, or nothing (an adjustment layer, only on video tracks): `Clip::kind`. A clip that plays no file has `media` set to `MediaId::NONE`. A timeline never plays itself, directly or through the timelines nested in it, and a nested clip's timeline exists.
- A clip's speed keyframes (`ramp`) are in time order, never two at one moment, each speed a fraction of at most 10 000 parts. With a ramp, the clip's `speed` means nothing; `Clip::source_at`, `source_len` and `source_offset` follow whichever applies, so nothing else needs to know.
- Collections are `imbl` persistent structures: cloning a `Project` is cheap and shares everything unchanged, so undo history, autosave and background work each keep their own snapshot.

## Keyframes

A keyframe's moment is a time in the clip's file (source time), not on the timeline, so keyframes stay on the picture they were set on: moving, trimming, rolling, sliding or changing the speed of a clip leaves each one on the same frame of its file (at half speed a fade takes twice as long on the timeline), and slipping a clip moves them with what it shows. Keyframes outside what a clip shows are kept, not shown, and still shape the values inside it; trimming back out brings them back, as it brings back the picture. A split gives each half only the keyframes of its own part, plus one at the cut holding the value there, so neither half looks any different (a smooth stretch cut in two eases on each side of the cut); a template keeps the keyframes of the stretch it holds the same way.

On the timeline a keyframe mark is a frame of the clip with keyframes, whatever numbers they are for, and `ChangeKeyframes` works on marks: moving one moves all its keyframes the same distance in the file (whole timeline frames, so at half speed two frames of the timeline are one of the file), keeps their values and eases, and never takes them off what the clip shows. A move that would give a number two keyframes in one frame is refused with the numbers named; keyframes of other numbers already there are joined, so two marks become one. Removing a mark's keyframes leaves a number without any at the value the last one held, as removing one at the playhead does.

Copied marks (`clipboard::CopiedKeyframes`) hold every keyframe of their frames (number, value, ease) and how far after the first each mark is **on the timeline**, so what was seen is what is pasted, whatever speed either clip plays at. `PasteKeyframes` puts the first on the frame at the playhead (brought onto the clip) and the others as far after it, in whole frames of the timeline pasted into, on the same clip or another picture clip. A pasted number's keyframe replaces one already in that frame (pasting replaces what it lands on, as pasting over a selection does; moving a mark refuses instead, since a drag that lands on a keyframe is more often a slip); other numbers' keyframes there stay, so marks join. Marks that would land past either end of the clip are left out and the rest pasted (`CopiedKeyframes::landing` says where each lands, `None` for one left out, so the window can say how many); a paste where none fits is refused with the reason. One undo step; the command carries the keyframes, so the journal replays it without the clipboard.

"At the playhead" means the timeline frame the playhead is in: a keyframe anywhere in that frame's stretch of the file counts as there (a clip at another speed may have keyframes between frame starts), and with the playhead off the clip its first or last frame stands in. Values in between are worked out in whole numbers with a fixed-point share, so the viewer and an export give exactly the same picture.

## Speed ramps

A clip plays at one speed (`speed`) or at a speed that changes over time (`ramp`, speed keyframes). A ramp's keyframes are moments of the clip **on the timeline**, counted from its start, because a ramp is about the timeline ("slow down here, for a second"); look keyframes stay in file time.

- **Where in its file a clip is** at a moment is the ramp added up from the clip's start (`Ramp::source_offset`): whole stretches between keyframes exactly ((v0 + v1) / 2 × length for linear and smooth), the stretch the moment is in with a fixed-point share, each brought to the nearest flick. Every common frame rate has a whole number of flicks a frame, so at a steady speed that suits the recording (half speed for 120 fps on a 60 fps timeline, 5/12 for 144 fps) every timeline frame lands exactly on a recorded frame, ramp or not; the viewer, playback and export work it out the same way.
- **Setting a ramp** (`SetRamp`) keeps the clip's start and the part of its file it plays, so its length follows: the frame boundary (at the timeline's rate) nearest to where the ramp has played that part, never past the end of the file. Linked clips get the same ramp at the same place on the timeline, and what comes after the clip moves along, as with `SetSpeed`. `SetSpeed` takes a ramp's place; an empty ramp puts the clip back at its `speed`.
- **Trims, rolls, slides and splits** keep the picture where it was: the clip starts further into its file by what the ramp plays over the trimmed stretch, and the keyframes move with the picture (`Clip::start_cut_by`), so trimming back out brings the same ramp back. Both halves of a split keep the whole ramp, in its place; a template cut out of a clip does the same. A **slip** keeps the ramp where it is in the clip and moves the picture under it. Keyframes outside what the clip shows are kept and still shape it.
- **Look keyframes** stay on their frame of the file through all of this; with a ramp, `Clip::frame_showing` finds the timeline frame that shows a moment of the file by halving, and moving keyframe marks moves each frame's keyframes as far in the file as the ramp plays between the two frames.
- **Frame blending** (`blend`, `SetFrameBlend`, pictures only): off, or the two recorded frames around each moment mixed by how near each is. Exactly on a recorded frame nothing is mixed, so a high-frame-rate recording slowed to suit it is never blended.

## Timelines

A project has one or more timelines (`sequences`), each with its own settings, tracks, labels, region and edit needle.

- **Which one is open** (`active_sequence`) is not an edit: `History::open` (and `Document::open_sequence`) changes it without an undo step or a journal line, and undo and redo leave it open while it exists in the state they reach (undoing the timeline that was just made opens the one that was open before). The editor keeps it with the timeline's layout (`<file>.view`); the project file holds the one open at the last save, which the launcher's card describes.
- **Making, copying, renaming, removing**: names are 1 to 40 characters and no two timelines share one, whatever their capitals. A new or copied timeline is opened by its command (so redo opens it again). A copy gives every track, clip, link and label a new id; clips nested in it play the same timelines. The last timeline, and one that clips elsewhere play, can't be removed (the error names the timelines that play it).
- **A nested timeline** is a clip whose `kind` is `Timeline(id)`. On a video track it shows that timeline's picture at its moment, placed, cut, turned, coloured and faded by its own look and keyframes like any picture; on an audio track it plays that timeline's sound (its own volume, pan and fades apply). Times "in its file" are times in that timeline, so trims, slips, speed, ramps and splits work as for a file, and trimming past the timeline's end is refused as past a file's end. `PlaceSequence` puts one in another at a time: its picture on the chosen video track (refused where that stretch isn't free), its sound on the first audio track free for its length (a track is made when none is), linked; refused when it would end up inside itself. `NestClips` moves clips (with their linked clips) into a new timeline with the same settings, each on the track of the same kind and number, from its start, and lays a linked pair playing it where they were: on the lowest video and the lowest audio track they left; refused when a clip that isn't moved lies between them there.
- **An adjustment layer** is a clip whose `kind` is `Adjustment`, on a video track, with no file. Its quick colour, at its opacity (and their keyframes), applies to everything below it in the picture for as long as it lasts; its place, size, turn and crop mean nothing, and it has no speed. It can be trimmed back as far as wanted (it plays nothing, so `Clip::starts_before_its_source` is never true for it). `AddAdjustment` lays one on the first video track from the chosen one up that is free for its length, or on a new track on top.
- **Templates** leave nested timelines out (a timeline belongs to its project); adjustment layers are kept.

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
| 10 | Clips gain `keyframes` | Version number only: defaults to none |
| 11 | Clips gain `ramp` (speed keyframes) and `blend` (frame blending) | Version number only: defaults to no ramp and no blending |
| 12 | Clips gain `kind`: a file, another timeline (`Timeline(id)`, a nested timeline) or an adjustment layer. Several timelines were stored from format 1 | Version number only: defaults to a file |
| 13 | The project gains `library`: labels on media files, custom lists and a note per file | Version number only: defaults to none |
| 14 | Media streams gain `stated_length`: for a file imported with its end missing, the length it says it has (`duration` is what is really there) | Version number only: defaults to none (a whole file) |
| 15 | The library gains `uses`: how each file is to be added from the Library page (`MediaUse`: in and out marks, a level per sound stream), kept when the Library's marks are saved in the project | Version number only: defaults to none |
| 16 | Tracks gain `sound` (role, pan, effects, volume and pan lines); timelines gain `master_sound` (the mix's compressor and limiter) | Version number only: defaults to every effect off |

### Changing the format

1. Copy the current model types into `src/formats/v<N>.rs` (frozen, never edited again).
2. Change the model and raise `file::FORMAT_VERSION` to `N + 1`.
3. Add a step to `file::MIGRATIONS` that reads version `N` text into the frozen types, converts them, and writes version `N + 1` text.
4. Write the new sample with `WRITE_SAMPLES=1 cargo test -p project --test samples`. Keep every older sample: they must still open.
5. Templates hold the same `Clip`, `Track` and `MediaRef`. A new field with a default needs nothing: the stored template samples are never rewritten, and they prove that a template saved before the field still reads (clips gained `look` that way; a template saved now keeps each clip's look). If old template text no longer reads into the types (a stored template sample stops opening), raise `templates::FORMAT_VERSION` too and add a step to `templates::MIGRATIONS`, the same way; the command in step 4 then writes the new format's sample, once.

## Track sound

The Audio page's settings are project data (format 16), changed by undoable commands like any edit:

- `SetTrackSound` sets an audio track's whole `TrackSound` at once (one undo step per change on the page); refused for a video track (a picture's sound is on its linked audio track) and for values out of range. `SetMasterSound` sets a timeline's `MasterSound`. Volume, mute and solo stay where they were (`SetTrackMix`), and the mix's volume (`SetMasterGain`).
- Track lines are kept in timeline time, unlike clip keyframes: a line is about the mix, not the recording.
- A template's new tracks get the template track's role, pan and effects but not its lines, which are in the time of the timeline it was cut from.
- Turning these numbers into sound is `audio`'s work (`audio::settings::TrackSettings::from_project`).

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

## Media paths

A media file's path is normally a full one. A relative path means "from the project file's folder": `file::resolve_media_paths` makes such paths full whenever a project is opened (`Document::open`) or listed (`file::read_info`'s summary), so the model in memory always holds full paths and a project saved again writes full ones. Packages are written with relative paths so their folder can be moved anywhere; the editor's imports are made full first, so a relative path never comes from anywhere else. No format change: no earlier file has a relative path the app wrote (paths typed on the command line were taken from the folder the app started in, usually the project's).

## The package

`package::plan` and `package::run` make a folder that opens on another machine with nothing missing:

| In the package | Why |
|---|---|
| `<project file>` | A clean save of the project as it is: only the files its timelines use (clips in nested timelines count), each at `Media/<file name>` (`/` on every system; same names get " 2"), labels, lists and notes of those files kept |
| `Media/` | Every file a timeline uses, copied |
| `<file>.thumbnail.png`, `<file>.view` | The launcher's picture and the timeline layout, when there are any |
| `<file>.versions/` | Only when asked (`run`'s `with_versions`; the dialog asks each time): every saved version that can be read, its media paths pointing inside the package, and the files only versions use copied into `Media/` too (`Plan::versions`) |

Left out: the autosave journal and unsaved work set aside after a crash (the copy is a clean save), backups from before upgrades, templates and labels offered in every project (the person's, not the project's), and proxies (they belong to recordings; built again where the package opens). Packaging is refused while a file the timelines use can't be found.

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
- Every change, undo and redo is appended to `<project file>.journal` (one line each) the moment it happens, so a crash of the app loses nothing. Asking the disk to keep it through a power cut too (`sync_data`) takes from under a millisecond to tens of milliseconds, so that happens on a thread of its own (`journal_sync.rs`), within moments, and the interface never waits for the disk: an import of 500 files writes 500 lines, and syncing each on the interface thread took up to 18 ms at a time (phase 5 gate). A power cut can lose the changes of the last moments; a half-written last line is ignored (below).
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
| Set speed | Plays a clip and its linked clips at one speed (1/100 to 100 times as fast; the interface offers 25 % to 400 %): the same part of the file, so the clip gets longer or shorter and everything after it moves along. A ramp gives way to it |
| Paste keyframes | Copied keyframe marks at the playhead on a picture clip, spaced as they were on the timeline: see Keyframes above. A pasted number replaces its keyframe in that frame; marks that would land off the clip are left out; refused when none fits, for a sound clip, or with a value out of range |
| Set ramp | Gives a clip and its linked clips speed keyframes: see Speed ramps above. Speeds that are not fractions of at most 10 000 parts are refused |
| Set frame blending | Off, or blending the two recorded frames around each moment; only clips on video tracks |
| Set settings | Fixes a timeline's size, frame rate and sound rate; it stops being automatic |
| Insert template | One step: everything from the chosen time on, on every track, moves later by the template's length (clips under it are split, labels move too) and the template's clips fill the room, each on the track of the same kind and number. Tracks that are missing are made after the others of their kind, as `AddTrack` does, each as loud as the template's track in that place and with its name when someone chose it ("Music") and no track of that kind has it yet; otherwise it gets the next usual name ("V2", "A3"). A template with fewer tracks than the timeline leaves the other tracks empty for its length: their clips move along like everything else. A file the project already has (same contents: size and sample hash, wherever it is now) is reused; other files are added. Every clip and link gets a new id; clips linked in the template stay linked. A timeline still on Automatic takes its settings from the template's first video. The command carries the whole template, so the journal replays it without the file |
| Add / copy / rename / remove a timeline | See Timelines above. Removing the open one opens the first left |
| Nest clips | The selected clips and their linked clips go into a new timeline; one linked pair of clips playing it takes their place. See Timelines above |
| Place a timeline | A timeline as one piece in another, at a time on a chosen video track, its sound on a free audio track; never inside itself |
| Add adjustment layer | An adjustment layer from a time for a length on the first free video track from the chosen one up, or a new one on top |
| Relink (`Relink`, `RelinkFiles`) | Points a file, or several as one step, at where it is now; refused, with nothing changed, for a file the project doesn't have (or no files). Finding moved files uses `RelinkFiles`, so all that one search found is undone together |
| Prune (`RemoveMedia`) | Takes the chosen files out of the project as one step, with their labels, their places in lists and their notes (the labels and lists themselves stay). Refused, with nothing changed, when a timeline uses any of them (directly or in a nested timeline), for a file the project doesn't have, or for no files. Nothing on disk is touched; undo brings them back |
| Import | One step: adds the file and places it at the end of the timeline, the picture on the first video track and each sound stream on its own audio track (made when missing; an unused default track takes the stream's name), all linked. Pictures last 5 seconds. On an automatic timeline the first video also sets the settings (upright size, frame rate, first sound stream's rate) |
