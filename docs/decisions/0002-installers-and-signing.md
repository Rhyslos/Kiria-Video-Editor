# 0002 · Installers, FFmpeg bundling and signing

**Date:** 10 November 2026

**Decision:**
1. The Windows installer is built with Inno Setup (a per-user `Setup.exe`) and the Linux build is an AppImage, both from our own scripts in `packaging/`, instead of a generated MSI.
2. Releases bundle the newest FFmpeg 8.1 GPL shared build at release time; the exact build and its checksum are written into `licenses/FFMPEG-VERSION.txt` of every release.
3. No paid Windows code-signing certificate. Updates are protected by our own ed25519 signature instead.

**Why:**
1. The install layout (`versions/<version>/` with the launcher outside) is what makes side-by-side updates and rollback possible in Phase 3; generated installers assume one flat folder. Inno Setup is free, ships on GitHub's Windows runners, needs no administrator rights for a per-user install, and installs silently for the automated test.
2. Pinning one exact build needs a mirror of the archive; the upstream build page only keeps recent builds. A series pin plus a recorded checksum is enough for a hobby project and keeps security fixes flowing in.
3. A certificate costs money every year and only removes the "unknown publisher" warning on first install.

**Instead of:** an MSI from a release-tooling generator; an exactly pinned FFmpeg archive; a code-signing certificate.
