# updater

Release checks, signature checks, downloads and installing. Rolling back a version that fails to start arrives in Phase 3.

## Status

Phase 1 done: checking GitHub, verifying the signed manifest, resumable verified downloads, update preferences, and the install layout the launcher reads. Phase 3 started: installing a verified download beside the current version, switching to it, and swapping in a newer launcher.

## Files

| File | What it holds |
|---|---|
| `src/release.rs` | `check()`: asks GitHub for releases (3-second limit) and picks the newest for the channel (a beta build always follows betas); `decide()` is the testable part. Tags must look like `v1.2.3`; anything else (drafts, `deps-…`) is ignored |
| `src/manifest.rs` | `Manifest` (version, channel, every file's size and SHA-256), `verify_and_parse()` against `RELEASE_PUBLIC_KEY`, and `sign()` for the release workflow |
| `src/download.rs` | `download()`: resumes a `.part` file, verifies size and checksum, only then renames it into place |
| `src/fetch.rs` | `fetch()`: manifest and signature first, then the package for this platform. Both are kept in the download folder so they can be checked again before installing |
| `src/install.rs` | `Layout`: `current` file and `versions/<version>/` folders; finds the editor to start |
| `src/apply.rs` | Installing: `pending` (a verified download newer than the running version), `install` (unpack beside the current version, then switch `current`; or replace the AppImage), `prune`, `replace_launcher` |
| `src/prefs.rs` | `UpdatePrefs`: automatic on/off, channel, skipped version; `load` / `save` to `updates.ron` in the data folder |

## Trust

Nothing from a release is used unless:
1. `manifest.json.sig` is a valid ed25519 signature of `manifest.json` by the key whose public half is `RELEASE_PUBLIC_KEY`, and
2. the downloaded file's size and SHA-256 match the manifest.

A failure at either step deletes the download and explains that nothing was installed. See `RELEASING.md` for the key itself.

The same two checks run again right before a download is installed, so a file changed on disk in between is refused too.
