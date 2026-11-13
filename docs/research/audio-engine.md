# Audio engine

Turning the timeline's tracks into the sound that comes out of the speakers and into exported files, with effects on every track and on the master: high pass, EQ with an analyser, compressor, noise clean-up, ducking, loudness, a master compressor and a true-peak limiter, and volume and pan lines. Build plan step 8.1 "DSP", and everything on the Audio page (8.4) and at Gate 8 that depends on it.

Status: measured on this cloud machine (2 cores) and chosen (section 4). The first part of section 5 is built: `crates/audio` (every effect, the master, the mixer, its controller and rings; see its README). The engine does not use it yet. The owner's machine can run the same measurement (the "Audio engine spike" workflow).

## 1. Problem

### What there is today

`engine::playback` runs one audio thread per playback. It mixes 1024-sample blocks with `engine::mix::Mixer` (decoding every clip's file, clip volume, pan and fades, speed and speed ramps tape-style, nested timelines, track mute, solo and volume, master volume) and keeps a quarter-second ring buffer full; the sound card's callback only copies from that ring. `engine::meter` measures the master (true peak, momentary, short-term and integrated loudness, from `engine::loudness`) and hands the numbers to the interface through atomics. Export mixes through the same `Mixer`.

That is safe, but nothing the audio thread does is real-time in the strict sense: decoding opens files and seeks, which can take any time, and a change to a setting is heard a quarter of a second late because the ring is that long. There are no effects.

### What is needed

| Need | Target |
| --- | --- |
| The real-time code | never allocates, frees, locks, waits or touches files; every effect takes fixed-size blocks |
| 32 tracks, every effect on each, plus the master | well inside one core of an ordinary machine, with room for decoding and the interface on the others (Gate 8: no dropouts on either test machine) |
| A setting changed on the Audio page (a fader, an EQ band dragged) | heard within a few tens of milliseconds, without clicks or zipper noise |
| Meters (per track: peak, loudness, gain reduction; master: true peak, momentary, short-term, integrated) | read by the interface without locks, as today |
| The analyser behind the EQ | a spectrum of the selected track, drawn from work done off the real-time thread |
| Preview and export | the same sound to the sample when played from the same point (export runs the same code offline) |
| Gate 8 | a noisy recording with game, mic and chat mixes to -14 LUFS integrated with no true peak over -1 dBTP |
| Licences | GPL-compatible, pure Rust preferred |

Per track, in this order: high pass (a sensible default for voice), noise clean-up, parametric EQ (with the analyser), compressor (threshold, ratio, attack, release, knee, makeup, gain-reduction meter, plain presets), ducking (turned down under dialogue), volume and pan (automation lines), meter. Master: compressor, true-peak limiter, loudness meters.

Not this system's job: decoding (`media`), the clarity check (8.2, a worker), sync by sound (8.3, a worker), loudness scans of whole files (a worker, as the Library's are now).

## 2. Approaches

### (a) How a track's effects are held and called

**A. A list of enum values.** `Vec<Effect>` where `Effect` is an enum of every kind of effect; each block, a `match` calls the right one. Any order, any number; adding an effect kind means adding a variant.

**B. A list of trait objects.** `Vec<Box<dyn Effect>>`. The most open (effects from anywhere), one indirect call per effect per block; turning an effect on means building a new box (an allocation, so off the real-time thread).

**C. One fixed strip per track.** A struct with every effect the Audio page has, as fields, always there, each switched on or off, run in a fixed order. Nothing to build when an effect is turned on; the order is the page's.

The effect code is the same in all three; only the holding and calling differ. Per-block dispatch of eight effects is tiny next to 256 samples of work each, so the expectation is no measurable difference, and the choice rests on what each makes easy.

### (b) How settings reach the real-time thread without locks

**S1. An atomic per number.** Every setting of every track is an `AtomicU32` holding an `f32`, plus a counter per track that the interface bumps after a change. Each block the real-time thread checks the counters and, for a track that changed, reads its numbers again and works out what changed. No queues; but numbers that belong together (a band's frequency and gain) can be read half old, half new, and every track is polled every block.

**S2. Pre-built effect states swapped in.** The interface side builds a whole new strip (coefficients worked out, buffers allocated) and sends it through a lock-free queue. The real-time thread carries the running state over (filter memories, envelopes, the clean-up's windows, meters), swaps it in and sends the old one back through a second queue, so it is freed on another thread. Anything can change, including what needs memory; but every slider movement builds a strip.

**S3. A queue of small commands.** Plain `Copy` messages (track, which number, value) through one lock-free queue; the real-time thread applies them at the start of a block and works out again only what changed. Cheap on both sides, in order, nothing torn; cannot carry anything that needs memory.

S2 and S3 combine naturally: commands for numbers, swaps for things that need memory (a new track, a learned noise profile, edited automation lines).

### (c) Where the real-time mix lives

**P1. As today:** one thread decodes and mixes, a long ring to the sound card. Simple; settings heard late; decoding and effects share a thread that is not real-time.

**P2. In the sound card's callback.** Decoding moves to a feeder thread that fills one lock-free ring per track ahead of time; the callback takes a block from each ring, runs the effects and the master, and writes to the card. The sound library can ask the system to run the callback's thread at the priority it gives sound (its `realtime` feature: the multimedia class scheduler on Windows, real-time scheduling on Linux; off in today's build, so today's callback runs at ordinary priority). Device buffer sizes vary, so a small adapter cuts them into fixed blocks.

**P3. A mixing thread of our own** between the per-track rings and a short ring to the card. Same as P2 but on a thread we start, which then needs its priority raised by hand on every system, and one more ring.

### (d) Noise clean-up

**Spectral gate.** The sound is cut into overlapping windows (1024 samples, a new one every 256), each turned into frequencies with an FFT; every frequency not clearly above the learned noise there is turned down (spectral subtraction with a floor), smoothly over time and across neighbouring frequencies, and the windows are added back together. Needs a noise profile: learned on a worker from a stretch the person picks, or from the clip's quietest stretches. Takes out hiss and hum under words, not only between them. Delays the sound by 1024 samples (21.3 ms at 48 kHz: a sample is finished once the last of the four windows over it is put back; the spike said 768, which was wrong). FFT from `realfft` (on `rustfft`), both MIT or Apache-2.0, pure Rust.

**Downward expander.** Below a threshold the sound is turned down further; no FFT, no profile, no delay. Only acts between words: noise under words stays, and the threshold must sit above the noise and below the quietest words.

### (e) Ducking

**Live sidechain.** A detector listens to the dialogue tracks' sound (after their effects and faders) in the speech band, decides per sample whether someone is speaking (threshold, with a hold so music doesn't pump between words), and smooths that with a down and an up speed; each ducked track turns itself down by its own depth. Dialogue tracks are processed first in each block. Always in step with edits; export gets the same.

**A pre-worked line.** A worker scans the dialogue clips and writes a volume line on the music, which the real-time thread only follows. Could look ahead and could be shown and edited, but must be worked out again after every edit, and is wrong until it is.

### (f) Block size and latency

256 or 512 samples (5.3 or 10.7 ms at 48 kHz). Smaller blocks mean settings are taken sooner and each block's worst case is smaller; larger ones cost less per sample in per-block overhead. The clean-up's hop (256) sets a natural size: a 256-sample block does exactly one window per channel every time, so every block costs the same.

### Sources

- Robert Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter coefficients": the high-pass, shelf and peak filters.
- The international broadcast loudness recommendation and the loudness rule built on it (already followed by `engine::loudness`): K-weighting, gating, true peak by four-times oversampling.
- Steven Boll, "Suppression of acoustic noise in speech using spectral subtraction" (1979): the spectral gate's gain.
- Ross Bencina, "Real-time audio programming 101: time waits for nothing": why the real-time thread may not allocate, lock or wait.
- The `rtrb` crate (already used by `engine::playback`): a wait-free single-producer single-consumer ring.

## 3. Prototypes

`spikes/audio-engine/` is one program with no sound card and no files: it makes its own input, so it runs the same everywhere (the "Audio engine spike" workflow builds it for Windows).

- **Input:** eight 10 s stereo signals at 48 kHz, two of each kind: speech-like (a buzz at a wandering pitch through moving resonances, in syllables and phrases with pauses, over hiss and a 50 Hz hum), music (chords and a kick drum), effects (bursts of noise, some near full scale) and ambience (rumbling noise). 32 tracks read them at different offsets: 8 dialogue, 8 music, 8 effects, 8 ambience.
- **The full chain on every track:** 24 dB/octave high pass, clean-up (spectral gate, or expander), six-band EQ, compressor, ducking (on the 24 tracks that aren't dialogue, keyed by the 8 that are), volume and pan lines with a point every half second, a meter (peak, K-weighted momentary loudness), and an analyser tap on one track feeding a worker that does 2048-point FFTs. **Master:** compressor, true-peak limiter at -1 dBTP (64 samples of look-ahead, four-times oversampled peaks), meters.
- **Designs A, B and C** run the same effect code in the same order; their outputs are checked to be identical to the bit.
- **Settings S1, S2 and S3** each drive design C's strips (boxed, so S2 can swap them).
- **Allocations:** a global allocator counts every allocation and free made on the thread while it is marked as the real-time thread. Every measured run reports the count.
- **Timing:** each block by the clock, and by the thread's own processor time (Linux), which leaves out time the thread was not running. Five runs per design, the designs taking turns.
- **Paced runs:** the real-time thread sleeps until each block is due and mixes all 32 tracks, while another thread changes one setting 400 times at random moments; with and without a busy thread spinning on every core, and with the mixing thread's priority raised.
- **Checks** (`cargo test`): designs identical and allocation-free; the limiter holds -1 dBTP on loud noise; the compressor's steady gain reduction matches its ratio (10.5 dB for 14 dB over at 4:1); the gate lowers pauses and keeps words; every settings way delivers a change without allocating.

## 4. Result

Measured on this cloud machine (2 cores), release build. Times in µs per block; a 256-sample block lasts 5333 µs, a 512 one 10 667 µs.

### A, B and C, every effect on every track

| Block | Design | Clean-up | Mean | Median | 99th | Worst (clock) | Worst (own time) | Mean / block | Allocations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 256 | A enum | gate | 772 | 742 | 4768 | 5845 | 1301 | 14.5 % | 0 |
| 256 | B boxed trait | gate | 768 | 722 | 4106 | 9065 | 1160 | 14.4 % | 0 |
| 256 | C fixed strip | gate | 807 | 726 | 4792 | 8837 | 1453 | 15.1 % | 0 |
| 256 | C fixed strip | expander | 688 | 637 | 4687 | 11 028 | 1213 | 12.9 % | 0 |
| 512 | A enum | gate | 1630 | 1444 | 5590 | 9562 | 2877 | 15.3 % | 0 |
| 512 | B boxed trait | gate | 1547 | 1444 | 5543 | 17 903 | 2205 | 14.5 % | 0 |
| 512 | C fixed strip | gate | 1492 | 1476 | 5526 | 16 468 | 2481 | 14.0 % | 0 |
| 512 | C fixed strip | expander | 1322 | 1300 | 5410 | 9385 | 2214 | 12.4 % | 0 |

- The three designs are the same within the machine's noise (a few per cent between runs, the order changing from run to run) and their output is identical to the bit. Holding effects one way or another costs nothing measurable.
- **32 tracks with everything on take about 15 % of one core**, at either block size. Mixing on one thread is plenty; splitting tracks across threads is not needed.
- The worst block by the thread's own processor time is 1.2 to 1.5 ms at 256 (a quarter of the block) and 2.2 to 2.9 ms at 512. The worst by the clock (up to 18 ms) and the 99th (about 4.5 ms, in every row, even the cheap ones below) are this machine taking the thread away, not work: the reason the real-time thread needs the priority the system gives sound (below).
- Master after the limiter: highest true peak -2.1 dBTP with the mix at about -19 LUFS; the limiter test drives it with noise at +6 dBFS and it holds -1 dBTP.

What each effect costs, alone on all 32 tracks (design C, 256, mean; the first row is the floor every other row includes):

| On every track | Mean | Mean / block |
| --- | --- | --- |
| nothing (reading, fader lines, meter, sum), master off | 150 | 2.8 % |
| nothing, master on (compressor, limiter, meters) | 117 | 2.2 % |
| high pass, 24 dB/octave | 199 | 3.7 % |
| EQ, 6 bands | 375 | 7.0 % |
| compressor | 252 | 4.7 % |
| clean-up: spectral gate | 334 | 6.3 % |
| clean-up: expander | 240 | 4.5 % |
| ducking (24 tracks, key from 8) | 121 | 2.3 % |
| everything, gate | 805 | 15.1 % |

The first two rows are within the machine's noise of each other. The EQ is the largest part (six filters per channel per track); the spectral gate costs about 0.1 ms more per block than the expander for all 32 tracks.

### Clean-up: gate against expander

Speech over steady noise; how far the sound goes down well inside pauses (more is better) and well inside words (less is better):

| Noise peaks | Method | Pauses down | Words down |
| --- | --- | --- | --- |
| -50 dBFS | spectral gate | 13.3 dB | 0.05 dB |
| -50 dBFS | expander | 5.4 dB | 0.00 dB |
| -34 dBFS | spectral gate | 14.3 dB | 0.83 dB |
| -34 dBFS | expander | 5.7 dB | 0.00 dB |

The expander's threshold was set 6 dB over the noise's peaks, as a person would; it lets the hiss through while it rises and falls between syllables. The gate takes out more than twice as much in decibels, and works under words too (not measured here; the expander cannot by its nature). A first version of the gate, compared with the noise's mean magnitude rather than its mean power and opening instantly, only managed 9 dB: noise often rises above its mean magnitude, and an instant opening lets each such moment through for a whole release.

### S1, S2 and S3: what a change costs

Each block (256), the interface side changes the third EQ band's frequency of one track or of all 32, then the real-time side takes it and mixes the block:

| Way | Changes | Real-time mean | Interface per change | Real-time allocations |
| --- | --- | --- | --- | --- |
| S1 atomics | none | 794 | – | 0 |
| S1 atomics | 1 track every block | 835 | 0.1 | 0 |
| S1 atomics | 32 tracks every block | 862 | 0.0 | 0 |
| S2 swap | none | 765 | – | 0 |
| S2 swap | 1 track every block | 881 | 27.1 | 0 |
| S2 swap | 32 tracks every block | 1086 | 29.5 | 0 |
| S3 commands | none | 922 | – | 0 |
| S3 commands | 1 track every block | 973 | 0.2 | 0 |
| S3 commands | 32 tracks every block | 949 | 0.0 | 0 |

- Taking a change costs the real-time side next to nothing with S1 and S3 (inside the noise between rows). S2 costs about 10 µs per swapped strip on the real-time side (carrying the clean-up's windows over) and 27 to 30 µs per change on the interface side (building a strip, planning its FFTs). Fine for rare changes, wasteful for a dragged slider.
- **S2 is easy to get wrong.** Its first version freed memory on the real-time thread: carrying the meter's readout over by assignment dropped the new strip's own readout there. The counting allocator caught it at once (the fix swaps them, so the unused one goes back with the old strip). Any build of S2 needs that allocator in its tests.

### Paced in real time

How long from a change to the first block that uses it, and how late the thread wakes up for each block (ms):

| Way | Block | Busy cores | Priority | Change: median | 99th | worst | Late: median | 99th | worst | Blocks over by one | over by four |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| S1 atomics | 256 | 0 | ordinary | 2.15 | 5.27 | 5.34 | 0.09 | 1.32 | 3.95 | 0 / 562 | 0 |
| S1 atomics | 512 | 0 | ordinary | 2.96 | 9.14 | 10.01 | 0.10 | 3.60 | 5.34 | 0 / 281 | 0 |
| S2 swap | 256 | 0 | ordinary | 2.08 | 5.20 | 5.36 | 0.10 | 3.36 | 3.38 | 0 / 562 | 0 |
| S2 swap | 512 | 0 | ordinary | 3.09 | 9.47 | 9.65 | 0.09 | 0.74 | 3.98 | 0 / 281 | 0 |
| S3 commands | 256 | 0 | ordinary | 2.37 | 5.16 | 6.74 | 0.09 | 2.64 | 3.98 | 0 / 562 | 0 |
| S3 commands | 512 | 0 | ordinary | 2.80 | 8.88 | 10.08 | 0.10 | 3.14 | 5.84 | 0 / 281 | 0 |
| S3 commands | 256 | 2 | ordinary | 1.80 | 6.74 | 7.49 | 0.06 | 3.88 | 7.92 | 4 / 562 | 0 |
| S3 commands | 256 | 2 | raised | 2.19 | 5.17 | 5.24 | 0.02 | 0.06 | 0.06 | 0 / 562 | 0 |
| S3 commands | 512 | 2 | ordinary | 1.78 | 9.00 | 9.07 | 2.58 | 7.25 | 8.76 | 3 / 281 | 0 |
| S3 commands | 512 | 2 | raised | 3.01 | 9.17 | 9.36 | 0.01 | 0.03 | 0.04 | 0 / 281 | 0 |

- **A change is taken within one block, whichever way**: half a block on average, a block at worst (5.3 ms at 256, 10.7 at 512). The way settings travel decides nothing about latency; the block size does. What is heard adds the device's buffer (typically 10 to 20 ms), against 250 ms today.
- **Priority decides dropouts.** With every core busy at ordinary priority, the thread woke up to 8 ms late and 4 blocks in 562 would have dropped out with one block of buffer. An earlier run on the same machine was worse: 22.7 ms late at worst and 174 of 562 blocks over (65 of 281 at 512). With the priority raised, the worst wake-up was 0.06 ms under the same load.
- No allocation on the real-time thread in any run.

### The choice

- **(a) C, a fixed strip per track.** Dispatch costs the same every way, so the choice is about structure: the Audio page has a fixed set of tools in a fixed order (EQ, Compressor, Clean-up, Ducking, Loudness), and a strip that always holds every effect means turning one on or off is a flag, never an allocation, so it travels as a plain command. Its state is in one place, which makes carrying it over (S2) and resetting it simple. When the Effects library brings audio effects (phase 9), they go into a short list of inserts after the strip's EQ, held as design A (an enum), the cheapest open-ended way.
- **(b) S3 commands for numbers, S2 swaps for what needs memory; atomics only outwards.** Commands cost 0.1 to 0.2 µs on the interface and nothing measurable on the real-time side, keep numbers that belong together together and in order, and carry on/off, mute and solo too. New tracks, a learned noise profile and edited automation lines need memory: those are built on the interface side (or a worker) and swapped in, the old ones sent back to be freed off the thread. S1 is dropped: it polls every number of every track every block and can read a band half changed. Meters and gain reduction go out through atomics, as `engine::meter` already does.
- **(c) P2, the mix in the sound card's callback; decoding on a feeder thread into one ring per track.** The paced runs show that priority is what keeps blocks on time when the machine is busy. The sound library already knows how to ask for it for its callback thread on each system (its `realtime` feature, to be turned on; its helper crate is MPL-2.0, which `deny.toml` allows), so there is no operating-system code of our own; a thread of our own (P3) would need that code and one more ring. The 32-track mix with every effect is a quarter of a block at worst, so the callback has the room. Decoding, seeking, clip volume and fades, speed, ramps and nested timelines stay where they can wait: in today's `Mixer` logic, now per track, on a feeder thread running half a second ahead.
- **(d) The spectral gate,** with a learned profile: 13 to 14 dB of noise out of pauses against 5 to 6 for the expander, words left within 1 dB, for 0.1 ms more per block across 32 tracks; `realfft` and `rustfft` are MIT or Apache-2.0, pure Rust. The expander is not built.
- **(e) Live sidechain ducking:** the speech band of the dialogue tracks' sound after their effects, threshold, hold, down and up speeds, a depth per ducked track; 2.3 % of a block for 24 ducked tracks. Always right after an edit and the same in export. A worked-out line that can be looked at stays a possible later addition.
- **(f) 256-sample blocks** at the device's own rate (5.3 ms at 48 kHz): changes taken within 5.3 ms, smaller worst blocks, the same cost per sample as 512, and exactly one clean-up window per block so every block costs the same. The engine's own delay is fixed (the gate's 1024 samples on every track, its windows passed through untouched when clean-up is off, plus the limiter's 87): 1111 frames, 23.1 ms at 48 kHz, taken off the clock so the picture stays in step.

What the measurement did not cover: a real sound card (buffer sizes, the callback's own timing and priority on Windows), real decoding competing for the cores, and the owner's machines. Run the spike there before Gate 8.

## 5. The build

### `crates/audio`

As built, the names moved a little: the graph is `mixer.rs` (`Mixer`), the true peak stays in `dsp/loudness.rs` beside the meter that uses it, the Loudness tool is `dsp/level.rs`, and a track's meter is `meter.rs`. The crate's README is the current map; the table below is the plan as written.

Pure Rust, no sound card, no files: everything here runs the same live and offline, and is tested without a device.

| Module | What |
| --- | --- |
| `dsp/biquad.rs` | Second-order filters (cookbook shapes, K-weighting), one state per channel |
| `dsp/filters.rs` | High pass (24 dB/octave) and the six-band EQ |
| `dsp/dynamics.rs` | Compressor (static curve with knee, attack, release, makeup, gain reduction) |
| `dsp/cleanup.rs` | The spectral gate; `learn` for a noise profile (worker side) |
| `dsp/duck.rs` | The ducking key and the ducker |
| `dsp/limiter.rs`, `dsp/true_peak.rs` | The master's true-peak limiter and four-times oversampled peaks |
| `dsp/loudness.rs` | Moved from `engine::loudness` (engine keeps a re-export until export moves over) |
| `dsp/lines.rs` | Volume and pan lines, worked out per sample on a fixed grid from the timeline's start, so where a block starts never changes the sound |
| `strip.rs` | `Strip`: every effect of a track in the page's order, each on or off; `take_state` for swaps |
| `master.rs` | Master compressor, limiter, meters |
| `graph.rs` | `Graph`: dialogue tracks first, the ducking key, the other tracks, the master; `process(block)` takes each track's block and writes the mix |
| `settings.rs` | `TrackSound`, `MasterSound`: settings as plain values, built from the project |
| `control.rs` | `Command` (track, setting, value; on/off; mute; solo), `Controller` (the interface's end), the swap queue and its return queue, the collector that frees what comes back |
| `feed.rs` | One lock-free ring per track; the reading end counts samples it had to fill with silence and tells the feeder to skip them, so a late file never drifts |
| `readout.rs` | Meters and gain reduction out through atomics (from `engine::meter`) |
| `analyser.rs` | The tap on the selected track and the worker that turns it into the EQ's spectrum |
| `block.rs` | The fixed-block adapter: any device buffer size in, 256-sample blocks through the graph |

Denormals are flushed to zero at the start of each callback (`os::denormals_as_zero`, a few lines in `crates/os`, which may use `unsafe`); without it, filters and envelopes dying away get very slow on some processors.

### How `engine` uses it

- `engine::playback`: cpal's `realtime` feature is turned on, so the callback runs at sound's priority (logged when the system refuses). The audio thread becomes the feeder. It builds the `Graph` (allocating) before playback, then decodes per track with today's `Mixer` logic (clip volume, pan, fades, speed, ramps, nested timelines, each nested timeline mixed through a `Graph` of its own, offline) into the tracks' rings, about half a second ahead. The cpal callback owns the `Graph` and the adapter, reads the rings, mixes and meters. The clock counts samples played, minus the engine's fixed delay; it starts once every ring holds a first block, as today.
- Track volume, mute and solo leave `Mixer` and become strip settings sent as commands, so they are heard at once while playing.
- The Audio page (8.4) holds a `Controller`: each slider movement is a command; learning a noise profile runs on a worker and its result is swapped in.
- Export (phase 6) runs the same feeder and `Graph` on its own thread, block after block, without a device. Not touched now: export is being worked on separately.
- The project gains per-track sound settings and volume and pan lines (a new format version with its migration and stored sample). Clip lines are kept in the clip's file time, like its keyframes; track lines in timeline time. A noise profile is stored as levels in 64 bands by frequency, so it is small and works at any rate.

### Tests

- Each effect against worked-out values: filter responses at chosen frequencies against their formulas; the compressor's static curve and its steady gain reduction; the limiter on loud noise, tones and the standard's true-peak tones, never over its ceiling; the gate on speech over noise (pauses down at least 12 dB, words within 1 dB); ducking depth and timing; loudness as now.
- No allocation: a counting global allocator in `crates/audio`'s tests runs 32 tracks with every effect, commands every block and swaps, and asserts zero allocations and frees on the processing thread.
- Same sound however it is cut: the same input through device buffers of 128, 441, 480 and 1024 samples is identical to the bit; playback from the start equals export from the start.
- Stress: 32 tracks with every effect for 10 s; an ignored test run by hand prints the times (as `engine/tests/jumps.rs` does), and the log counts blocks that came late or rings that ran dry while playing, for Gate 8.
- Gate 8's mix to -14 LUFS with no true peak over -1 dBTP becomes a test on a recording with game, mic and chat streams.
