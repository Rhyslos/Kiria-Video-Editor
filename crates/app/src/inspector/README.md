# inspector

The parts of the Edit page's inspector. `../inspector.rs` keeps the panel's state (the values in waiting, each a `crate::pending::Pending`, and what the page takes from it: `seek`, `follow`, `steady`), the project as it would be (`shown`, `sounds_being_set`), the hand on the picture in the viewer (`subject`, `changed_outside`, `let_go`), which clip is shown (`shown_clip`, `effects_target`, `shows_a_clip`), the timeline's rows when no clip is, and `ui`, which runs one frame in named steps: finish what was left on another clip, the header, Title, the picture sections, Effects, Transition, Speed, Audio, then the look's undo step and what Reset and the diamonds asked for (`answer`).

| File | What it holds |
|---|---|
| `look.rs` | A look's numbers as the panel shows them (`Values`: pixels, percent and degrees, read back without changing what wasn't touched), the range each number moves in, Fit (`fitted`, also used by the handles on the picture) and Fill (`fill_percent`), the corner and half layouts (`in_corner`, `in_half`) and each picture section's one-line summary. Pure arithmetic, nothing drawn |
| `keyframes.rs` | `KeyTable`: where each group of the look's numbers has keyframes at the playhead (at one, between, none), the nearest before and after and the ease of the one there, for the diamonds; `over_time`, the "Over time: …" start of a section's summary |
| `picture.rs` | Transform (with the Blend row, Fit, Fill, "On the picture" and the layouts), Crop and Quick color, or Opacity and Quick color for an adjustment layer; the fields each section's Reset takes; `Asks`, what was asked for besides changed numbers |
| `sound.rs` | The Audio section (volume, pan, fade in and out of the clip's sound clips, set together as one `SetSound`), `SoundValues` and the words its numbers are written and read in ("−3.0 dB", "L 20", "0.50 s · 30 frames"), the header's summary, and which sound clips a clip's Audio sets (`sound_clips`) |
| `effects.rs` | The Effects section (see the crate README) |
| `speed.rs` | The Speed section (see the crate README) |
| `title.rs` | The Title section (see the crate README) |
| `transition.rs` | The Transition in section (see the crate README) |
