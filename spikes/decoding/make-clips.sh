#!/usr/bin/env bash
# Makes the synthetic test clips for the decoding spikes.
# Usage: make-clips.sh <output folder> [seconds]   (default 20 s; the 1-hour clip only with LONG=1)
#
# Moving test pattern plus film-grain noise, so the files are about as hard to
# decode as real game footage at GPU-recorder bit rates. Key frame every 2 s.
set -euo pipefail
out=${1:?output folder}
secs=${2:-20}
mkdir -p "$out"

src() { # size rate
  echo "testsrc2=size=$1:rate=$2,noise=alls=18:allf=t+u,format=yuv420p"
}

enc() { # name size rate codec-args...
  local name=$1 size=$2 rate=$3; shift 3
  [ -f "$out/$name" ] && { echo "have $name"; return; }
  echo "making $name"
  ffmpeg -hide_banner -loglevel error -f lavfi -i "$(src "$size" "$rate")" -t "$secs" \
    -g $((rate * 2)) "$@" "$out/$name"
}

enc h264-1080p60.mp4  1920x1080 60  -c:v libx264 -preset veryfast -b:v 20M -maxrate 20M -bufsize 40M
enc h265-1440p60.mp4  2560x1440 60  -c:v libx265 -preset ultrafast -b:v 40M -x265-params log-level=error -tag:v hvc1
enc h265-1440p144.mkv 2560x1440 144 -c:v libx265 -preset ultrafast -b:v 60M -x265-params log-level=error
enc av1-1440p60.mkv   2560x1440 60  -c:v libsvtav1 -preset 12 -b:v 30M

if [ "${LONG:-0}" = 1 ]; then
  # Variable frame rate: drop frames at random so timestamps are uneven.
  name=h264-1440p60-vfr-1h.mkv
  [ -f "$out/$name" ] || ffmpeg -hide_banner -loglevel error -f lavfi -i "$(src 2560x1440 60),select='gt(random(0),0.1)'" \
    -t 3600 -fps_mode vfr -g 120 -c:v libx264 -preset ultrafast -b:v 30M "$out/$name"
fi
ls -la "$out"
