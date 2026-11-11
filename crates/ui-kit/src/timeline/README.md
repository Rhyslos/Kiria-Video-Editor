# timeline

Parts of the timeline widget (`../timeline.rs`) that stand on their own. The widget itself, its input and its drawing stay in `timeline.rs`.

| File | What it holds |
|---|---|
| `rows.rs` | `TrackHeights`: each track's height as a factor of the usual height of its kind (28 to 400 points), with what to save and a revision that counts changes. `Rows`: the rows of one area for tracks of mixed heights (video rows stack upward, audio rows downward), which row a position is in, and how far the area can scroll |
