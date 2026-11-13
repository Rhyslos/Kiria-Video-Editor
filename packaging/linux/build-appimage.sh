#!/usr/bin/env bash
# Builds <name>-<version>-linux-x86_64.AppImage with the launcher, the editor,
# the FFmpeg libraries, all licences and the user guide.
#
# Run from the repository root after `cargo build --release -p app -p launcher`.
# Needs APP_NAME, APP_ID and APP_VERSION in the environment (`cargo xtask brand`).
set -euo pipefail

: "${APP_NAME:?run cargo xtask brand first}" "${APP_ID:?}" "${APP_VERSION:?}"
OUT="${OUT:-target/dist}"
APPDIR=target/AppDir
WORK=target/appimage-work
rm -rf "$APPDIR" "$WORK"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/licenses" "$APPDIR/usr/share/user-guide/images" "$WORK" "$OUT"

cp target/release/launcher target/release/editor "$APPDIR/usr/bin/"
install -m 755 packaging/linux/AppRun "$APPDIR/AppRun"

# FFmpeg libraries and licence.
source packaging/ffmpeg.env
curl -fsSL -o "$WORK/ffmpeg.tar.xz" "$FFMPEG_LINUX_URL"
tar -xJf "$WORK/ffmpeg.tar.xz" -C "$WORK"
FF=$(find "$WORK" -maxdepth 1 -type d -name 'ffmpeg-*' | head -n 1)
cp -a "$FF"/lib/*.so* "$APPDIR/usr/lib/"
cp "$FF/LICENSE.txt" "$APPDIR/usr/share/licenses/FFmpeg-LICENSE.txt"
{
  LD_LIBRARY_PATH="$FF/lib" "$FF/bin/ffmpeg" -hide_banner -version | head -n 1 || echo "FFmpeg $FFMPEG_SERIES"
  echo "SHA-256 of the downloaded archive: $(sha256sum "$WORK/ffmpeg.tar.xz" | cut -d' ' -f1)"
} > "$APPDIR/usr/share/licenses/FFMPEG-VERSION.txt"

# The user guide, where the programs look for it (usr/bin/../share/user-guide).
cp docs/user-guide/index.html "$APPDIR/usr/share/user-guide/"
cp docs/user-guide/images/* "$APPDIR/usr/share/user-guide/images/"

cp LICENSE "$APPDIR/usr/share/licenses/LICENSE.txt"
cp crates/ui-kit/assets/fonts/*-OFL.txt "$APPDIR/usr/share/licenses/"

# Desktop entry and icon.
cp packaging/icons/icon.png "$APPDIR/$APP_ID.png"
cp packaging/icons/icon.png "$APPDIR/.DirIcon"
cat > "$APPDIR/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=$APP_NAME
Comment=Video editor
Exec=launcher %f
Icon=$APP_ID
Categories=AudioVideo;Video;AudioVideoEditing;
Terminal=false
EOF

# appimagetool turns the folder into one runnable file.
curl -fsSL -o "$WORK/appimagetool" \
  https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "$WORK/appimagetool"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$WORK/appimagetool" --no-appstream "$APPDIR" \
  "$OUT/$APP_NAME-$APP_VERSION-linux-x86_64.AppImage"
echo "Wrote $OUT/$APP_NAME-$APP_VERSION-linux-x86_64.AppImage"
