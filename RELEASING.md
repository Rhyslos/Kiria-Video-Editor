# Releasing

Every release is built, tested, signed and published by GitHub Actions. Starting it is the only manual step.

## Test builds

Every pull request runs the same Release workflow without publishing anything. Its run page has a `windows` artifact (the installer and update package) for trying the change before it is merged.

A test build has the version an automatic beta of it would have (`<next patch>-beta.<run number>`), so its installer goes over the newest beta, and it follows later betas by itself.

A pull request's build is never signed: its run skips the signing job, so the code of a change that was not merged never sees the signing key, and no installed copy would ever take its build as an update. Install it from the `windows` artifact by hand. Because the drill below needs a signed build, a pull request's run skips the drill too; the first push to `main` after merging runs it before that beta is published.

## The update drill

Twice now a release went out that installed copies could not update to, so the workflow proves it before every release and every automatic beta (`packaging/windows/update-drill.ps1`). Pull requests skip it: it needs a manifest signed with the real key, because the previous release trusts only the public key built into it (a throwaway key would be refused, and `cargo xtask sign` refuses any key but the real one), and pull requests are never signed. A change that breaks updating is therefore found by the run on `main` after merging, which then fails before publishing anything:

- **Before publishing** it installs the previous release on a clean Windows machine and lets it update to the new build by itself: find it, download it, check the signature, install it, start the editor, replace the launcher, and then say "up to date" instead of offering the update again. The previous release reads the list of releases from the runner (the files about to be published) through the `<NAME>_UPDATE_FEED` variable. If any step fails, nothing is published.
- **After publishing** it does the same once more with the previous release asking GitHub itself, as everybody's copy does. A release that installed copies can't find or fetch turns the run red within minutes. If a newer release was published meanwhile, installed copies go straight to that one, and the check says so instead of failing. Releases of one branch run one at a time (the workflow's `concurrency` group), so two pushes to `main` close together publish one after the other; of several waiting, only the newest runs.
- It also installs the new build on its own and runs one step, so every release can be the starting point of the next drill.

Each step is the launcher's own `--update-step <report file>`: what a normal start does about updates, without a window. The deliberately broken rollback-test build skips the drill.

The Linux AppImage is paused for now: releases and betas are Windows only. The workflow keeps the Linux job, switched off, with a comment saying how to turn it back on; the Linux lines below apply again then.

## Automatic betas

Every push to `main` (except documentation-only changes) publishes a beta release with no manual step: version `<next patch>-beta.<run number>`, for example `0.1.1-beta.12` after `0.1.0`. Beta builds follow betas by themselves, so a copy installed from a beta picks up every merged feature on its next start. Stable copies only see betas when *Betas too* is chosen in the launcher's settings.

Only the ten newest automatic betas stay on the releases page: after publishing, the workflow removes older ones and their tags (`packaging/prune-betas.sh`). Stable releases are never removed.

## When a version needs a newer launcher

Raise `MIN_LAUNCHER_VERSION` in `crates/brand/src/lib.rs`. Installed copies with an older launcher then replace it as soon as they install that version, instead of waiting until it has started properly.

## Rollback test build

To check that installed copies recover from a broken update: **Actions → Release → Run workflow**, leave the tag empty and tick **crash_test**. It publishes a beta whose editor crashes on start (the install tests skip starting it). A beta copy downloads it, fails to start it twice, switches back to the previous version and explains why; that beta is never offered again. Normal betas carry on afterwards.

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
   - runs the update drill from the previous release (above),
   - publishes the GitHub release with all files, but only if every step above passed,
   - removes old automatic betas, and runs the drill once more against GitHub itself.
6. Installed copies find it the next time they open.

## The signing key

Updates are only installed if `manifest.json.sig` matches the public key built into the app (`crates/updater/src/manifest.rs`). The private key is the secret `RELEASE_SIGNING_KEY` and must never be committed.

If the key is ever lost or leaked: make a new one with `cargo xtask keygen <file>`, replace `RELEASE_PUBLIC_KEY`, store the new secret in the `release` environment (below), and release. Copies installed before that release need a manual reinstall, because they only trust the old key.

### Where the signing key is kept

Only the Release workflow's `sign` job uses the key. It runs in a GitHub environment named `release` (`environment: release`) and is skipped for pull requests. The environment is what keeps the key away from code that was not merged: a pull request's run uses the workflow file from the pull request, so an `if:` alone can be edited away, but an environment limited to `main` and `v*` tags hands its secrets to no other branch, whatever the workflow file says. A repository secret, by contrast, is given to a workflow on any branch.

Set it up once (needs admin rights on the repository):

1. **Settings → Environments → New environment**, name it `release` exactly, and click **Configure environment**. (If the workflow has run since this change, `release` is already listed, made by GitHub with no rules: open it instead.)
2. Under **Deployment branches and tags**, choose **Selected branches and tags**, then **Add deployment branch or tag rule** twice: *Ref type* **Branch**, pattern `main`; and *Ref type* **Tag**, pattern `v*`.
3. Under **Environment secrets**, **Add environment secret**: name `RELEASE_SIGNING_KEY`, the same value as the repository secret (keep it from where the key is stored safely; a secret's value can't be read back from GitHub).
4. **Settings → Secrets and variables → Actions → Repository secrets**: delete `RELEASE_SIGNING_KEY` there. Until it is deleted, any branch's workflow can still read it.
5. Check: push to `main` (or wait for the next merge). The Release run's **Sign the release** job shows the `release` environment and its manifest step does not print "RELEASE_SIGNING_KEY is not set".

Nothing breaks before these steps are done: a job with an environment gets the repository's secrets as well as the environment's (an environment secret only wins over a repository secret of the same name), and GitHub creates a missing environment, without rules, the first time a job names it. So the workflow keeps signing with the repository secret until the key is moved.

After the steps, hand-started runs (**Run workflow**) sign only when started from `main` or a `v*` tag; started from another branch, the signing job is refused by the environment and the run fails. Start releases and rollback tests from `main`, as before.

## Windows "unknown publisher" warning

The installer is not signed with a paid code-signing certificate, so Windows SmartScreen may say "Windows protected your PC". Click **More info**, then **Run anyway**. This is expected for a hobby project; the update signature above still protects every update.
