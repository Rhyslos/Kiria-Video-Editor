# brand

The one place the app's name and identity live.

Name, short name, app id, data-folder name, the GitHub repository used for update checks, `VERSION` (the workspace version, or `BUILD_VERSION` for automatic betas) and `MIN_LAUNCHER_VERSION` (the oldest launcher that can install this version; raise it when a version needs a newer launcher). Every window title, installer, update check and folder path reads from here, so renaming the app is a one-line change. A test fails the build if the name is typed anywhere else in the Rust code.

## Status

Done (Phase 0.1).
