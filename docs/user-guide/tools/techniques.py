#!/usr/bin/env python3
"""Writes the small projects the user guide's Editing techniques pictures
are taken of, from the recordings a project of the editor's already has
(the demo's, just after importing).

Usage: techniques.py <a project file the editor wrote> <folder to write into>

Each project is written whole in the editor's own text format (RON, its
compact form), at the format this tool knows (47): the media list is
copied from the given project, so its recordings' fingerprints are the
real ones, and the timelines are laid out here. The editor checks them
when the pictures open them.

- `cuts.kiria`: three shots with an L cut (the first shot's sound goes on
  two seconds into the second) and a J cut (the third shot's sound starts
  a second and a half before its picture).
- `crossfades.kiria`: two shots dissolving into each other, their game
  sound crossfading, and the music crossfading smoothly into a later part
  of itself.
"""

import os
import re
import sys

FORMAT = 47


def media_map(text):
    """The project's `media:{...}` map as written, and file name -> id."""
    start = text.index("media:{")
    depth = 0
    for i in range(start + len("media:"), len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                block = text[start : i + 1]
                break
    ids = {os.path.basename(p): int(i) for i, p in re.findall(r'(\d+):\(id:\1,path:"([^"]+)"', block)}
    return block, ids


def t(seconds):
    """A time as the file writes it: a fraction, here in thousandths."""
    return f"({round(seconds * 1000)},1000)"


class Timeline:
    def __init__(self):
        self.next_id = 100
        self.tracks = []

    def new_id(self):
        self.next_id += 1
        return self.next_id

    def track(self, kind, name, clips):
        self.tracks.append((self.new_id(), kind, name, clips))

    def clip(self, media, start, length, source_in, stream=0, transition=None):
        fields = [
            f"id:{self.new_id()}",
            f"media:{media}",
            *([f"stream:{stream}"] if stream else []),
            f"start:{t(start)}",
            f"source_in:{t(source_in)}",
            f"duration:{t(length)}",
        ]
        if transition:
            kind, seconds = transition
            fields.append(f"transition:Some((kind:{kind},length:{t(seconds)}))")
        return "(" + ",".join(fields) + ")"

    def write(self, path, name, media):
        def tracks(kind):
            return ",".join(
                f'(id:{i},kind:{k},name:"{n}",clips:[{",".join(c)}])' for i, k, n, c in self.tracks if k == kind
            )

        seq_id = self.new_id()
        project = (
            f'(name:"{name}",media:{media[len("media:"):]},sequences:[(id:{seq_id},name:"Main sequence",'
            f"settings:(width:1280,height:720,frame_rate:(30,1),sample_rate:48000),"
            f"video_tracks:[{tracks('Video')}],audio_tracks:[{tracks('Audio')}],edit_needle:(0,1))],"
            f"active_sequence:{seq_id},next_id:{self.new_id() + 1})"
        )
        with open(path, "w", encoding="utf-8") as f:
            f.write(f"(\n  format_version: {FORMAT},\n  project: {project},\n)\n")


def main(source, out):
    with open(source, encoding="utf-8") as f:
        block, ids = media_map(f.read())

    def media(prefix):
        return next(i for name, i in ids.items() if name.startswith(prefix))

    match, clutch, boss, music = media("Ranked match"), media("Clutch round"), media("Boss fight"), media("Intro music")
    os.makedirs(out, exist_ok=True)

    cuts = Timeline()
    cuts.track(
        "Video",
        "V1",
        [cuts.clip(match, 0, 10, 5), cuts.clip(clutch, 10, 10, 2), cuts.clip(boss, 20, 10, 3)],
    )
    cuts.track(
        "Audio",
        "Game",
        [
            # L cut: the match's sound goes on under the next shot.
            cuts.clip(match, 0, 12, 5, stream=1),
            cuts.clip(clutch, 12, 6.5, 4, stream=1),
            # J cut: the boss fight is heard before it is seen.
            cuts.clip(boss, 18.5, 11.5, 1.5, stream=1),
        ],
    )
    cuts.track("Audio", "Mic", [cuts.clip(match, 0, 12, 5, stream=2)])
    cuts.write(os.path.join(out, "cuts.kiria"), "Editing techniques: cuts", block)

    fades = Timeline()
    fades.track(
        "Video",
        "V1",
        [fades.clip(boss, 0, 8, 0), fades.clip(clutch, 8, 8, 0, transition=("Dissolve", 1))],
    )
    fades.track(
        "Audio",
        "Game",
        [fades.clip(boss, 0, 8, 0, stream=1), fades.clip(clutch, 8, 8, 0, stream=1, transition=("Crossfade", 1))],
    )
    fades.track(
        "Audio",
        "Music",
        [fades.clip(music, 0, 8, 0), fades.clip(music, 8, 8, 14, transition=("CrossfadeSmooth", 2))],
    )
    fades.write(os.path.join(out, "crossfades.kiria"), "Editing techniques: crossfades", block)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
