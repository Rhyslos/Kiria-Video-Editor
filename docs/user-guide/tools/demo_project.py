#!/usr/bin/env python3
"""Dresses a project the editor made from imports into the user guide's
demo project: labels, a list and a note on files, timeline labels, the
facecam on a track of its own sliding into a corner, the music on its own
track, track roles and a few effects, and two files left out of the timeline
(for Prune).

It also writes two templates into the app's data folder and two saved
versions beside the project, for the Templates and Saved versions windows.

Usage: demo_project.py <project file> <the app's data folder>

The project file is the editor's own text format (RON). This edits it as
text, so it only knows the shape the editor writes (format 17); the
editor's own loading checks the result when the screenshots open it. The
autosave journal beside the project is removed, so the editor opens the
file as written here.
"""

import os
import re
import sys

ROLE = {"Game": "Effects", "Mic": "Dialogue", "Voice chat": "Dialogue", "Music": "Music"}


def block_end(lines, start):
    """The index of the line closing the block opened at `start`."""
    indent = len(lines[start]) - len(lines[start].lstrip())
    for i in range(start + 1, len(lines)):
        line = lines[i]
        if len(line) - len(line.lstrip()) == indent and line.strip().startswith(")"):
            return i
    raise ValueError(f"no end for the block at line {start + 1}")


def media_ids(text):
    """File name -> media id."""
    ids = {}
    for m in re.finditer(r'\n        id: (\d+),\n        path: "([^"]+)"', text):
        ids[os.path.basename(m.group(2))] = int(m.group(1))
    return ids


def clip_blocks(lines):
    """(start, end, media id) of every clip, in the order they are written."""
    found = []
    i = 0
    while i < len(lines):
        if lines[i] == " " * 14 + "(" and i + 2 < len(lines) and lines[i + 2].startswith(" " * 16 + "media: "):
            end = block_end(lines, i)
            media = int(lines[i + 2].split(":")[1].strip().rstrip(","))
            found.append((i, end, media))
            i = end + 1
        else:
            i += 1
    return found


def track_blocks(lines, kind):
    """(start, end, name) of every track of `kind` ("Video" or "Audio")."""
    found = []
    for i, line in enumerate(lines):
        if line == " " * 10 + "(" and i + 2 < len(lines) and lines[i + 2] == " " * 12 + f"kind: {kind},":
            end = block_end(lines, i)
            name = lines[i + 3].split('"')[1]
            found.append((i, end, name))
    return found


class Project:
    def __init__(self, path):
        self.path = path
        with open(path, encoding="utf-8") as f:
            self.lines = f.read().split("\n")
        self.next_id = int(re.search(r"\n    next_id: (\d+),", self.text()).group(1))

    def text(self):
        return "\n".join(self.lines)

    def new_id(self):
        self.next_id += 1
        return self.next_id - 1

    def take_clip(self, media):
        """Removes the first clip playing `media` and returns its lines."""
        for start, end, m in clip_blocks(self.lines):
            if m == media:
                taken = self.lines[start : end + 1]
                del self.lines[start : end + 1]
                return taken
        raise ValueError(f"no clip plays media {media}")

    def drop_clips(self, media):
        while any(m == media for _, _, m in clip_blocks(self.lines)):
            self.take_clip(media)

    def add_track(self, kind, name, clip, after):
        """A new track of `kind` named `name` holding `clip` (lines), made
        from the track named `after`, put after it."""
        start, end, _ = next(t for t in track_blocks(self.lines, kind) if t[2] == after)
        track = list(self.lines[start : end + 1])
        track[1] = " " * 12 + f"id: {self.new_id()},"
        track[3] = " " * 12 + f'name: "{name}",'
        clips_at = track.index(" " * 12 + "clips: [")
        clips_end = block_end_bracket(track, clips_at)
        track[clips_at + 1 : clips_end] = clip
        self.lines[end + 1 : end + 1] = track

    def set_in_clip(self, media, field, value):
        for start, end, m in clip_blocks(self.lines):
            if m == media:
                for i in range(start, end):
                    if self.lines[i].startswith(" " * 16 + f"{field}:") or self.lines[i].startswith(" " * 18 + f"{field}:"):
                        indent = len(self.lines[i]) - len(self.lines[i].lstrip())
                        self.lines[i] = " " * indent + f"{field}: {value},"
                        return
        raise ValueError(f"no {field} in the clip of media {media}")

    def replace_in_clip(self, media, old, new):
        for start, end, m in clip_blocks(self.lines):
            if m == media:
                for i in range(start, end):
                    if self.lines[i].strip() == old:
                        indent = len(self.lines[i]) - len(self.lines[i].lstrip())
                        self.lines[i : i + 1] = [" " * indent + n for n in new]
                        return
        raise ValueError(f"no {old} in the clip of media {media}")

    def track_sound(self, name, kind, settings):
        """Sets `field: value` lines inside a track's sound, by effect."""
        start, end, _ = next(t for t in track_blocks(self.lines, kind) if t[2] == name)
        section = None
        for i in range(start, end):
            stripped = self.lines[i].strip()
            m = re.match(r"(\w+): \($", stripped)
            if m:
                section = m.group(1)
            for (where, field), value in settings.items():
                if where in (section, None) and stripped.startswith(f"{field}:"):
                    indent = len(self.lines[i]) - len(self.lines[i].lstrip())
                    self.lines[i] = " " * indent + f"{field}: {value},"

    def replace_line(self, old, new):
        i = self.lines.index(old)
        self.lines[i : i + 1] = new

    def save(self):
        text = self.text()
        text = re.sub(r"\n    next_id: \d+,", f"\n    next_id: {self.next_id},", text)
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(text)


def block_end_bracket(lines, start):
    indent = len(lines[start]) - len(lines[start].lstrip())
    for i in range(start + 1, len(lines)):
        if lines[i] == " " * indent + "],":
            return i
    raise ValueError("no end of the list")


def media_entry(text, media):
    """The lines of a media entry, as the project lists it."""
    lines = text.split("\n")
    start = lines.index(" " * 6 + f"{media}: (")
    end = block_end(lines, start)
    entry = lines[start + 1 : end]
    # In a template the entries are a list, two steps less indented.
    return [" " * 6 + "(" ] + [line[2:] for line in entry] + [" " * 6 + "),"]


def template(text, media, seconds, linked):
    """A template of `seconds` of `media`'s start, picture and its first sound."""
    clip = lambda id, stream, link: [
        "          (",
        f"            id: {id},",
        "            media: 1,",
        f"            stream: {stream},",
        "            start: (0, 1),",
        "            source_in: (0, 1),",
        f"            duration: ({seconds}, 1),",
        f"            link: {link},",
        "            speed: (num: 1, den: 1),",
        "          ),",
    ]
    entry = media_entry(text, media)
    entry[1] = " " * 8 + "id: 1,"
    track = lambda id, kind, name, clips: [
        "      (",
        f"        id: {id},",
        f"        kind: {kind},",
        f'        name: "{name}",',
        "        clips: [",
        *clips,
        "        ],",
        "        muted: false,",
        "        solo: false,",
        "        locked: false,",
        "        hidden: false,",
        "        gain_db: 0.0,",
        "      ),",
    ]
    return "\n".join(
        [
            "(",
            "  format_version: 1,",
            "  template: (",
            f"    length: ({seconds}, 1),",
            "    media: [",
            *["  " + line for line in entry],
            "    ],",
            "    video_tracks: [",
            *track(2, "Video", "V1", clip(3, 0, "Some(4)" if linked else "None")),
            "    ],",
            "    audio_tracks: [",
            *(track(5, "Audio", "A1", clip(6, 1, "Some(4)")) if linked else []),
            "    ],",
            "  ),",
            ")",
            "",
        ]
    )


def main(path, data_dir):
    p = Project(path)
    ids = media_ids(p.text())

    def media(prefix):
        return next(v for k, v in ids.items() if k.startswith(prefix))

    ranked, clutch, facecam = media("Ranked match"), media("Clutch round"), media("Facecam")
    boss, picture, music = media("Boss fight"), media("Thumbnail idea"), media("Intro music")

    # Unused files, for Prune and the "Not in a timeline" filter.
    p.drop_clips(boss)
    p.drop_clips(picture)

    # The facecam on V2 from the start, sliding into the bottom right corner.
    cam = p.take_clip(facecam)
    p.add_track("Video", "V2", cam, "V1")
    p.set_in_clip(facecam, "start", "(0, 1)")
    p.set_in_clip(facecam, "duration", "(30, 1)")
    p.set_in_clip(facecam, "scale", "333333")
    p.set_in_clip(facecam, "x", "303333")
    p.set_in_clip(facecam, "y", "283333")
    p.replace_in_clip(
        facecam,
        "keyframes: {},",
        [
            "keyframes: {",
            "  X: [",
            "    (at: (0, 1), value: 720000, ease: Smooth),",
            "    (at: (2, 1), value: 303333, ease: Linear),",
            "  ],",
            "},",
        ],
    )

    # The music on a track of its own, under everything from the start.
    tune = p.take_clip(music)
    p.add_track("Audio", "Music", tune, "Voice chat")
    p.set_in_clip(music, "start", "(0, 1)")

    # Roles and a few effects, as someone mixing would set them.
    for name, role in ROLE.items():
        p.track_sound(name, "Audio", {(None, "role"): role})
    for name in ("Mic", "Voice chat"):
        p.track_sound(name, "Audio", {("high_pass", "on"): "true"})
    p.track_sound("Mic", "Audio", {("compressor", "on"): "true"})
    p.track_sound("Music", "Audio", {("ducking", "on"): "true"})

    # Timeline labels.
    seq_labels = [
        "        labels: [",
        "          (",
        f"            id: {p.new_id()},",
        '            name: "First kill",',
        "            at: (12, 1),",
        "            color: Green,",
        "          ),",
        "          (",
        f"            id: {p.new_id()},",
        '            name: "Comeback",',
        "            at: (27, 1),",
        "            color: Orange,",
        "          ),",
        "          (",
        f"            id: {p.new_id()},",
        '            name: "Clutch",',
        "            at: (48, 1),",
        "            color: Pink,",
        "          ),",
        "        ],",
    ]
    p.replace_line("        labels: [],", seq_labels)

    # The Library: labels, a list and a note.
    funny, clutch_label, broll = p.new_id(), p.new_id(), p.new_id()
    team = p.new_id()
    library = [
        "    library: (",
        "      labels: [",
        f'        (id: {clutch_label}, name: "Clutch", color: Orange),',
        f'        (id: {funny}, name: "Funny", color: Pink),',
        f'        (id: {broll}, name: "B-roll", color: Blue),',
        "      ],",
        "      tagged: {",
        f"        {ranked}: [{clutch_label}],",
        f"        {clutch}: [{clutch_label}, {funny}],",
        f"        {boss}: [{funny}],",
        f"        {facecam}: [{broll}],",
        f"        {picture}: [{broll}],",
        "      },",
        "      notes: {",
        f'        {ranked}: "The comeback starts at 0:25. Keep the voice chat here.",',
        "      },",
        "      lists: [",
        f'        (id: {team}, name: "Clips for the team", media: [{clutch}, {ranked}]),',
        "      ],",
        "      uses: {},",
        "    ),",
    ]
    start = p.lines.index("    library: (")
    end = block_end(p.lines, start)
    p.lines[start : end + 1] = library

    # The music dips under the comeback, as a volume line on its track.
    start, end, _ = next(t for t in track_blocks(p.lines, "Audio") if t[2] == "Music")
    at = next(i for i in range(start, end) if p.lines[i].strip() == "volume_line: [],")
    indent = " " * (len(p.lines[at]) - len(p.lines[at].lstrip()))
    points = [(0, 0), (22, 0), (24, -9000), (30, -9000)]
    p.lines[at : at + 1] = (
        [indent + "volume_line: ["]
        + [indent + f"  (at: ({t}, 1), value: {v})," for t, v in points]
        + [indent + "],"]
    )

    p.save()
    for side in (".journal", ".journal.start"):
        try:
            os.remove(path + side)
        except FileNotFoundError:
            pass

    # Two templates, made from the boss fight recording.
    text = p.text()
    templates = os.path.join(data_dir, "templates")
    os.makedirs(templates, exist_ok=True)
    for name, seconds in (("Channel intro", 4), ("Outro with end card", 8)):
        with open(os.path.join(templates, name + ".template"), "w", encoding="utf-8") as f:
            f.write(template(text, boss, seconds, linked=True))

    # Two saved versions: copies of the project as it is now.
    versions = path + ".versions"
    os.makedirs(versions, exist_ok=True)
    for stamp, name in ((1794470000000, "First rough cut"), (1794730000000, "Before the music")):
        with open(os.path.join(versions, f"{stamp} {name}.kiria"), "w", encoding="utf-8") as f:
            f.write(text)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
