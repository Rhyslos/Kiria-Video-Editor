#!/usr/bin/env bash
# Makes the synthetic test clips for the highlights spike: three recordings
# with events planted at known times, each with its ground truth beside it.
# Usage: make-clips.sh <output folder> [seconds]   (default 180 s of 1440p 60 H.265;
# H264=1 for 1440p H.264, several times quicker to make; SMALL=1 for 720p H.264)
#
# The plans live in src/synth.rs, so Linux and Windows make the same clips.
# Set FFMPEG to the program if it isn't on the PATH.
set -euo pipefail
out=$(realpath -m "${1:?output folder}")
secs=${2:-180}
args=(make-clips "$out" --seconds "$secs" --ffmpeg "${FFMPEG:-ffmpeg}")
if [ "${SMALL:-0}" = 1 ]; then args+=(--small); fi
if [ "${H264:-0}" = 1 ]; then args+=(--h264); fi
cd "$(dirname "$0")"
cargo run --release -- "${args[@]}"
ls -la "$out"
