# timeline

Parts of the timeline widget (`../timeline.rs`) that stand on their own. The widget itself, its input and its drawing stay in `timeline.rs`.

| File | What it holds |
|---|---|
| `rows.rs` | `TrackHeights`: each track's height as a factor of the usual height of its kind (28 to 400 points), with what to save, a revision that counts changes, and `keep_only`, which puts the heights of removed tracks aside (they are not saved) and gives a track its height back when it returns, as it does when its removal is undone. `Rows`: the rows of one area for tracks of mixed heights (video rows stack upward, audio rows downward), which row a position is in, the nearest row to a position past the first or last (where a dragged clip or a dropped file lands), and how far the area can scroll |
| `headers.rs` | The column of track names: its right-click menu (add a video or an audio track; rename, hide or remove the track under the pointer, with the reason when it can't be removed), the eye in a video track's header that hides the track, and the field a name is typed in after a double-click (Enter or a click elsewhere gives the name, Escape leaves it) |
| `loading.rs` | `Loading`: background reading a clip or a track still waits for (sound or pictures; waiting, being read with the share done and the time left, or failed), what it says in a clip of any width and by a track's name, and how it is drawn |
| `cache_bar.rs` | The cache bar under the ruler: `CacheState` (ready, preparing, waiting), `CacheRange`, `flatten` (one tidy list out of overlapping ranges; the least ready state wins), and drawing that skips what is out of view and gives ranges too thin to tell apart one shared span |
