# spikes/highlights

Compares ways of finding highlights in long game recordings, from [docs/research/highlights.md](../../docs/research/highlights.md), on the same recordings, against moments picked by a person. Throwaway code: deleted once the choice is built.

| File | What |
| --- | --- |
| `src/media.rs` | FFmpeg reading: every audio stream in one pass, turned into mono at 16 kHz; the video turned into tiny pictures (64×36 brightness, 32×18 colour) on the CPU or the GPU's decoder, every frame, every frame without deblocking, or key frames only |
| `src/audio.rs` | Sound features every 10 ms: loudness, and spectral flux (how much new sound appeared) |
| `src/video.rs` | Picture features every decoded frame: difference from the frame before, and colour histogram distance |
| `src/detect.rs` | The approaches: A loudness, B flux with the streams combined, C cuts and sudden motion; each at three sensitivities; merging moments |
| `src/score.rs` | Reads picked moments, matches found to picked within a window, precision, recall, F1, false positives per hour |
| `src/synth.rs` | Makes synthetic test clips with events at known times, plus decoys, and their ground truth |
| `src/main.rs` | Runs everything on every recording and prints Markdown tables |
| `make-clips.sh` | Makes the synthetic clips (Linux) |
| `run.ps1`, `run.bat` | Windows: makes the clips, measures them and your recordings, writes `results.md` |

## What it measures

For every recording: the time of each pass through the file (the sound of every stream together; the picture once per decoding choice), as a multiple of real time; and for each approach at low, medium and high sensitivity, the moments found. With a list of picked moments beside the recording it scores them: a found moment within ±1 s of a picked one is a hit (each picked moment takes at most one), everything else found is a false positive. The last tables add up every scored recording, with recall per kind of moment when the list says what each moment is, and the same at ±3 s.

| Approach | What it does |
| --- | --- |
| A loudness | Each stream on its own: a moment where the last 200 ms are at least 10 dB louder than the stream usually is (its talking level, or the louder three quarters of the 30 s around) |
| B flux, combined | Sudden new sound on each stream, measured against how busy that stream usually is; onsets on several streams within half a second are one event, the mic counting most (1, game 0.6, chat 0.5) |
| C video, every frame | Cuts (colours change at once, or the whole picture jumps in one frame with calm frames around it) and sudden motion (0.2 s of picture change 2.5 times what is usual for the 20 s around) |
| C key frames, then the stretch | Only key frames decoded; where two neighbouring key frames differ in colour, that stretch is decoded to find the cut. No motion |
| A + C, B + C, A + C key frames | The two put together; moments within 1.5 s are one |

The moments found at medium sensitivity go to `found/<recording>.<approach>.csv`, to compare by eye with the recording.

## Running it

**Windows:** run the "Highlights spike" workflow in GitHub Actions, download the `highlight-bench-windows` artifact and unzip it. Put your recordings in `recordings`, each with its list of picked moments beside it (see below), and double-click `run.bat`. It makes three synthetic clips the first time (a few minutes), then measures; about 10 to 15 minutes per half hour of recording. It writes `results.md` and the `found` folder. Nothing leaves the PC.

**The list of picked moments** is a text file with the recording's name and `.csv`: `recordings\match-04.mkv` gets `recordings\match-04.csv`. One moment per line, the time as the player shows it, then optionally what it is:

```text
time,kind
1:42,mic
3:05,game
12:30,cut
1:02:10,motion
```

Times can be seconds (`102.5`), minutes and seconds (`1:42`) or hours, minutes and seconds (`1:02:10`). Kinds are free words; `mic`, `game`, `chat`, `cut` and `motion` line up with the synthetic clips. Separate with commas, semicolons or tabs, so a spreadsheet saved as CSV works.

**Linux:** install FFmpeg 8.1 development files (or unpack the build pinned in `packaging/ffmpeg.env` and set `FFMPEG_DIR` to it), put an `ffmpeg` program on the `PATH`, then:

```sh
./make-clips.sh clips            # H264=1 or SMALL=1 (720p) for clips that are quicker to make
cargo run --release -- clips/*.mkv > results.md
```

`cargo test --release` checks the features, detectors, scoring and clip plans.

Options: `--decode software` or an FFmpeg device type (`d3d11va`, `vaapi`; the default is `d3d11va` on Windows, `software` elsewhere; give it twice to measure both), `--frames all,keys,nodeblock` (which video passes; default `all,keys`), `--threads N` (decoder threads, default all), `--roles game,mic,chat` (what each audio stream carries, in order, when the stream titles don't say), `--window SECONDS` (1), `--limit SECONDS` (only the start of each recording), `--found FOLDER`.

Two more commands: `highlight-bench make-clips FOLDER [--seconds N] [--h264 | --small] [--only NAME]` makes the synthetic clips (`arena`, `racing`, `calm`) and their `.csv` ground truth, and `highlight-bench score FOUND.csv PICKED.csv [--window SECONDS]` scores one list against another.
