# editor

Parts of the window (`../editor.rs`, `Editor`) with files of their own; each is an `impl Editor` block.

| File | What it holds |
|---|---|
| `keyframe_clipboard.rs` | Copy and Paste for keyframe marks |
| `timelines.rs` | Several timelines on the Edit page: the tabs, opening, making, copying, nesting and placing timelines, adjustment layers |
| `audio_ui.rs` | The Audio page as the window runs it: the shared viewer top left, the page's tools top right, the clarity check's band (followed every frame, its fixes applied as one undo step each, its findings handed to the timeline as coloured stretches), the bar (Timeline / Mixer / Split and its direction, Show, the timeline's tools, the lines tool) and the timeline (one video reference row, the audio tracks shown, their lines) and the mixer under it; the meters taken once a frame, the overview's counting, the analyser's track; the line and role events of every timeline; the demo tracks of `--audio-tracks` |
| `library_ui.rs` | The Library page as the window runs it: what the page is handed, what it asked for, and its player following its clock |

The app's [README](../../README.md) says more about each.
