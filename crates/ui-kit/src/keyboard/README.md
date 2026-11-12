# keyboard

Parts of the keyboard shortcuts page (`../keyboard.rs`, which holds `ShortcutsPage`: the page's state, its filters and search, and its drawing).

| File | What it holds |
|---|---|
| `layout.rs` | Keyboards as they are drawn, with nothing from the system in it. `Shape` (ANSI, ISO, JIS) gives every key's place and size (`Cap`, in key units; the tall Enter is one six-cornered key). `Layout` is a shape and what is printed where it differs from a US keyboard; `us_mark` is the US character shown beside it, so the letter of a shortcut can be found on any keyboard. `LayoutId` names the built-in layouts (US, UK, Norwegian, Japanese) for settings and the dropdown, and finds one from a Windows language identifier or an XKB layout name. `Layout::written` is a shortcut as that keyboard prints it ("Ctrl+-" for Ctrl+Slash on a Norwegian keyboard; a printed + is "Plus"), `describe` adds the US name where it differs. `KeyboardChoice` (Automatic, the usual, or `Fixed`) is what settings keep; `SystemKeyboard` is what the system reports, as the page shows it, and `resolve` picks the layout to draw (US English when the system's isn't drawn yet) |
| `drawing.rs` | How the page draws one key (tinted and dotted by context, dimmed when unused, the tall Enter under one outline, the US character in the corner where the layout prints another), a context chip and a context dot |
| `rebind.rs` | `Rebinding`: changing a shortcut step by step (waiting for keys; a clash that waits for swap or cancel; reset, which can clash too). No drawing |
| `cheat_sheet.rs` | `html`: the printable cheat sheet, one HTML file with nothing to fetch: a keyboard picture per combination of Ctrl, Shift and Alt in use, every command by context with the keys it has now, and the mouse |

## The layout the system reports

Asked by the app through `crates/os` (the only place a foreign call may be made) and handed to the page as a `SystemKeyboard`: on Windows the active layout's language identifier goes to `LayoutId::from_windows_language`, on Linux the X server's active XKB layout name to `LayoutId::from_xkb_name`. With "Automatic (from the system)" in the dropdown (the usual) the page draws that keyboard and follows a switch within half a second; picking a layout there overrides it, and the system's own is then shown beside the dropdown when it differs. A layout the drawing doesn't know is named ("language 0419", "\"ru\"") and US English is drawn. Layouts beyond the four built in (Arabic, Russian and the rest) are a `Layout` each: a shape and the characters that differ, plus a line in each lookup.
