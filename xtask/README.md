# xtask

Project chores written in Rust, so they run the same on Windows and Linux. Run them with `cargo xtask <task>` (the alias lives in `.cargo/config.toml`).

| Task | What it does |
|---|---|
| `check-media` | Checks that every file listed in `test-media/MANIFEST.md` is present and has the listed SHA-256 checksum |
| `brand` | Prints the app's name, id and version as `KEY=value` lines for CI |
| `beta-version <run>` | Prints the version, `BUILD_VERSION` and tag of an automatic beta (next patch, `-beta.<run>`) |
| `keygen`, `manifest`, `sign`, `verify` | The release signing key and the signed file list (see `RELEASING.md`) |
| `icons <folder>` | Draws the app icons |
