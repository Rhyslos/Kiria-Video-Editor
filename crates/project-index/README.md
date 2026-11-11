# project-index

The launcher's list of known projects and folders.

## Status

Phase 2.1 done: the index file, finding projects (created here, opened from disk, or found in a project folder) and reading only each file's summary.

## How it works

A small SQLite file, `projects.sqlite` in the app's data folder, with two tables:

- `folders`: every folder in the launcher's tree and its group (Projects, Media folders, Other folders).
- `projects`: one row per known project file: name, picture size, frame rate, length, when it was last opened, when the file last changed, any issue (missing, saved by a newer version, damaged), and whether it was removed from the list.

`Index::projects` is one query and is safe on the interface thread (200 projects list in well under a millisecond). `Index::refresh` walks the project and other folders (three levels deep, hidden folders skipped), re-reads only files whose modification time changed, and marks files that disappeared; the launcher runs it on a worker thread with its own connection. SQLite runs in WAL mode so both connections work at once.

Saved versions are project files too, kept in `<project file>.versions/` beside their project. They belong to that project and are not listed: the search skips those folders, and a refresh drops version files an earlier search listed (unless one was opened on purpose).

Reading a project never loads it: `project::file::read_info` reads only the format version and summary at the top of the file.

"Remove from list" hides a project instead of deleting the row, so a refresh doesn't bring it back; opening it again lists it again.

"Delete project…" does delete the row (`Index::deleted`), once the files are in the recycle bin: no card is left saying the file is missing, and a project restored from the recycle bin into a project folder is found again by the next refresh.

## Files

| File | What it holds |
|---|---|
| `src/entry.rs` | `Folder`, `FolderKind`, `Issue` (with the card text) and `ProjectEntry` |
| `src/store.rs` | `Index`: open, folders (`add_folder_if_new` skips folders a listed one already covers), record, refresh, mark opened, forget, follow a move, drop a deleted project. Table layout and its upgrade steps (`SCHEMA_VERSION`; version 2 added `automatic`) |
| `src/scan.rs` | `find_project_files`: project files in a folder and up to three levels below, leaving out saved versions (`<project file>.versions/`) |

## Rules

- Changing the tables: raise `SCHEMA_VERSION` and add a step to `upgrade`. A file from a newer version is refused with an explanation, never changed.
- Times are seconds since 1970 (`now()`); lengths and frame rates are stored as exact fractions.
