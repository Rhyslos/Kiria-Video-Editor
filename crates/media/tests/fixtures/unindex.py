#!/usr/bin/env python3
"""Makes a Matroska file look like a recording cut short by a crash: no
seek index (Cues) and no Duration, for the fixtures.

Usage: ffmpeg -i <in.mkv> -c copy -f matroska - > <out.mkv>
       unindex.py <out.mkv>

Written to a pipe, FFmpeg's Matroska muxer can't go back, so it writes
no Cues and leaves its clusters' sizes unknown, as a recorder's file is
until it is finished. It still writes the Duration it was handed, so
this turns that element into a Void element of the same length and
nothing else in the file moves.
"""

import sys


def main(path):
    data = bytearray(open(path, "rb").read())
    if data.find(b"\x1c\x53\xbb\x6b") >= 0:
        sys.exit(f"{path} has Cues: write it to a pipe")
    at = data.find(b"\x44\x89\x88")
    if at < 0 or data.find(b"\x44\x89\x88", at + 1) >= 0:
        sys.exit(f"{path}: expected one eight-byte Duration element")
    # 0x4489, size 8, 8 bytes = 11 bytes; Void (0xEC), size 9, 9 bytes = 11.
    data[at : at + 11] = b"\xec\x89" + bytes(9)
    open(path, "wb").write(bytes(data))


if __name__ == "__main__":
    main(sys.argv[1])
