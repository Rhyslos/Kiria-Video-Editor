# docs/research

One note per big system, written *before* it is built (working rule 7). Big systems include playback, caching, the timeline widget, rendering, the audio engine, background removal, auto white balance and the highlight finder.

Name each note after its system, for example `playback.md`, and give it these sections:

1. **Problem:** what the system must do and the numbers it must hit.
2. **Approaches:** known ways to solve it, with sources.
3. **Prototypes:** the two or three tried in `spikes/<system>/`, and how they were measured.
4. **Result:** the comparison table and the choice, with the reason.

- [decoding.md](decoding.md): getting video frames from files into GPU textures (4A).
- [caching.md](caching.md): showing a frame within 50 ms while scrubbing and jumping (4A).
- [timeline.md](timeline.md): the timeline widget (4B): painter with culling, measured.
- [rendering.md](rendering.md): putting the timeline's picture together from its layers on the GPU (4C on). Measured: a worker composes into a floating-point frame in linear light.
- [audio-engine.md](audio-engine.md): the real-time audio engine (8.1): effects per track and on the master, how settings reach the real-time thread, where the mix runs. Measured: a fixed strip per track, mixed in the sound card's callback in 256-sample blocks, settings as a command queue; 32 tracks with every effect take about 15 % of one core.
- [sync-by-sound.md](sync-by-sound.md): lining up a separate mic or a friend's recording by what both hear (8.3). A short comparison in a test, no spike: whitened correlation in the voice band beats onset correlation 7 to 3 on made-up recordings, then the exact fit at 48 kHz.
- [highlights.md](highlights.md): finding highlights in long game recordings (5.4): sound loudness and onsets per stream, cuts and sudden motion in the picture, scored against picked moments. Preliminary; waiting for the owner's recordings.
