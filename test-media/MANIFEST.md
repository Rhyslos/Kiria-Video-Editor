# Test media manifest

The recordings every phase gate is tested against. The files themselves are large and are not in git: keep them in a shared folder and copy them into `test-media/`. Run `cargo xtask check-media` to confirm every file is present and unchanged.

Add one row per file. The checksum is the file's SHA-256 (on Windows: `Get-FileHash <file>`; on Linux: `sha256sum <file>`).

| File | SHA-256 | What it tests |
|---|---|---|

## Files still to collect

- 1080p 60 fps H.264 game recording, about 5 minutes
- 1440p 144 fps recording
- Variable frame rate recording, about 1 hour (audio sync over time)
- H.265 and AV1 recordings from the AMD encoder
- H.264 and H.265 recordings from the NVIDIA encoder
- MKV with three audio streams (game, microphone, voice chat)
- Facecam clip (auto white balance, background removal)
- Facecam clip with hair against a busy background (background removal)
- A few deliberately broken files (truncated, wrong extension, unsupported codec)
