#!/usr/bin/env bash
# Makes the synthetic test clips for the caching spike.
# Usage: make-clips.sh <output folder> [seconds]   (default 20 s; a 720p clip too with SMALL=1)
#
# The clips resemble game recordings: moving test pattern plus film-grain
# noise at GPU-recorder bit rates, P frames only (GPU recorders rarely write
# B frames), key frames 2 s apart, and one clip with key frames 8 s apart
# like the recordings that jump slowly. The AV1 encoder keeps its own layered
# frame structure. Set FFMPEG to the program if it isn't on the PATH.
set -euo pipefail
out=${1:?output folder}
secs=${2:-20}
ffmpeg=${FFMPEG:-ffmpeg}
mkdir -p "$out"

src() { # size rate
  echo "testsrc2=size=$1:rate=$2,noise=alls=18:allf=t+u,format=yuv420p"
}

enc() { # name size rate key-frame-distance-in-frames codec-args...
  local name=$1 size=$2 rate=$3 keys=$4; shift 4
  [ -f "$out/$name" ] && { echo "have $name"; return; }
  echo "making $name"
  "$ffmpeg" -hide_banner -loglevel error -f lavfi -i "$(src "$size" "$rate")" -t "$secs" \
    -g "$keys" "$@" "$out/$name"
}

x265="log-level=error:bframes=0:scenecut=0"
enc h264-1080p60-2s.mp4 1920x1080 60 120 -c:v libx264 -preset veryfast -bf 0 -sc_threshold 0 -b:v 20M -maxrate 20M -bufsize 40M
enc h265-1440p60-2s.mp4 2560x1440 60 120 -c:v libx265 -preset ultrafast -b:v 40M -x265-params "$x265" -tag:v hvc1
enc h265-1440p60-8s.mp4 2560x1440 60 480 -c:v libx265 -preset ultrafast -b:v 40M -x265-params "$x265" -tag:v hvc1
encoders=$("$ffmpeg" -hide_banner -encoders 2>/dev/null || true)
if [[ $encoders == *libsvtav1* ]]; then
  enc av1-1440p60-2s.mkv 2560x1440 60 120 -c:v libsvtav1 -preset 12 -b:v 30M
else
  echo "no AV1 encoder in this FFmpeg build: skipping the AV1 clip"
fi
if [ "${SMALL:-0}" = 1 ]; then
  # For slow machines: decodes several times faster than it plays even in
  # software on two threads, as 1440p does on a desktop GPU.
  enc h264-720p60-2s.mp4 1280x720 60 120 -c:v libx264 -preset veryfast -bf 0 -sc_threshold 0 -b:v 8M -maxrate 8M -bufsize 16M
fi
ls -la "$out"
