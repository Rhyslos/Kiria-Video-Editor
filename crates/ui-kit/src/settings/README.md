# settings

Parts of Settings (`../settings.rs`) with files of their own.

| File | What it holds |
|---|---|
| `catalogue.rs` | What Settings holds, as data: `Category` (a tab), `Section` (a card), `Setting` (a row: id, label, one line of description, keywords to be found by, whether a change waits for the next start). `place_of` finds a tab, card or row by id (a tab or card: its first row); `search` and `found` find rows whose label, description, keywords, card title or tab name hold every word typed, in any case and either number ("proxy" finds "Proxies"); `repeated_ids`, for a program's test that its ids are its own |
| `controls.rs` | The controls of a row, one per kind of choice: `toggle` (on and off), `segmented` (two or three choices), `dropdown` (more), `number` (a slider and its value in a box), `auto_number` (an Auto switch, and the number beside it while Auto is off), `action` (a button such as "Clear all…"), `value` (words that can only be read). They are added from the right; every one is reached with Tab, worked with Space or Enter, and ringed in the accent colour while it has the keyboard |

## Using it

1. Describe the settings as a `const` slice of `Category`; give each row an id that is unique in the program (a test calls `repeated_ids`).
2. Keep a `SettingsView` and, while Settings is open, call `ui_kit::settings::window(ctx, theme, &mut open, |ui| view.show(ui, theme, CATEGORIES, |row| match row.id() { ... }))`, adding each row's control to `row.ui`.
3. To open Settings at a setting from elsewhere, call `view.open_at(CATEGORIES, id)` and open the window.

Changes go wherever the program keeps its settings; nothing here stores them.
