#!/usr/bin/env bash
# Times keeping the same frames as video: encode, size and decode.
# Usage: video.sh <frames.rgba> <width> <height> <work folder>
set -euo pipefail
in=$1; w=$2; h=$3; work=$4
ffmpeg=${FFMPEG:-ffmpeg}
mkdir -p "$work"
n=$(( $(stat -c %s "$in") / (w * h * 4) ))
run() { # name ext args...
  local name=$1 ext=$2; shift 2
  local f="$work/$name.$ext"
  local t0=$(date +%s.%N)
  "$ffmpeg" -hide_banner -loglevel error -y -f rawvideo -pix_fmt rgba -s "${w}x${h}" -r 60 -i "$in" "$@" "$f"
  local t1=$(date +%s.%N)
  sync
  "$ffmpeg" -hide_banner -loglevel error -threads 1 -i "$f" -f rawvideo -pix_fmt yuv420p -y /dev/null
  local t2=$(date +%s.%N)
  local mb=$(echo "$(stat -c %s "$f") / $n / 1000000" | bc -l)
  printf "%-28s %8.2f MB/frame  encode %7.1f ms/frame  decode %6.1f ms/frame (1 thread)\n" "$name" "$mb" \
    "$(echo "($t1-$t0)*1000/$n" | bc -l)" "$(echo "($t2-$t1)*1000/$n" | bc -l)"
  rm -f "$f"
}
run "H.264 intra crf 12 veryfast" mkv -c:v libx264 -preset veryfast -crf 12 -g 1 -pix_fmt yuv420p
run "H.264 crf 14 key every 8" mkv -c:v libx264 -preset veryfast -crf 14 -g 8 -bf 0 -pix_fmt yuv420p
run "JPEG per frame q 3" mkv -c:v mjpeg -q:v 3 -pix_fmt yuvj420p
run "FFV1 (lossless 4:2:0)" mkv -c:v ffv1 -level 3 -slices 4 -pix_fmt yuv420p
