# Releasing

Every release is built, tested, signed and published by GitHub Actions. Starting it is the only manual step.

## Test builds

Every pull request runs the same Release workflow without publishing anything. Its run page has a `windows` artifact (the installer and update package) and a `linux` artifact (the AppImage) for trying the change before it is merged.

## Automatic betas

Every push to `main` (except documentation-only changes) publishes a beta release with no manual step: version `<next patch>-beta.<run number>`, for example `0.1.1-beta.12` after `0.1.0`. Beta builds follow betas by themselves, so a copy installed from a beta picks up every merged feature on its next start. Stable copies only see betas when *Betas too* is chosen in the launcher's settings.

## Version numbers

- `0.x.y` until version 1.0. Raise `y` for fixes, `x` for new features.
- A version with a dash is a beta, for example `0.3.0-beta.1`. Betas only reach people on the Beta update channel.
- The project file format has its own number (`project::file::FORMAT_VERSION`) that only ever goes up, independently of the app version.

## Steps

1. Set `version` under `[workspace.package]` in `Cargo.toml` to the new version and commit.
2. Add a section for it at the top of `CHANGELOG.md`: what changed, in plain words.
3. Check that CI is green on `main`.
4. Start the release, either way:
   - push a tag: `git tag v0.1.0 && git push origin v0.1.0`, or
   - on GitHub: **Actions** → **Release** → **Run workflow**, type the tag (`v0.1.0`) and run it.
5. Wait for the **Release** workflow (about 15 minutes). It:
   - builds the editor and launcher for Windows and Linux,
   - bundles FFmpeg and every licence,
   - builds the Windows installer and the Linux AppImage,
   - installs the Windows build silently, starts it, and uninstalls it again,
   - starts the Linux build on a virtual screen,
   - writes `manifest.json` (every file's size and checksum), signs it, and checks the signature,
   - publishes the GitHub release with all files, but only if every step above passed.
6. Installed copies find it the next time they open.

## The signing key

Updates are only installed if `manifest.json.sig` matches the public key built into the app (`crates/updater/src/manifest.rs`). The private key is the repository secret `RELEASE_SIGNING_KEY` and must never be committed.

If the key is ever lost or leaked: make a new one with `cargo xtask keygen <file>`, replace `RELEASE_PUBLIC_KEY`, store the new secret, and release. Copies installed before that release need a manual reinstall, because they only trust the old key.

## Windows "unknown publisher" warning

The installer is not signed with a paid code-signing certificate, so Windows SmartScreen may say "Windows protected your PC". Click **More info**, then **Run anyway**. This is expected for a hobby project; the update signature above still protects every update.
