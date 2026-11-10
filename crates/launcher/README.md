# launcher

The small program the Start menu and desktop shortcuts open: the project browser and the update panel.

## Status

Phase 2 in progress. Done: the project browser (folder tree, project grid, search, Open…, New project with presets, rename, move, remove from list) and the version panel (check, Update, Skip this version, Automatic updates). Updates install themselves: a verified download is installed the next time the launcher starts, or at once with "Restart to update". Still to come: missing-media checks with relinking, the crash-recovery prompt, real thumbnails, and rolling back a version that fails to start (Phase 3).

## Behaviour

1. Logs to `<data folder>/logs/launcher.<date>.log` (no crash detection; that belongs to the editor).
1. In an installed copy, installs an update downloaded on an earlier run (`updater::apply`), checking its signature and checksum again first. If the new version brings a newer launcher, that is swapped in and started instead, with the same arguments. The window then says "Updated to x.y.z"; a failure is shown in the error strip and the old version keeps working.
2. With a project file as argument (`launcher trip.kiria`), starts the editor with it at once, lists it for next time and checks for updates. Being offline never delays this.
3. Otherwise opens the browser window:
   - **Top bar** from `ui-kit` (`TitleBar`): logo and name centred, settings on the right.
   - **Left**: All projects, then the three folder groups and "+ Add folder". On the first run a `<name> projects` folder is made in Documents. Right-click a folder to show it or remove it from the list.
   - **Bottom left**: the version panel (`updates.rs`).
   - **Main**: the 3-wide grid of project cards, most recently used first, with search, Open… and New project. Click selects, double-click or Enter opens, right-click offers Open, Rename…, Move to, Show in file manager and Remove from list.
4. Opening a project starts the editor with its path, notes the time in the index and closes the window. A download still running finishes before the process exits.
5. Finding projects on disk, renames, moves, file dialogs and update checks all run on worker threads (`work.rs`); the interface thread only reads the index.

## Where projects are saved

New projects go to one chosen folder, shown with a quiet "new projects" badge in the folder tree. On the first run that is the `<name> projects` folder made in Documents. It can be changed three ways:

- **Settings (⚙) → New projects → Change…** picks any folder.
- **Right-click a folder → Save new projects here.**
- **New project → Browse…** saves one project anywhere; ticking *Save new projects here from now on* makes that folder the choice.

A folder picked this way is added to the Projects group (unless a listed folder already contains it), so its projects show up in the grid. While a project folder is open in the grid, New project suggests that folder instead.

Preferences are read from `<data folder>/updates.ron` (defaults: automatic, Stable channel) and `<data folder>/launcher.ron` (the folder for new projects). The theme follows the editor's `settings.ron`.

## Files

| File | What it holds |
|---|---|
| `src/main.rs` | Start-up: options, opening a project directly, or the browser window |
| `src/browser.rs` | The window: panels, folder tree, header, dialogs, keyboard, automated runs |
| `src/cards.rs` | The project grid and its cards; search filtering |
| `src/new_project.rs` | The New project dialog and its presets |
| `src/updates.rs` | The version panel and its background check and download |
| `src/files.rs` | Creating, renaming and moving project files (the autosave journal moves along) |
| `src/editor_start.rs` | Starting the editor; showing a folder in the file manager |
| `src/work.rs` | `Task`: run work on a thread, pick the result up on a later frame |
| `src/demo.rs` | Sample projects in a temporary folder for screenshots and tests |
| `src/prefs.rs` | `LauncherPrefs` in `launcher.ron`: the folder new projects are saved in |
| `src/run_mode.rs` | Command-line options |

## Trying it without installing

```sh
cargo run -p launcher -- --demo 14            # sample projects in a temporary folder
cargo run -p launcher -- --demo 200 --smoke-test
xvfb-run -a -s "-screen 0 1600x1000x24" target/debug/launcher --demo 14 --screenshot launcher.png
```

With `--demo`, opening a project only reports what would happen; the editor is not started and nothing outside the temporary folder is touched.
