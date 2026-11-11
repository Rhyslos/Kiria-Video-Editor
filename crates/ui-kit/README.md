# ui-kit

Shared interface parts. Anything that must look and behave the same on every page lives here, so pages never draw their own version (working rules 1 to 3).

## Status

Phase 0.6 done: themes, fonts, shared sizes, the top bar, the shortcut table and error display. Phase 2 added the title bar for the launcher, the shared dialog look and the shared time wording. The shared timeline widget (playhead, edit needle, region, labels, ruler, transport) arrives in Phase 4.

## Files

| File | What it holds |
|---|---|
| `src/theme.rs` | `ThemeName` (Ash, Carbon) and `Theme`: every interface colour as a named token, plus fixed `status` colours (good, warning, error, edit needle). `Theme::apply` styles egui |
| `src/fonts.rs` | Installs the bundled sans-serif (interface) and monospace (timecodes, numbers) fonts; `semibold()` and `mono(size)` helpers |
| `src/layout.rs` | Shared sizes (top bar 48, left panel 240, right panel 292, drawer 264 with its 22 edge tab, master 80), `viewport_centre_x` for the tabs, and `Page` |
| `src/top_bar.rs` | `TopBar`: project name, page tabs centred over the viewport, search, resource readout, settings. `TitleBar`: the same bar for windows without pages (logo and name centred, settings right). `logo_mark` |
| `src/time_text.rs` | Times written one way everywhere: `ago` ("2 min ago", "yesterday") and `duration` ("4:05", "1:02:09") |
| `src/timeline.rs` | The timeline widget every page shares: ruler with timecode, labels row, video area (V1 at the bottom) and audio area that scroll separately, a draggable divider, the playhead, the red edit needle (dragged by its handle), the scroll bar, Ctrl+wheel zoom at the pointer. Draws only what is visible and merges clips too thin to tell apart (`docs/research/timeline.md`); returns `TimelineEvent`s. Tools: Select (click, drag to move, drag an edge to trim) Blade (click to cut), Ripple (trims move everything after), Roll (drag the cut between two touching clips), Slip (drag inside a clip to show another part of its file) and Slide (drag a clip between its neighbours); snapping to clip edges, the playhead, the needle and labels (Alt held while dragging skips it). Labels: click one to go to it, double-click to edit it (`EditLabel`). `highlights` marks stretches over every track (the silences Remove silence found). Video clips draw their pictures (`ClipPictures`) side by side, each the one nearest its place in the file, with the name on a dark pill. Audio clips draw their waveform (one bar per pixel column); audio track headers have M (mute), S (solo) and a volume fader (double-click for 0 dB). `toolbar` draws the tool row. `tests/timeline_cost.rs` measures a frame |
| `src/transport.rs` | The floating transport over the bottom of the viewer: play/pause at the exact centre, frame back and forward, stop |
| `src/viewer.rs` | The viewer every page shares: the timeline-shaped black frame, the picture fitted inside, a quiet note in the corner; `PreviewQuality` (Auto, Full, 1/2, 1/4) |
| `src/shortcuts.rs` | `Action`, `Shortcut`, `ShortcutTable`: the one table of shortcuts, matched on physical key positions |
| `src/icon.rs` | The app icon: `logo_mark`'s picture in the top bar and `with_window_icon` for every window |
| `src/drawer.rs` | The drawer on a page's left edge: `edge_tab` (a tall tab with its label written upward) and the Media / Effects `tab_switch`; the page fills in the list |
| `src/dialog.rs` | `dialog`: every window that asks something and waits uses this frame, padding and dimmed backdrop |
| `src/errors.rs` | `ErrorCenter`: the inline error strip with its fix button, and the Diagnostics window |
| `assets/fonts/` | The font files and their licences |
| `assets/icon/` | `icon-64.png`, drawn by `cargo xtask icons` from `packaging/icons/icon.svg` |

## Rules

- Colours come only from `Theme` and `status`. No hex values in pages.
- Sizes that more than one page uses come from `layout`.
- New shortcuts go into `ShortcutTable::default()`; the test there refuses a key bound twice.
- Shortcuts match on the physical key, so they work on every keyboard layout. While a text field has focus only Ctrl shortcuts fire.
