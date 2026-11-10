# xtask

Project chores written in Rust, so they run the same on Windows and Linux. Run them with `cargo xtask <task>` (the alias lives in `.cargo/config.toml`).

| Task | What it does |
|---|---|
| `check-media` | Checks that every file listed in `test-media/MANIFEST.md` is present and has the listed SHA-256 checksum |
