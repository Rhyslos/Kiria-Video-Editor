# color_page

The tools of the Color page. `../color_page.rs` keeps the page's state (`ColorPage`): the tool switcher, the layer dropdown with its Simple / Advanced switch, and the bar with Apply to (every clip, the same file, a label), Copy grade, Paste and Reset; `graded_clip` (the selected picture, else the top one at the playhead), `reached`, `shown` (a grade being dragged, or the clip without its grade while Before is on), `pick_white` (a click on the picture sets white balance from what is there), `pick_range` (a click puts the layer's colour range round what is there), `mask_shown` and `drag_mask` (the mask drawn over the picture and dragged there, one undo step when let go), `scope_ui` (the scope chosen, from the viewer's newest `sampled` copy, with its reading). `../editor/color_ui.rs` lays the page out.

Every change is one `SetGrades` undo step; a value being dragged is shown at once and becomes one when let go.

| File | What it holds |
|---|---|
| `basic.rs` | Basic: white balance (temperature and tint, Auto and its strength), then exposure, contrast and its pivot, saturation, highlights and shadows, each a slider with its number |
| `wheels.rs` | Wheels: Shadows, Midtones, Highlights and Overall, each with a brightness slider; Advanced draws them larger with red, green, blue and brightness to type |
| `curves.rs` | Curves: Master, Red, Green and Blue, one shown at a time; points added, dragged and taken away, the ends moving up and down only; Reset curve |
| `range.rs` | Color range: a layer kept to a range of hue, saturation and lightness with soft edges; "Pick from the picture" |
| `mask.rs` | Mask: a layer kept to an ellipse or rectangle (or what is outside it) with a soft edge; its outline over the picture, dragged there |
