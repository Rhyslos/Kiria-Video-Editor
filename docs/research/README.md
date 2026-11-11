# docs/research

One note per big system, written *before* it is built (working rule 7). Big systems include playback, caching, the timeline widget, rendering, the audio engine, background removal, auto white balance and the highlight finder.

Name each note after its system, for example `playback.md`, and give it these sections:

1. **Problem:** what the system must do and the numbers it must hit.
2. **Approaches:** known ways to solve it, with sources.
3. **Prototypes:** the two or three tried in `spikes/<system>/`, and how they were measured.
4. **Result:** the comparison table and the choice, with the reason.

- [decoding.md](decoding.md): getting video frames from files into GPU textures (4A).
- [timeline.md](timeline.md): the timeline widget (4B): painter with culling, measured.
