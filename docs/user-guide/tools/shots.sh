#!/usr/bin/env bash
# Takes the user guide's screenshots from the real programs, without a
# screen, and writes them as JPEG files into docs/user-guide/images/.
#
#   docs/user-guide/tools/shots.sh            every picture
#   docs/user-guide/tools/shots.sh audio      only those whose name has "audio"
#
# Needs (Linux): the editor and launcher built (`cargo build -p app -p
# launcher`), xvfb-run, FFmpeg's command-line program (`cargo xtask ffmpeg`
# puts one in .ffmpeg/sdk/bin) and python3 with Pillow. Without a graphics
# card, point VK_DRIVER_FILES at a software Vulkan driver (lavapipe).
#
# Everything it makes goes in $WORK (target/user-guide-shots unless set):
# made-up recordings, a private home folder (so your own settings and
# projects are never touched), the demo project and the full-size PNGs.
# Delete $WORK to start again from nothing.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
GUIDE=$(dirname "$HERE")
ROOT=$(cd "$GUIDE/../.." && pwd)
WORK=${WORK:-$ROOT/target/user-guide-shots}
EDITOR_BIN=${EDITOR_BIN:-$ROOT/target/debug/editor}
LAUNCHER_BIN=${LAUNCHER_BIN:-$ROOT/target/debug/launcher}
FFMPEG=${FFMPEG:-$ROOT/.ffmpeg/sdk/bin/ffmpeg}
FILTER=${1:-}
SIZE=1440x900
QUALITY=${QUALITY:-80}

MEDIA=$WORK/media
HOMEDIR=$WORK/home
PNG=$WORK/png
PROJECTS=$WORK/projects
mkdir -p "$MEDIA/Recordings" "$MEDIA/Music" "$HOMEDIR/data" "$PNG" "$PROJECTS" "$GUIDE/images"
# The demo helper is imported by a snippet below; keep its cache out of the tree.
export PYTHONDONTWRITEBYTECODE=1
export LD_LIBRARY_PATH="$(dirname "$(dirname "$FFMPEG")")/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# Runs a program with the private home folder.
run() {
  HOME=$HOMEDIR XDG_DATA_HOME=$HOMEDIR/data XDG_CACHE_HOME=$HOMEDIR/cache XDG_CONFIG_HOME=$HOMEDIR/config "$@"
}

# The app's data folder in the private home (named by the app).
data_dir() { find "$HOMEDIR/data" -mindepth 1 -maxdepth 1 -type d ! -name mesa_shader_cache | head -n 1; }

# --- Made-up recordings: a match with game, mic and voice chat, a facecam,
# --- a second clip, a separately recorded mic, music and a picture.
make_media() {
  local ff="$FFMPEG -hide_banner -loglevel error -y"
  local rec=$MEDIA/Recordings
  [ -f "$rec/Ranked match 2026-11-08 21-14.mkv" ] && return
  echo "Making the recordings…"
  local speech="volume='if(lt(mod(t\\,5)\\,3.2)\\,0.9*(0.6+0.4*sin(2*PI*3*t))\\,0.01)':eval=frame"
  # The game stream hears the room too (a little of the mic), as game captures do:
  # Sync by sound lines the separate mic up by it.
  $ff -f lavfi -i "mandelbrot=s=1280x720:r=30,trim=duration=40,format=yuv420p" \
    -filter_complex "anoisesrc=color=pink:d=40:a=0.5:r=48000:seed=7,$speech,asplit=2[mic][room];
      sine=f=110:d=40:sample_rate=48000,volume=0.3[tone];[room]volume=0.3[r];[tone][r]amix=inputs=2:normalize=0[game];
      anoisesrc=color=brown:d=40:a=0.3:r=48000,volume='if(lt(mod(t+2\\,7)\\,2)\\,1\\,0.005)':eval=frame[chat]" \
    -map 0:v -map "[game]" -map "[mic]" -map "[chat]" -c:v libx264 -preset veryfast -tune fastdecode -crf 33 -g 60 \
    -c:a aac -ac 2 -b:a 160k \
    -metadata:s:a:0 title=Game -metadata:s:a:1 title=Mic -metadata:s:a:2 "title=Voice chat" \
    "$rec/Ranked match 2026-11-08 21-14.mkv"
  $ff -f lavfi -i "testsrc2=s=1280x720:r=30:d=25" -f lavfi -i "sine=f=330:d=25:sample_rate=48000,volume=0.4" \
    -c:v libx264 -preset veryfast -tune fastdecode -crf 30 -g 60 -c:a aac -ac 2 -shortest "$rec/Clutch round 2026-11-09 19-02.mp4"
  $ff -f lavfi -i "cellauto=s=1280x720:r=30:rule=110,trim=duration=20,format=yuv420p" \
    -f lavfi -i "sine=f=220:d=20:sample_rate=48000,volume=0.3" \
    -c:v libx264 -preset veryfast -tune fastdecode -crf 30 -g 60 -c:a aac -ac 2 -shortest "$rec/Boss fight attempt 7.mp4"
  $ff -f lavfi -i "gradients=s=640x360:r=30:d=40:speed=0.02" -c:v libx264 -preset veryfast -crf 24 -an \
    "$rec/Facecam 2026-11-08 21-14.mp4"
  # The mic recorded on its own: the match's mic stream with hiss, starting 3 s later.
  $ff -i "$rec/Ranked match 2026-11-08 21-14.mkv" -f lavfi -i "anoisesrc=color=white:d=40:a=0.02" \
    -filter_complex "[0:a:1]pan=mono|c0=c0,volume=1.4[m];[m][1]amix=inputs=2:normalize=0,atrim=start=3" \
    -ar 48000 -c:a pcm_s16le "$rec/Mic 2026-11-08 21-14.wav"
  $ff -f lavfi -i "sine=f=220:d=30" -f lavfi -i "sine=f=277:d=30" -f lavfi -i "sine=f=330:d=30" \
    -filter_complex "[0][1][2]amix=inputs=3,volume='0.8+0.2*sin(2*PI*0.5*t)':eval=frame" \
    -ar 44100 -c:a libmp3lame -b:a 192k "$MEDIA/Music/Intro music.mp3"
  $ff -f lavfi -i "smptehdbars=s=1280x720:d=1" -frames:v 1 "$rec/Thumbnail idea.png"
}

# Settings as a new install has them, but with more room for the timeline,
# and proxies only where a picture is about them (PROXIES=Auto): building
# them would slow every other screenshot down. Shots taken while playing
# say BACKGROUND=Always, so waveforms are read meanwhile. Written before
# every run, since some runs change settings.
fresh_settings() {
  local data
  data=$(data_dir)
  [ -n "$data" ] && printf '(viewer_split: 0.42, proxies: %s, resources: (worker_threads: Auto, ram_cache_mb: Auto, disk_cache_gb: Auto, background: %s))\n' \
    "${PROXIES:-Off}" "${BACKGROUND:-PauseWhilePlaying}" > "$data/settings.ron"
  return 0
}

DEMO="$PROJECTS/demo/Ranked match edit.kiria"

# --- The demo project: the recordings imported, then dressed.
make_project() {
  [ -f "$DEMO" ] && return
  echo "Making the demo project…"
  mkdir -p "$(dirname "$DEMO")"
  local rec=$MEDIA/Recordings
  run timeout 180 xvfb-run -a -s "-screen 0 ${SIZE}x24" "$EDITOR_BIN" "$DEMO" \
    --import "$rec/Ranked match 2026-11-08 21-14.mkv" --import "$rec/Clutch round 2026-11-09 19-02.mp4" \
    --import "$rec/Facecam 2026-11-08 21-14.mp4" --import "$MEDIA/Music/Intro music.mp3" \
    --import "$rec/Thumbnail idea.png" --import "$rec/Boss fight attempt 7.mp4" \
    --screenshot "$PNG/_import.png" >/dev/null 2>&1
  python3 "$HERE/demo_project.py" "$DEMO" "$(data_dir)"
}

# A copy of the demo project in a folder of its own, for shots that change it.
copy_of() {
  local dir=$PROJECTS/$1
  rm -rf "${dir:?}"
  mkdir -p "$dir"
  cp "$DEMO" "$dir/"
  cp "$DEMO.thumbnail.png" "$dir/" 2>/dev/null || true
  echo "$dir/$(basename "$DEMO")"
}

# The region from $2 to $3 seconds in a project file.
with_region() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys
path, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path, encoding="utf-8").read()
text = text.replace("        region: None,", f"        region: Some((\n          start: ({a}, 1),\n          end: ({b}, 1),\n        )),", 1)
open(path, "w", encoding="utf-8").write(text)
PY
}

wanted() { [[ -z "$FILTER" || "$1" == *"$FILTER"* ]]; }

# The PNG as a JPEG in the guide.
keep() {
  python3 - "$PNG/$1.png" "$GUIDE/images/$1.jpg" "$QUALITY" <<'PY'
import sys
from PIL import Image
src, dst, quality = sys.argv[1], sys.argv[2], int(sys.argv[3])
Image.open(src).convert("RGB").save(dst, "JPEG", quality=quality, optimize=True, progressive=True)
PY
}

# editor_shot NAME PROJECT [options…]: the editor on PROJECT ("" for none).
editor_shot() {
  local name=$1 project=$2
  shift 2
  wanted "$name" || return 0
  fresh_settings
  local args=()
  [ -n "$project" ] && args+=("$project")
  echo "  $name"
  run timeout 120 xvfb-run -a -s "-screen 0 ${SIZE}x24" "$EDITOR_BIN" "${args[@]}" --window-size "$SIZE" "$@" \
    --screenshot "$PNG/$name.png" >"$PNG/$name.log" 2>&1 || echo "  $name failed: see $PNG/$name.log"
  [ -f "$PNG/$name.png" ] && keep "$name"
}

# launcher_shot NAME [options…]
launcher_shot() {
  local name=$1
  shift
  wanted "$name" || return 0
  echo "  $name"
  run timeout 60 xvfb-run -a -s "-screen 0 ${SIZE}x24" "$LAUNCHER_BIN" --no-update-check --window-size "$SIZE" "$@" \
    --screenshot "$PNG/$name.png" >"$PNG/$name.log" 2>&1 || echo "  $name failed: see $PNG/$name.log"
  [ -f "$PNG/$name.png" ] && keep "$name"
}

make_media
make_project
echo "Taking the screenshots…"

# Getting started.
launcher_shot launcher --demo 14
launcher_shot launcher-update --demo 14 --update-window available
launcher_shot launcher-crash --demo 8 --demo-crash
launcher_shot launcher-settings --settings help

# The editor at a glance.
editor_shot editor-overview "$DEMO" --playhead 27.5
editor_shot search-palette "$DEMO" --playhead 27.5 --action Search

# Library.
PROXIES=Auto editor_shot library "$DEMO" --page library
PROXIES=Auto editor_shot library-detail "$DEMO" --page library --library-select "Clutch round 2026-11-09 19-02.mp4"
editor_shot library-audio "$DEMO" --page library --library-tab audio --library-select "Intro music.mp3"
editor_shot prune "$DEMO" --action Prune
if wanted package; then
  mkdir -p "$WORK/packages"
  editor_shot package "$DEMO" --package-to "$WORK/packages"
fi
if wanted media-drawer; then
  # A recording that appears in a watched folder after the project was saved.
  mkdir -p "$WORK/watched"
  cp "$MEDIA/Recordings/Boss fight attempt 7.mp4" "$WORK/watched/Boss fight attempt 8.mp4"
  touch "$WORK/watched/Boss fight attempt 8.mp4"
  PROXIES=Auto editor_shot media-drawer "$DEMO" --media-drawer --watch-folder "$WORK/watched"
fi

# Edit page.
editor_shot track-menu "$DEMO" --playhead 27.5 --pointer 60,660 --right-click
if wanted region-menu; then
  region=$(copy_of region)
  with_region "$region" 30 45
  editor_shot region-menu "$region" --playhead 36 --pointer 720,590 --right-click
fi
if wanted timelines; then
  nested=$(copy_of nested)
  editor_shot timelines "$nested" --playhead 6 --select-first-clip --action NestSelection
fi
editor_shot templates "$DEMO" --playhead 27.5 --action Templates
editor_shot inspector "$DEMO" --playhead 5 --select-first-clip --action PictureHandles
editor_shot keyframes "$DEMO" --playhead 1 --select-first-clip
editor_shot speed "$DEMO" --playhead 5 --select-first-clip --pointer 1144,140 --scroll 520
editor_shot display-menu "$DEMO" --playhead 27.5 --pointer 882,414 --click
editor_shot split-view "$DEMO" --playhead 8 --select-first-clip --action SplitView
editor_shot remove-silence "$DEMO" --playhead 27.5 --action RemoveSilence
if wanted sync; then
  sync="$PROJECTS/sync/Voice over sync.kiria"
  rm -rf "${PROJECTS:?}/sync"
  mkdir -p "$(dirname "$sync")"
  editor_shot sync "$sync" --import "$MEDIA/Recordings/Ranked match 2026-11-08 21-14.mkv" \
    --import "$MEDIA/Recordings/Mic 2026-11-08 21-14.wav" --sync-by-sound
fi

# Audio page.
BACKGROUND=Always editor_shot audio-eq "$DEMO" --page audio --audio-tool eq --audio-track 2 --playhead 4 --play
BACKGROUND=Always editor_shot audio-compressor "$DEMO" --page audio --audio-tool compressor --audio-track 2 --playhead 4 --play
if wanted audio-cleanup; then
  # Twelve tracks made from the first one's clips, the first with its noise
  # learned (the editor saves them into the project, so on a copy).
  many=$(copy_of many)
  editor_shot audio-cleanup "$many" --page audio --audio-tool clean-up --audio-tracks 12 --audio-track 1 --playhead 4
fi
BACKGROUND=Always editor_shot audio-meters "$DEMO" --page audio --audio-tool meters --audio-track 2 --playhead 4 --play
BACKGROUND=Always editor_shot audio-mixer "$DEMO" --page audio --audio-tool ducking --audio-track 4 --audio-view mixer --playhead 4 --play
editor_shot audio-lines "$DEMO" --page audio --audio-tool loudness --audio-track 4 --playhead 27.5 --action VolumeLines
if wanted audio-clarity; then
  clarity=$(copy_of clarity)
  with_region "$clarity" 4 24
  # Loud music that doesn't duck: something for the check to find.
  python3 - "$HERE" "$clarity" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
from demo_project import Project, track_blocks
p = Project(sys.argv[2])
p.track_sound("Music", "Audio", {("ducking", "on"): "false"})
start, end, _ = next(t for t in track_blocks(p.lines, "Audio") if t[2] == "Music")
for i in range(start, end):
    if p.lines[i] == " " * 12 + "gain_db: 0.0,":
        p.lines[i] = " " * 12 + "gain_db: 9.0,"
p.save()
PY
  editor_shot audio-clarity "$clarity" --page audio --audio-tool eq --audio-track 2 --playhead 4 --audio-clarity-region --screenshot-wait 120
fi

# Export page.
editor_shot export "$DEMO" --page export
if wanted export-queue; then
  queue=$(copy_of queue)
  editor_shot export-queue "$queue" --export-queue
fi

# Settings and the shortcuts page.
editor_shot settings-editing "$DEMO" --settings editing
editor_shot shortcuts-page "" --shortcuts-page filter
editor_shot shortcuts-clash "" --shortcuts-page clash

# Saving.
editor_shot versions "$DEMO" --playhead 27.5 --action Versions

# Diagnostics and troubleshooting.
editor_shot diagnostics "$DEMO" --settings diagnostics --pointer 976,268 --click
editor_shot graphics-warning "" --graphics-warning

du -sh "$GUIDE/images"
