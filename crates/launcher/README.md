# launcher

The small program the Start menu and desktop shortcuts open: the project browser and the update panel.

## Status

Phase 2 in progress. Done: the project browser (folder tree, project grid, search, Open…, New project with presets, rename, move, remove from list) and the version panel (check, Update, Skip this version, Automatic updates). Updates install themselves: a verified download is installed the next time the launcher starts, or at once with "Restart to update". Still to come: missing-media checks with relinking, the crash-recovery prompt, real thumbnails, and rolling back a version that fails to start (Phase 3).

## Behaviour

1. Logs to `<data folder>/logs/launcher.<date>.log` (no crash detection; that belongs to the editor).
1. Opens its window through the [graphics](../graphics/) crate, as the editor does: on Windows Vulkan first, Direct3D 12 when Vulkan reports an error, and its own start marker (`launcher-vulkan.starting`) for a driver that crashes or freezes instead. See Graphics below.
1. In an installed copy, installs an update downloaded on an earlier run (`updater::apply`), checking its signature and checksum again first. The launcher itself is replaced later, once the new version has started properly, so a broken update can never take the recovery with it. Only when the update's manifest needs a newer launcher is it swapped in at once (after its `--self-test`) and started instead, with the same arguments. The window then says "Updated to x.y.z"; a failure is shown in the error strip and the old version keeps working.
2. With a project file as argument (`launcher trip.kiria`), starts the editor with it at once, lists it for next time and checks for updates. Being offline never delays this.
3. Otherwise opens the browser window:
   - **Top bar** from `ui-kit` (`TitleBar`): logo and name centred, settings on the right.
   - **Left**: All projects, then the three folder groups and "+ Add folder". On the first run a `<name> projects` folder is made in Documents. Right-click a folder to show it or remove it from the list.
   - **Bottom left**: the version panel (`updates.rs`).
   - **Main**: the 3-wide grid of project cards, most recently used first, with search, Open… and New project. Click selects, double-click or Enter opens, right-click offers Open, Rename…, Move to, Show in file manager and Remove from list.
4. Opening a project starts the editor with its path, notes the time in the index and closes the window. A download still running finishes before the process exits.
5. In an installed copy the launcher then waits, without a window, up to 20 seconds for the editor to report that its first window is on screen (`updater::health`). Two failed starts in a row switch back to the previous version, mark the failed one so it is never offered again, and open the launcher to explain (with Report the problem and Show crash reports).
5. Finding projects on disk, renames, moves, file dialogs and update checks all run on worker threads (`work.rs`); the interface thread only reads the index.

## Crash recovery

If the editor didn't close properly last time (its session marker is still there and no editor is running), the launcher says so at start-up:

- with unsaved work in the project that was open: **Kiria closed unexpectedly** with **Recover and open**, **Open last save** (the work is kept beside the project as `<file>.journal.discarded`) and **Not now**;
- with nothing unsaved: a line under the header saying nothing was lost.

The marker is then cleared so the editor doesn't mention it again. Cards of projects with unsaved crash work say so, and opening one asks the same question. `--demo 8 --demo-crash` shows the window with sample projects.

## Graphics

The launcher and the editor share one Graphics setting (`<data folder>/graphics.ron`) and one way of starting; all of it lives in the [graphics](../graphics/) crate.

- If the last start with Vulkan never drew a frame, this start uses Direct3D 12 without loading the Vulkan driver, and the window asks: **The window didn't open last time**, with **Keep Direct3D 12** and **Try Vulkan again next time**. Closing without answering keeps Direct3D 12. It waits for the rollback and crash-recovery windows if one of those is open.
- **Settings (⚙) → Graphics** has the same row as the editor's Settings: Automatic, Vulkan or Direct3D 12, for every window from the next start, and what is in use now. Diagnostics shows it on its first line.
- `--smoke-test`, `--screenshot` and `--demo` only read the setting and the marker: they write no marker, change no setting and ask nothing. `--self-test` answers before any of this.
- `--graphics-warning` shows the question although nothing failed, for screenshots.
- A Vulkan crash in the editor is one failed start for the health check below; the editor's next start avoids Vulkan and reports that it started, so no version is rolled back (`a_vulkan_crash_in_the_editor_changes_the_graphics_not_the_version` in `src/main.rs`).

## The update window

When a check finds a newer version, a window opens over the browser: **Update available** with **Download** and **Skip this version**. Download turns it into a progress bar (MB done of MB total; **Hide** lets it carry on in the background). When the download is checked, **Update ready** offers **Restart now** or **Later** (it installs the next time the app opens). With automatic updates on, the window opens straight at the progress bar. Closing it never cancels anything: the version panel at the bottom left offers the same choices.

`--update-window <available | downloading | ready>` opens it with a sample version, for screenshots.

## Where projects are saved

New projects go to one chosen folder, shown with a quiet "new projects" badge in the folder tree. On the first run that is the `<name> projects` folder made in Documents. It can be changed three ways:

- **Settings (⚙) → New projects → Change…** picks any folder.
- **Right-click a folder → Save new projects here.**
- **New project → Browse…** saves one project anywhere; ticking *Save new projects here from now on* makes that folder the choice.

A folder picked this way is added to the Projects group (unless a listed folder already contains it), so its projects show up in the grid. While a project folder is open in the grid, New project suggests that folder instead.

Preferences are read from `<data folder>/updates.ron` (defaults: automatic, Stable channel) and `<data folder>/launcher.ron` (the folder for new projects). The theme follows the editor's `settings.ron`; the Graphics choice is in `<data folder>/graphics.ron`, shared with the editor.

## Files

| File | What it holds |
|---|---|
| `src/main.rs` | Start-up: options, opening a project directly, or the browser window (opened with the first graphics system that starts) |
| `src/browser.rs` | The window: panels, folder tree, header, dialogs, keyboard, automated runs |
| `src/cards.rs` | The project grid and its cards; search filtering |
| `src/new_project.rs` | The New project dialog: Automatic settings by default (the first video imported decides), or one of the templates |
| `src/updates.rs` | The version panel, the update window (Update available → download bar → Update ready), and the background check and download |
| `src/files.rs` | Creating, renaming and moving project files (the autosave journal moves along) |
| `src/editor_start.rs` | Starting the editor; showing a folder in the file manager |
| `src/thumbnails.rs` | The pictures on the cards: read on a worker (four at a time) when a card first shows, and again when its project changes |
| `src/work.rs` | `Task`: run work on a thread, pick the result up on a later frame |
| `src/demo.rs` | Sample projects in a temporary folder for screenshots and tests |
| `src/recovery.rs` | Unsaved work left by a crash: reads the editor's session marker and project journals |
| `src/prefs.rs` | `LauncherPrefs` in `launcher.ron`: the folder new projects are saved in |
| `src/run_mode.rs` | Command-line options |
| `src/drill.rs` | `--update-step <report file>`: one start's worth of updating without a window (install what is waiting, look for a newer version and download it, start the editor and wait for it to work), then a `key=value` report. The release workflow's update drill is made of these |

## Trying it without installing

```sh
cargo run -p launcher -- --demo 14            # sample projects in a temporary folder
cargo run -p launcher -- --demo 200 --smoke-test
xvfb-run -a -s "-screen 0 1600x1000x24" target/debug/launcher --demo 14 --screenshot launcher.png
```

With `--demo`, opening a project only reports what would happen; the editor is not started and nothing outside the temporary folder is touched.
