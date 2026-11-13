# packaging

Everything that turns a build into something people install. The release workflow (`.github/workflows/release.yml`) runs these; see `RELEASING.md` for the steps.

| File | What it does |
|---|---|
| `ffmpeg.env` | Which FFmpeg build is bundled (8.1 series, GPL, shared libraries) |
| `icons/` | The app icon. `icon.svg` is the drawing (a shaded colour wheel with a puck); `cargo xtask icons` draws `icon.png`, `icon.ico` and the interface's `crates/ui-kit/assets/icon/icon-64.png` from it (the `.ico` keeps bitmaps up to 128 px and a PNG at 256 px, which every part of Windows shows). Both programs build `icon.ico` in on Windows (their `build.rs`). Change the drawing, never the drawn files |
| `windows/stage.ps1` | Builds the install layout (`launcher.exe`, `icon.ico`, `current`, `versions/<version>/` with the editor, a copy of the launcher, the icon, FFmpeg, licences and the user guide (`user-guide/`: `docs/user-guide/index.html` and its `images/`, so each version brings the guide that describes it); the launcher copies a version's icon over the shortcuts' one when they differ) and the update package `<name>-<version>-windows-x64.zip` |
| `windows/installer.iss` | Inno Setup script for `<name>-Setup-<version>.exe`: per-user install, Start menu and optional desktop shortcut to the launcher, clean uninstall |
| `linux/build-appimage.sh` | Builds `<name>-<version>-linux-x86_64.AppImage` with the launcher, editor, FFmpeg, licences and the user guide (`usr/share/user-guide/`, where the programs look from `usr/bin`) |
| `release-env.sh` | Works out the version and release tag of a workflow run: a tag, a typed tag, an automatic beta for a push to `main`, or a deliberately broken rollback-test beta (`crash_test`) |
| `windows/update-drill.ps1` | The update drill: installs the previous release on the workflow's clean machine and lets it update to the new build by itself, before publishing (from files served on the runner) and after (from GitHub). Refuses to run anywhere else: it empties the app's data folder |
| `prune-betas.sh` | Removes all but the ten newest automatic betas (and their tags) from the releases page, after each release is published; `--dry-run` lists them, `--self-test` checks the choice |
| `linux/AppRun` | The AppImage's entry point: the launcher with the bundled libraries on the library path |

The app name, id and version come from `cargo xtask brand`, so they are never typed here.
