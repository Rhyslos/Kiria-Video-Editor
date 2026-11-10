# ui-kit

Shared interface parts. Anything that must look and behave the same on every page lives here, so pages never draw their own version (working rules 1 to 3).

## Status

Phase 0.6 done: themes, fonts, shared sizes, the top bar, the shortcut table and error display. The shared timeline widget (playhead, edit needle, region, labels, ruler, transport) arrives in Phase 4.

## Files

| File | What it holds |
|---|---|
| `src/theme.rs` | `ThemeName` (Ash, Carbon) and `Theme`: every interface colour as a named token, plus fixed `status` colours (good, warning, error, edit needle). `Theme::apply` styles egui |
| `src/fonts.rs` | Installs the bundled sans-serif (interface) and monospace (timecodes, numbers) fonts; `semibold()` and `mono(size)` helpers |
| `src/layout.rs` | Shared sizes (top bar 48, right panel 292, master 80), `viewport_centre_x` for the tabs, and `Page` |
| `src/top_bar.rs` | `TopBar`: project name, page tabs centred over the viewport, search, resource readout, settings |
| `src/shortcuts.rs` | `Action`, `Shortcut`, `ShortcutTable`: the one table of shortcuts, matched on physical key positions |
| `src/errors.rs` | `ErrorCenter`: the inline error strip with its fix button, and the Diagnostics window |
| `assets/fonts/` | The font files and their licences |

## Rules

- Colours come only from `Theme` and `status`. No hex values in pages.
- Sizes that more than one page uses come from `layout`.
- New shortcuts go into `ShortcutTable::default()`; the test there refuses a key bound twice.
- Shortcuts match on the physical key, so they work on every keyboard layout. While a text field has focus only Ctrl shortcuts fire.
