# shortcuts

Parts of the one shortcut table (`../shortcuts.rs`, which holds `Shortcut`, `Binding` and `ShortcutTable` itself: the combinations in use, looking them up, changing them and the clash rule).

| File | What it holds |
|---|---|
| `actions.rs` | `Action`: every command a shortcut can trigger, from one list that gives each its name for people, its `Context` and the name settings know it by. `Context` (Anywhere, Viewer, Timeline, While dragging): where a command acts, with the colour the shortcuts page gives it |
| `saved.rs` | `SavedBindings`: the changed lines of the table as settings keep them, by command name and with the keys as text ("Ctrl+Shift+Z"). `ShortcutTable::saved` writes them and `ShortcutTable::restored` reads them back, skipping what it does not know and never putting two commands on one combination |
| `mouse.rs` | `MOUSE`: what the mouse does (wheel, drags, clicks, keys held while dragging), by context, for the Mouse section and the cheat sheet. Shown, not changeable: update a line with the widget it describes |

## Rules

- A new command is one line in `actions.rs` (`actions!`), and one in `USUAL` in `../shortcuts.rs` if it has a shortcut from the start.
- One combination does one thing in the whole table: every context is live at once today. `ShortcutTable::clash` is the only place that decides it; when pages bring contexts that are never live together, that is where they are compared.
- Text that names a shortcut asks the table (`keys_for`, `keys_or_name`, `tip`), so it follows a changed shortcut. No key names typed into interface text.
