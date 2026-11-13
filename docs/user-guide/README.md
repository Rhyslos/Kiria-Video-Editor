# user-guide

The user guide: how to use the app, page by page, with real screenshots and every shortcut. It ships with every installer and opens in the system's web browser from Settings › Help in the editor and the launcher, with F1, and from the editor's search (Ctrl+K "help").

Update it in the same commit as any feature it describes (CLAUDE.md, Decisions).

| File or folder | What it holds |
|---|---|
| `index.html` | The whole guide in one page: its words, styles and script inline, nothing fetched from the network, so it opens from disk (`file://`) in any browser. The app's icon (`packaging/icons/icon.svg`, as a data URI) is its logo and tab icon. To keep the reader's place: numbered contents on the left that open out only the chapter being read and light the section on screen, the chapter and section named in the top bar, a reading-progress line under it, the address kept on the section being read (replaced, so Back still leaves the page), a `#` link on every heading, previous and next chapter at the end of each chapter, and a button back to the top. The search box (`/` or Ctrl+K) lists the places that match with the words around each (arrows choose, Enter goes there and makes the match flash, Escape clears) and marks the words on the page. Light and dark follow the system, with a button for Auto, Light and Dark, remembered in the browser and set in `<head>` before anything is drawn. Pictures open large with their caption on a click. Below 920 px wide the contents become a drawer behind a button. The shortcut tables at the end (between the `shortcuts:start` and `shortcuts:end` marks) are made from the app's shortcut table: don't edit them by hand |
| `images/` | The screenshots, as JPEG files the page loads lazily. Made by `tools/shots.sh`; keep the folder under about 15 MB |
| `tools/shots.sh` | Takes every screenshot again from the real programs under a virtual screen (Linux, `xvfb-run`), in a private home folder, and writes `images/`. `tools/shots.sh audio` takes only those whose name has "audio". It makes its own recordings with FFmpeg and the demo project in `target/user-guide-shots/` |
| `tools/demo_project.py` | Dresses the project the editor made from the imports into the demo one: labels, a list and a note on files, timeline labels, the facecam on its own track sliding into a corner, the music on its own track with a volume line, track roles and a few effects, two files left out of the timeline (for Prune), two templates and two saved versions. It edits the project file as text, so it knows only the format the editor writes now (17); when that changes, `shots.sh` stops at the first editor run and this needs a look |

Only `index.html` and `images/` are installed (`packaging/windows/stage.ps1`, `packaging/linux/build-appimage.sh`); the programs look for them as `user-guide/index.html` beside themselves, then in the current version's folder (the installed launcher), then in `../share/user-guide/` (the AppImage), then here in a debug build (`ui_kit::user_guide`).

## Keeping it true

- **Shortcuts:** `crates/ui-kit/tests/user_guide_shortcuts.rs` fails when the tables at the end of `index.html` differ from the shortcut table (a command, a key, a name or a mouse line changed). `UPDATE_USER_GUIDE=1 cargo test -p ui-kit --test user_guide_shortcuts` writes them again.
- **Words:** each section names the buttons, menus and settings as the interface writes them. When a feature changes, change its section; search the page for the old name.
- **Pictures:** run `tools/shots.sh` (or with a filter for the pictures that changed), then look at every picture before committing. Pointer positions in the script (`--pointer x,y`) are for a 1440 × 900 window.
- **No other editors:** like the rest of the project (CLAUDE.md, rule 4), the guide describes features rather than naming other video editors. Search words such as "razor" or "compound clip" are fine as words.
- **Not in it yet:** the Color page and effects (phases 7 and 9). Add their chapters when they are built.

## Checking it in a browser

Open `index.html` from disk. To check it without a screen: Chromium with Playwright (`require('playwright')`, `chromium.launch()`), full-page screenshots at a desktop and a narrow width in light and dark, the search (type a word, Enter), every image loading and every `#` link landing on an id.
