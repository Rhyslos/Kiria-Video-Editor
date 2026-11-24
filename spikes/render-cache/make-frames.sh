#!/usr/bin/env bash
# Makes raw RGBA frames like a rendered timeline: a game-like moving
# pattern with grain, a facecam box and a title over it.
# Usage: make-frames.sh <out.rgba> [frames] [size]
set -euo pipefail
out=${1:?out.rgba}; n=${2:-30}; size=${3:-2560x1440}
ffmpeg=${FFMPEG:-ffmpeg}
"$ffmpeg" -hide_banner -loglevel error -y \
  -f lavfi -i "testsrc2=size=$size:rate=60,noise=alls=18:allf=t+u" \
  -f lavfi -i "mandelbrot=size=480x270:rate=60" \
  -filter_complex "[0][1]overlay=W-w-40:H-h-40,drawbox=x=40:y=40:w=900:h=120:color=black@0.6:t=fill,format=rgba" \
  -frames:v "$n" -f rawvideo "$out"
