#!/usr/bin/env python3
"""Writes a Matroska file's container crop (PixelCropBottom / PixelCropRight)
in place of its display size, for the cropping fixtures.

Usage: crop.py <file.mkv> <crop right> <crop bottom>

FFmpeg's Matroska muxer writes a display width and height when the file's
display aspect differs from its pixels (`-aspect`) and, with
`-write_crc32 0`, no checksums over them. Each is a one-byte number after
a two-byte ID, the same length as a crop element, so the two are written
over in place and nothing else in the file moves: DisplayWidth (0x54B0)
becomes PixelCropRight (0x54DD), DisplayHeight (0x54BA) PixelCropBottom
(0x54AA), as a screen recorder writes them for frames its encoder padded.
"""

import sys


def main(path, right, bottom):
    data = bytearray(open(path, "rb").read())
    for old, new, value in ((b"\x54\xb0", b"\x54\xdd", right), (b"\x54\xba", b"\x54\xaa", bottom)):
        at = data.find(old + b"\x81")
        if at < 0 or data.find(old + b"\x81", at + 1) >= 0:
            sys.exit(f"{path}: expected one one-byte {old.hex()} element")
        if not 0 <= value < 256:
            sys.exit("crops over 255 pixels don't fit")
        data[at : at + 4] = new + b"\x81" + bytes([value])
    open(path, "wb").write(bytes(data))


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]))
