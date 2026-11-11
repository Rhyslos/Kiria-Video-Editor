# keyboard

Parts of the keyboard shortcuts page (`../keyboard.rs`, which holds `ShortcutsPage`: the page's state, its filters and search, and its drawing).

| File | What it holds |
|---|---|
| `layout.rs` | Keyboards as they are drawn, with nothing from the system in it. `Shape` (ANSI, ISO, JIS) gives every key's place and size (`Cap`, in key units; the tall Enter is one six-cornered key). `Layout` is a shape and what is printed where it differs from a US keyboard; `us_mark` is the US character shown beside it, so the letter of a shortcut can be found on any keyboard. `LayoutId` names the built-in layouts (US, UK, Norwegian, Japanese) for settings and the dropdown, and finds one from a Windows language identifier or an XKB layout name |
| `drawing.rs` | How the page draws one key (tinted and dotted by context, dimmed when unused, the tall Enter under one outline, the US character in the corner where the layout prints another), a context chip and a context dot |
| `rebind.rs` | `Rebinding`: changing a shortcut step by step (waiting for keys; a clash that waits for swap or cancel; reset, which can clash too). No drawing |
| `cheat_sheet.rs` | `html`: the printable cheat sheet, one HTML file with nothing to fetch: a keyboard picture per combination of Ctrl, Shift and Alt in use, every command by context with the keys it has now, and the mouse |

## The layout the system reports

Not read yet: the keyboard drawn is the one picked in the dropdown (kept in settings). What is missing is only the asking. On Windows that is one call for the active layout's handle, whose low word goes to `LayoutId::from_windows_language`; it is a foreign call, so it needs `unsafe`, which the code rules keep out of this crate and the app. On Linux it is the XKB layout name, which goes to `LayoutId::from_xkb_name`. Asked once a second or so while the page is open, either would follow a layout switch live; the dropdown would then be the manual override. Layouts beyond the four built in (Arabic, Russian and the rest) are a `Layout` each: a shape and the characters that differ.
