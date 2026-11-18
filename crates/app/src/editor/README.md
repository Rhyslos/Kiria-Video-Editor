# editor

Parts of the window (`../editor.rs`, `Editor`) with files of their own; each is an `impl Editor` block.

| File | What it holds |
|---|---|
| `keyframe_clipboard.rs` | Copy and Paste for keyframe marks |
| `timelines.rs` | Several timelines on the Edit page: the tabs, opening, making, copying, nesting and placing timelines, adjustment layers |
| `follow.rs` | "Follow it from here" on a Blur or Pixelate box: starts `engine::track::Following` from the playhead to the clip's end, shows how far it is in the status line (Escape stops it: the shortcut table's `ClearSelection`, which then does nothing else that press, its key named from the table), and sets what it found as the box's Left and Top keyframes (`Effect::following`, one `SetEffects`) |
| `steady.rs` | "Analyse the clip" on a Stabilise effect: starts `engine::steady::Analysing` over the whole clip, shows how far it is (Escape stops it, nothing set, as for `follow.rs`), and sets the view keyframes and zoom it found on the effect, one `SetEffects` |
| `color_ui.rs` | The Color page's layout: the shared viewer (showing a grade being dragged), the bar of the clip being graded, the tools under them, and the timeline with the sound folded in |
| `audio_ui.rs` | The Audio page as the window runs it, its parts laid out by `audio_page::heights` with a handle between each two (`ui_kit::splitter::stack`, the heights a setting): the shared viewer top left, the page's tools top right, the band across the middle (the clarity check, followed every frame, its fixes applied as one undo step each, its findings handed to the timeline as coloured stretches; or a live picture of the sound: the band's histories followed every frame from what was measured, the analyser told what to listen to), the bar (Timeline / Mixer / Split and its direction, Show, the timeline's tools, the lines tool) and the timeline (one video reference row, the audio tracks shown, their lines) and the mixer under it; the meters taken once a frame, the overview's counting, the analyser's track; the line and role events of every timeline |
| `settings_ui.rs` | The Settings window: `CATEGORIES` (the tabs Look, Editing, Playback & proxies, Audio, Library, Resources, Cache, Graphics, Keyboard and Diagnostics, their cards and rows, with each row's id as a constant) and each row's control (`ui_kit::settings`); `open_settings_at(id)` opens it at a tab or setting (the top bar's resource readout opens Resources) |
| `library_ui.rs` | The Library page as the window runs it: what the page is handed, what it asked for, and its player following its clock |
| `script.rs` | What an automated run scripts (`run_mode::Automation`, nothing in a normal run): `Script`, the steps the window then takes by itself once the project and its imports are in (`step`, once per frame: `--action` and `--then` commands, `--select-first-clip`, `--sync-by-sound`, `--play`, `--library-tab`, `--package-to`, `--close-request`, and the Audio page's demo: `--audio-tracks` makes the project anew as one undoable `Restore`, `--audio-clarity-region`, `--audio-track`); how the window starts (`settings`, `arrange`: page, drawer, full screen, the Export page's demo queue, Settings and the shortcuts page); what a screenshot waits for and how long |

The app's [README](../../README.md) says more about each.
