# library

The media index and search: one project's media in SQLite, answering searches, filters and orders fast enough for thousands of files.

## Status

Phase 5.1 done: the index (types, labels, custom lists, notes, use on timelines, full-text search) and its worker; the Edit page's media drawer ("Find a file") and the search palette ask it. Media folders are watched for new recordings. The Library page itself is Phase 5.2.

## Where things live

- **Labels, lists and notes are project data** (`project::Library`, format 13), changed only by commands (`AddMediaLabel`, `SetMediaLabel`, `SetMediaNote`, `AddMediaList`, `SetListMember`, …). So they undo and redo like every other edit, go into the autosave journal, come back after a crash, travel with the project file and restore with a saved version.
- **Labels can also be offered in every project** (`project::GlobalLabels`, `labels.ron` in the app's data folder, like templates): only names and colours. Putting one on a file copies it into the project's own labels first, so the index, like the project file, only ever sees project labels.
- **The index is derived.** Everything in it is built from the project (`Index::sync`), in memory, when a project opens; after that each sync writes only the files that changed. There is no index file on disk, so there is nothing to list in `project::side_files`, nothing to migrate, and nothing that can disagree with the project file. 10 000 files are indexed in well under a second on a worker.

## How it works

| File | What it holds |
|---|---|
| `src/index.rs` | `Index`: the tables (below), `sync(project)` (compares each file's row with the last sync and rewrites only those that changed, in one transaction; labels and lists are rewritten when they change), `search(query)` (one SQL query built from the `Query`), `usage(media)` (clips per timeline) and `with_fingerprint` (the same recording imported twice) |
| `src/query.rs` | `Query`: words, types (`Type`: Video, Audio, Image, Effect), labels (all must be on a file), a list, `Usage` (any, used, unused) and `Sort` (name, newest, oldest, longest, shortest, type, most used, a list's own order) |
| `src/live.rs` | `Live`: the index on a thread of its own. `follow(project)` hands it a snapshot when the media, the library or the timelines changed (pointer comparisons: free every frame); `ask(asker, query)` asks for one asker ("media drawer", "search palette"); `answer(asker)` reads the newest answer without waiting, and `is_current` says whether it is to the newest query and project. After each sync every asked query is answered again, so lists follow edits and undo |
| `src/watch.rs` | Media folders watched for new recordings. `Scanner::look` lists video, sound and picture files (`EXTENSIONS`, the import dialog's) in the folders and three levels below (hidden ones skipped) changed after a moment; a file counts (`Found`) once it has the same size and modification time on two looks in a row and opens for reading, so a recording still being written waits until it stops growing. `Watcher` runs it on a thread of its own every `SETTLE` (2 s), asking for the folders before each look (so a folder added in the launcher meanwhile is watched), and publishes the finished files newest first; `found()` never waits. Polling, not system change events: standard library only, the same on every system and on network drives |
| `tests/index.rs` | What each query finds, syncs that follow edits and undo, the worker, and the phase 5 gate: 10 000 files, every kind of query under 100 ms |

Tables:

| Table | Holds |
|---|---|
| `types` | Video, Audio, Image, Effect (effects arrive with the effects library) |
| `media` | One row per file of the project, keyed by its `MediaId`, with its fingerprint (size and hash), type, name, path, length, note, the clips using it, and its words in lower case |
| `labels`, `media_labels` | Each label (name, colour, order) and which files have it |
| `lists`, `list_members` | Each custom list and its files, in order |
| `usage` | Clips per file per timeline (nested timelines and adjustment layers play no file) |
| `search` | Full-text search (FTS5, trigram) over each file's name, label names and note |

## Searching

- Words are matched in any case, **anywhere** in a file's name, its labels' names and its note (`dji` finds `DJI_0042.mp4`, `0042` too); every word must match. Words of three letters or more go through the full-text index; shorter ones are looked up in the lower-case words column.
- What is typed is always quoted, so nothing typed is read as search syntax.
- Filters (types, labels, a list, used or not) narrow the same query; the order is the `Sort`.

## Rules

- Never write project data here: change the project with a command and sync.
- Never query on the interface thread: use `Live` (queries take a few milliseconds for 10 000 files, but a sync after a big import takes longer).
