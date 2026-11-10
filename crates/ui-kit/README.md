# ui-kit

Shared interface parts. Anything that must look and behave the same on every page lives here, so pages never draw their own version (working rules 1 to 3).

## Status

Phase 0.6 done: themes, fonts, shared sizes, the top bar, the shortcut table and error display. Phase 2 added the title bar for the launcher, the shared dialog look and the shared time wording. The shared timeline widget (playhead, edit needle, region, labels, ruler, transport) arrives in Phase 4.

## Files

| File | What it holds |
|---|---|
| `src/theme.rs` | `ThemeName` (Ash, Carbon) and `Theme`: every interface colour as a named token, plus fixed `status` colours (good, warning, error, edit needle). `Theme::apply` styles egui |
| `src/fonts.rs` | Installs the bundled sans-serif (interface) and monospace (timecodes, numbers) fonts; `semibold()` and `mono(size)` helpers |
| `src/layout.rs` | Shared sizes (top bar 48, left panel 240, right panel 292, master 80), `viewport_centre_x` for the tabs, and `Page` |
| `src/top_bar.rs` | `TopBar`: project name, page tabs centred over the viewport, search, resource readout, settings. `TitleBar`: the same bar for windows without pages (logo and name centred, settings right). `logo_mark` |
| `src/time_text.rs` | Times written one way everywhere: `ago` ("2 min ago", "yesterday") and `duration` ("4:05", "1:02:09") |
| `src/shortcuts.rs` | `Action`, `Shortcut`, `ShortcutTable`: the one table of shortcuts, matched on physical key positions |
| `src/icon.rs` | The app icon: `logo_mark`'s picture in the top bar and `with_window_icon` for every window |
| `src/dialog.rs` | `dialog`: every window that asks something and waits uses this frame, padding and dimmed backdrop |
| `src/errors.rs` | `ErrorCenter`: the inline error strip with its fix button, and the Diagnostics window |
| `assets/fonts/` | The font files and their licences |
| `assets/icon/` | `icon-64.png`, drawn by `cargo xtask icons` from `packaging/icons/icon.svg` |

## Rules

- Colours come only from `Theme` and `status`. No hex values in pages.
- Sizes that more than one page uses come from `layout`.
- New shortcuts go into `ShortcutTable::default()`; the test there refuses a key bound twice.
- Shortcuts match on the physical key, so they work on every keyboard layout. While a text field has focus only Ctrl shortcuts fire.
