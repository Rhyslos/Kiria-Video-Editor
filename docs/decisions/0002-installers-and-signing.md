# 0002 · Installers, FFmpeg bundling and signing

**Date:** 10 November 2026 (point 2 changed 18 November 2026)

**Decision:**
1. The Windows installer is built with Inno Setup (a per-user `Setup.exe`) and the Linux build is an AppImage, both from our own scripts in `packaging/`, instead of a generated MSI.
2. Every build and release uses one exact FFmpeg 8.1 GPL shared build: `packaging/ffmpeg.env` names a dated release of the build host and the SHA-256 of each archive, and an archive with another hash is refused. The build and its checksum are written into `licenses/FFMPEG-VERSION.txt` of every release. The pin is bumped on purpose (`packaging/README.md`), at the latest when the host removes the pinned dated release.
3. No paid Windows code-signing certificate. Updates are protected by our own ed25519 signature instead.

**Why:**
1. The install layout (`versions/<version>/` with the launcher outside) is what makes side-by-side updates and rollback possible in Phase 3; generated installers assume one flat folder. Inno Setup is free, ships on GitHub's Windows runners, needs no administrator rights for a per-user install, and installs silently for the automated test.
2. At first only the series was pinned (a moving "latest" download, its checksum merely recorded), so whatever the host had that day went into signed installers and no build could be repeated. A dated release with a checked hash makes builds repeatable and refuses a changed or damaged upstream file. The host keeps dated releases only for a while, so the pin has to be bumped from time to time; CI then fails loudly instead of shipping something unknown, and bumping is when security fixes come in.
3. A certificate costs money every year and only removes the "unknown publisher" warning on first install.

**Instead of:** an MSI from a release-tooling generator; the newest build of the series at build time, or a mirror of our own of the FFmpeg archive; a code-signing certificate.
