# packaging

Everything that turns a build into something people install. The release workflow (`.github/workflows/release.yml`) runs these; see `RELEASING.md` for the steps.

| File | What it does |
|---|---|
| `ffmpeg.env` | Which FFmpeg build is bundled (8.1 series, GPL, shared libraries) |
| `icons/` | The app icon as `icon.png` and `icon.ico`, drawn by `cargo xtask icons packaging/icons` |
| `windows/stage.ps1` | Builds the install layout (`launcher.exe`, `current`, `versions/<version>/` with the editor, FFmpeg and licences) and the update package `<name>-<version>-windows-x64.zip` |
| `windows/installer.iss` | Inno Setup script for `<name>-Setup-<version>.exe`: per-user install, Start menu and optional desktop shortcut to the launcher, clean uninstall |
| `linux/build-appimage.sh` | Builds `<name>-<version>-linux-x86_64.AppImage` with the launcher, editor, FFmpeg and licences |
| `linux/AppRun` | The AppImage's entry point: the launcher with the bundled libraries on the library path |

The app name, id and version come from `cargo xtask brand`, so they are never typed here.
