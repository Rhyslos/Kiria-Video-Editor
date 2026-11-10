# updater

Release checks, signature checks and downloads. Installing side by side and rolling back arrive in Phase 3.

## Status

Phase 1 done: checking GitHub, verifying the signed manifest, resumable verified downloads, update preferences, and the install layout the launcher reads.

## Files

| File | What it holds |
|---|---|
| `src/release.rs` | `check()`: asks GitHub for releases (3-second limit) and picks the newest for the channel; `decide()` is the testable part. Tags must look like `v1.2.3`; anything else (drafts, `deps-…`) is ignored |
| `src/manifest.rs` | `Manifest` (version, channel, every file's size and SHA-256), `verify_and_parse()` against `RELEASE_PUBLIC_KEY`, and `sign()` for the release workflow |
| `src/download.rs` | `download()`: resumes a `.part` file, verifies size and checksum, only then renames it into place |
| `src/fetch.rs` | `fetch()`: manifest and signature first, then the package for this platform |
| `src/install.rs` | `Layout`: `current` file and `versions/<version>/` folders; finds the editor to start |
| `src/prefs.rs` | `UpdatePrefs`: automatic on/off, channel, skipped version; `load` / `save` to `updates.ron` in the data folder |

## Trust

Nothing from a release is used unless:
1. `manifest.json.sig` is a valid ed25519 signature of `manifest.json` by the key whose public half is `RELEASE_PUBLIC_KEY`, and
2. the downloaded file's size and SHA-256 match the manifest.

A failure at either step deletes the download and explains that nothing was installed. See `RELEASING.md` for the key itself.
