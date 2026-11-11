# timeline

Parts of the timeline widget (`../timeline.rs`) that stand on their own. The widget itself, its input and its drawing stay in `timeline.rs`.

| File | What it holds |
|---|---|
| `rows.rs` | `TrackHeights`: each track's height as a factor of the usual height of its kind (28 to 400 points), with what to save and a revision that counts changes. `Rows`: the rows of one area for tracks of mixed heights (video rows stack upward, audio rows downward), which row a position is in, and how far the area can scroll |
| `loading.rs` | `Loading`: background reading a clip or a track still waits for (sound or pictures; waiting, being read with the share done and the time left, or failed), what it says in a clip of any width and by a track's name, and how it is drawn |
| `cache_bar.rs` | The cache bar under the ruler: `CacheState` (ready, preparing, waiting), `CacheRange`, `flatten` (one tidy list out of overlapping ranges; the least ready state wins), and drawing that skips what is out of view and gives ranges too thin to tell apart one shared span |
