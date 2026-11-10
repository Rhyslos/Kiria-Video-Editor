# launcher

The small program the Start menu and desktop shortcuts open.

## Status

Phase 1: starts the editor at once, then checks for updates in the background and, with automatic updates on, downloads and verifies the new version. The project browser window arrives in Phase 2 and installing downloaded updates in Phase 3.

## Behaviour

1. Logs to `<data folder>/logs/launcher.<date>.log` (no crash detection; that belongs to the editor).
2. Finds the editor with `updater::Layout`: `versions/<current>/editor` next to the launcher, or an editor right next to it (development builds, the AppImage).
3. Starts it with everything passed on the command line except `--no-update-check` (a project file, test options). Being offline never delays this: measured at under 0.1 s from launcher start to editor start with no network.
4. Unless `--no-update-check` is given: checks GitHub (3-second limit). If a newer version is available and automatic updates are on, downloads it to `<data folder>/downloads/<version>/` and verifies it.

Preferences are read from `<data folder>/updates.ron` (defaults: automatic, Stable channel).
