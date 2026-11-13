# Sound quality: file to speakers

A follow-up to [audio-engine.md](audio-engine.md), not a new system: the owner played the same game recording (a 3 h MKV, five AAC-LC 48 kHz stereo streams: game, mic, voice call, music, browser) in an established editor and here. Here it sounded "flat"; there "inside the game". Turning the volume up here made the two sound the same, so the difference is level, with width and depth following it.

## 1. Problem

For a stereo stream at default settings (clip, track and master at 0 dB, centred, every effect off), what the sound card is handed must be the file's own samples, left and right, at the file's level. Several streams are their sum. Where is level lost between the file and the speakers?

## 2. The path, stage by stage

| Stage | What it does to a centred stereo stream at default settings |
|---|---|
| `media::audio` | FFmpeg decodes; `swr` converts to interleaved 32-bit float at the mixer's rate. Stereo in, stereo out: an exact copy (no matrix). At the same rate nothing is resampled. Surround is folded to stereo with FFmpeg's default matrix (centre and surrounds at -3 dB, the low-frequency channel left out, not scaled down for float), exactly as `ffmpeg -ac 2` does. A file read at another rate goes through `swr`'s default filter. |
| `engine::mix` (the feeder) | A clip left as recorded is not touched (`Shape::of` is `None`); it is added to a silent buffer times 1.0. Each stream is its own track. |
| `crates/audio` strip | High pass, EQ, compressor, Loudness tool, ducking off: skipped. The clean-up, off, is a plain delay to the bit. The fader: `volume_gain(0) = 1`, `pan_gains(0) = [1, 1]` (the centre leaves both sides as they are; the pan is a balance, not a -3 dB law), no clip pan on top. |
| Master | Volume 0 dB = 1.0; compressor off; trim 0; limiter off: its ceiling is infinite and only its fixed delay runs (gain exactly 1). |
| `engine::output` | Clamps to ±1 (the card would anyway), converts to the device's sample format when it isn't float. Left and right go to the device's first two channels, the rest silent; a one-channel device gets their average. |

Nothing turns anything down by default. Nothing makes the import quieter: each stream is placed at 0 dB on its own track.

## 3. Measured

Files made with FFmpeg (wide stereo pink noise with different tones left and right, the same noise upside down on one side, a 20 Hz to 20 kHz sweep, 5.1 and 7.1 with a tone per channel, AAC, Opus and PCM at 44.1 and 48 kHz, and a five-stream MKV laid out like the owner's), each imported into a project and mixed by the real path without a device (`engine::offline::OfflineMix`, which a test shows equals playback to the sample), compared with FFmpeg's own decode of each stream and their sum:

| File | Here against FFmpeg | L/R correlation, here / file | Level |
|---|---|---|---|
| Wide stereo, AAC 48 kHz | -220 dB (float rounding) | -0.002 / -0.002 | equal to 0.0 dB |
| Wide stereo, Opus 48 kHz | -220 dB | -0.011 / -0.011 | equal |
| Wide stereo, AAC 44.1 kHz read at 48 kHz | -220 dB | -0.004 / -0.004 | equal |
| Out of phase (R = -L), AAC | -215 dB | -1.000 / -1.000 | equal |
| 5.1 AAC, against `-ac 2` | -222 dB | 0.250 / 0.250 | equal; every channel but the low-frequency one heard |
| 7.1 Opus and AAC, against `-ac 2` | -223 dB | 0.200 / 0.200 | equal |
| Five streams, the mix against their sum | -220 dB | -0.001 / -0.001 | equal |

From the middle of the file (a jump), a stream coded with noise substitution (the AAC encoder replaces noisy bands with noise the decoder makes up) differs from a straight read by about -18 dB, at the same level and width: FFmpeg itself gives the same after `-ss` (the decoder's noise generator starts afresh). Tonal sound matches within 4·10⁻⁵.

Resampling (only when the file's rate and the device's differ): `swr`'s default filter is flat to 20 kHz (-0.6 dB there, -2 dB at 21 kHz); the very-high-quality resampler from the same build (`soxr`) is the same to 20 kHz. Not worth a change.

Five streams at 0 dB can go over full scale: the owner-like test file peaks at 1.2 (+1.6 dBFS) summed, 0.03 % of samples over. They are clamped at the output, as the card would clip them.

So the mix is exact; the difference had to be after it, in how the sound reaches the system.

## 4. The device

The stream was opened with the device's own mix format (cpal's default config, the system's "mix format"). With the system's spatial sound or a headset's virtual surround on, that can be eight channels (7.1): the stereo mix then went to the front left and right of a 7.1 layout and the other six were silent. A virtual surround renders 7.1 into headphones as speakers around the listener, so the front pair is placed in front and, depending on the renderer, mixed down with others at a lower level; a program that opens a stereo stream hands the system stereo. cpal also marks every stream "direct out" (no speaker positions), which a system mixer may treat differently from a stereo stream. This is the one stage left where the app's sound can differ from the established editor's, and it matches the complaint (quieter, flatter, cured by turning up). It is an inference: it can't be measured without the owner's machine, and the owner's settings (section 6) settle it.

## 5. Decisions

- **The stream is stereo.** When the device mixes more than two channels, playback opens a two-channel stream (the system converts it to the device's layout, as for any stereo program), checked by opening one first; a device that won't take it gets its own number, as before. The log says which device, rate, channels and sample format were used.
- **No protection over full scale by default.** The master's limiter stays off; over 0 dBFS the card is handed full scale (hard clipping), in playback and export alike. A safety limiter on by default would change every existing project's sound, break "export equals playback to the sample" only where it acts, and hide that a mix is too hot; the master's limiter is one click away and the meters show the peaks. A centred stereo stream at default settings plays as the file holds it, to the bit (tested through the playback path on 2-, 6- and 8-channel devices).
- **No surround output.** A surround file is folded to stereo; the timeline is stereo. Surround timelines would be a project-plan change (a channel layout per timeline, a panner, export of more channels).

## 6. For the owner to check

- `ffprobe -v error -show_entries stream=index,codec_name,channels,channel_layout,sample_rate:stream_tags=title clip.mkv`: each stream's channels and layout.
- Windows Sound settings › the output device › Properties: "Spatial sound" (off, Windows Sonic, or a vendor's), and Advanced › default format. A headset's own software may add virtual 7.1.
- Volume mixer (Settings › System › Sound › Volume mixer): the editor's own level, which the system keeps per program.
- The log's "sound output open" line (device, rate, channels, format) and "the sound device mixes N channels" line.
