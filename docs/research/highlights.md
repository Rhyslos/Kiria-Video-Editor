# Highlight finder

How the editor finds the moments worth keeping in a long game recording and suggests timeline labels there. Build plan step 5.4 "Highlight finder"; the project plan's "scans long recordings for loud or busy moments (mic shouts, game-audio spikes, scene changes) and suggests timeline labels there. Works for any game because it only listens and looks."

## 1. Problem

**What a highlight is.** In a game recording the moments a person cuts out to keep are the ones where something happened and someone reacted. Each leaves a mark the editor can find without knowing the game:

| Where | What it sounds or looks like | What is usually there besides |
| --- | --- | --- |
| The mic | a shout, a laugh, a swear, talking suddenly much louder and higher | ordinary talking, long silences, coughs, keyboard and mouse clicks, breathing |
| The game sound | an explosion, a kill or death sound, a crash, a round-end sting | music, steady gunfire, engines, footsteps, all of it loud for long stretches |
| Voice chat | teammates shouting or laughing | teammates talking |
| The picture, cuts | a death screen, a kill camera, a replay, a menu, a map change | none in a normal stretch of play; a recording is one continuous capture |
| The picture, motion | a whip turn, the view shaking after a hit, a flash | the camera moving all the time, at the game's usual pace |

The mic counts most: a shout or a laugh is the player saying "this one". Research on sports broadcasts found the same with the commentator's excited voice, there the strongest single cue to a highlight (see Sources).

**The recordings.** 1440p 60 H.265 in MKV from a GPU recorder, key frames every 2 seconds (some recorders every 5 to 10), no B frames, 30 minutes to several hours long, with two or three audio streams: the game, the mic, and sometimes voice chat. Some recorders name the streams, some don't, and some put a mix of everything on the first.

**Targets.**

| Need | Target |
| --- | --- |
| Speed | in the background, faster than real time with room to spare: at least 10 times on the 24-thread desktop with the analysis pool's share of it, so an hour of recording is done in 6 minutes; paused while the timeline plays, like proxies |
| Found | at least the moments both people would pick in a 30-minute test recording (Gate 5): recall of 90 % or more on picked moments |
| False positives | counted, per hour of recording. At medium sensitivity at least half the suggestions should be right and no more than about 30 wrong ones an hour (one every 2 minutes), so going through them is quicker than watching |
| Timing | within a second of the moment; a label marks where it starts and the person trims around it |
| Sensitivity | one setting (low, medium, high) that moves predictably between fewer, surer suggestions and more of them |
| Any game | no training per game, nothing that reads a particular game's screen |
| Memory | features only (a few megabytes an hour), never the sound or pictures themselves |
| Interface thread | nothing; results arrive as faded suggested labels (build plan 5.4) |

## 2. Approaches

### A. Loudness per stream, against its usual level

For every audio stream on its own: the loudness of the last 200 ms, every 50 ms, compared with what is usual for that stream. A moment starts where it rises 10 dB or more above usual and stays there for at least 100 ms; a second rise within 2 s belongs to the same moment.

- "Usual" is the hard part. A mic is silent half the time, so the average loudness makes every word look loud. The spike uses the larger of two levels: the stream's talking level (the middle loudness of the stretches well above its quietest tenth), and the louder three quarters of the 30 s around the moment (so a loud match doesn't light up as a whole).
- This is short-term against long-term loudness, the same idea as a broadcast loudness meter's momentary and short-term readings, and the energy part of classic onset detection (Bello and others).
- Sensitivity is the dB margin: 14, 10 and 7 dB for low, medium and high.
- Cost: the audio must be decoded, which is cheap (AAC or Opus, a few streams), but the file must be read through: MKV interleaves sound with the picture, so the demuxer reads every byte of video to find the sound.

### B. Sudden new sound (spectral flux) per stream, combined

For every stream: the spectral flux, how much of every frequency appeared since 10 ms before, summed over 100 ms; compared with what is usual for that stream (median and spread over 30 s); peaks 3 spreads above the median are onsets. Onsets on different streams within half a second are one event, scored by adding each stream's strength weighted by what it carries: mic 1, game 0.6, chat 0.5. Sensitivity is the score an event needs: 7, 4 and 2.

- Flux finds where sound starts, even when it is no louder overall: an explosion over loud music, a shout over game sound bleeding into the mic. It is the standard onset detector in music analysis (Bello and others; Dixon).
- Combining streams rewards a moment where the player shouts *and* the game explodes, which is what a highlight usually is, and the weights put the mic first.
- Cost: as A, plus a 512-point FFT every 10 ms per stream; still well under 1 % of a core.

### C. The picture: cuts and sudden motion

Every frame (or every key frame) shrunk to 64×36 brightness and 32×18 colour, straight from the decoder's planes. Two numbers per frame against the frame before: the colour histogram distance (0 to 1) and the mean brightness difference.

- **Cut:** the histogram distance jumps (0.25 or more and three times the second around it), or the whole picture changes in one frame with calm frames on either side (a cut between two views of one place, which keeps its colours). This is the classic shot-boundary detection (Zhang, Kankanhalli and Smoliar; Hanjalic) and what FFmpeg's scene score does.
- **Sudden motion:** the brightness difference over 0.2 s rises to 2.5 times what is usual for the 20 s around it. Games move all the time, so only a sudden jump in movement counts.
- Sensitivity scales the thresholds.

How to get the pictures is most of the cost. FFmpeg's H.264 and H.265 decoders can't decode at a lower resolution (the caching note found the same), so the choices are:

1. **Every frame**, on the CPU or on the GPU's decoder with each frame copied back. The full decoding cost: about what playing the file costs, without the drawing.
2. **Every frame without deblocking** (`skip_loop_filter`): a little faster on the CPU, pictures slightly wrong, which tiny pictures don't notice. GPU decoders ignore it.
3. **Key frames only**: decode only the key frames (one in 120 at 2 s), find neighbouring pairs whose colours differ, and decode only those stretches frame by frame to find the cut. Very cheap; finds cuts to a different-looking picture, and motion only where it changes the colours over the stretch.
4. **Alongside the proxy build.** Building a proxy already decodes every frame of the original (caching note, approach C). Shrinking each frame to 64×36 as well costs almost nothing. Not measured separately here: it is approach 1 with the decode already paid for.

### Combinations

A or B (sound) put together with C (picture): moments from either within 1.5 s are one suggestion, at the earliest time, marked with everything that found it ("mic+cut").

### Helpers every approach builds on

- **Which stream is which.** By the stream's title when it has one ("mic", "voice", "chat", "game", "desktop"), else by order (game, mic, chat), as recorders usually write them, with a setting to correct it per recording. A stream that is a mix of everything works like a game stream but makes mic shouts harder to see.
- **One pass for the sound.** All audio streams are decoded in the same pass through the file, at 16 kHz mono; only features are kept (two numbers per 10 ms per stream).
- **A label at the start.** A moment's time is where it starts, not its peak, so the label lands where the cut would begin.

### Considered and left out

- **A learned sound classifier.** Small networks trained on a large public collection of labelled sound clips (527 kinds, among them laughter, screaming, explosion and gunshot; Kong and others) could tell a laugh from a cough. Left for later, as a second stage that only scores the candidates A or B found (a few seconds each, so cheap). Two things to settle first: the licence of the trained weights (the code of the best-known set is MIT-licensed, but the training clips come from a video site, and weights need a licence that allows shipping them in a GPL program), and the size (4 to 80 million parameters). It would run through the same ONNX runtime planned for background removal.
- **Reading the game's screen** (kill feeds, score counters, health bars): accurate, but needs work per game and breaks "works for any game".
- **Speech recognition** for words like "no way": heavy, depends on the language, and the loudness of the shout already says most of it.
- **Viewer chat and stream platform signals**, used by work on highlights of live streams: local recordings have none.
- **The mic's pitch.** Excited speech goes up in pitch as well as loudness (Rui, Gupta and Acero). A cheap addition to A for the mic if coughs and clicks turn out to be the false positives that matter.
- **Motion vectors from the decoder** instead of frame differences: only the CPU decoder can export them, and they cost as much as decoding; frame difference at 64×36 was tried first (the result says where it falls short).
- **FFmpeg's scene filter** on its own: the same measure as C's histogram, but a separate pass that can't share the decode with motion or a proxy build.

## 3. Prototypes

One program in `spikes/highlights/` reads a recording with FFmpeg 8.1 (the same bindings as the other spikes), runs every approach at three sensitivities and scores what it finds against a list of moments. It decodes the picture on the CPU, on the GPU with frames copied back, without deblocking, or key frames only.

**Clips with known moments.** Real recordings need a person to pick moments, and two people pick differently. To have exact ground truth too, `highlight-bench make-clips` makes synthetic recordings shaped like game recordings, from plans in `src/synth.rs`, so Linux and Windows make the same ones:

- **Sound**, made sample by sample: a game bed (music with occasional single shots, a loud engine, or quiet ambience), someone talking on the mic in spurts with pauses, and on two clips voice chat talking too.
- **Picture**, made by the `ffmpeg` program from its test sources: scenes from different sources with grain, seen through a slowly drifting window, encoded like a recorder (fixed key frame distance, no B frames).
- **Events**, in the ground truth: shouts and laughs on the mic (about 20 dB over talking), explosions in the game sound, laughs on the chat, hard cuts to another scene, cuts to another view of the same scene (same colours), whip pans (most of a screen in 0.4 s), and explosions with a whip pan together.
- **Decoys**, not in the ground truth: a cough on the mic (about 10 dB over talking), two seconds of steady gunfire, the music or engine swelling by 6 dB over 3 s, a slow pan over 3 s.
- Events and decoys are 6 to 11 s apart, each kind as often as the clip's plan says.

| Clip | Game sound | Streams | Key frames | Mostly |
| --- | --- | --- | --- | --- |
| arena | music and single shots | game, mic, chat, named | 2 s | shouts, explosions with and without whip pans, gunfire decoys |
| racing | loud engine | game, mic, named | 8 s | explosions (crashes), cuts, engine swells |
| calm | quiet ambience | game, mic, chat, not named | 2 s | laughs on mic and chat, cuts, slow pans |

**Scoring.** A found moment within ±1 s of a picked one is a hit; each picked moment takes at most one found moment, closest pairs first; everything else found is a false positive. Precision is hits over found, recall hits over picked, F1 their harmonic mean. False positives are also given per hour of recording. Recall is split by the kind of moment where the list says what each is. For moments marked by hand a ±3 s window is given too, since a person notes the time a little after it happens.

**Speed** is the length of the recording over the wall time of the passes an approach needs (sound, picture, or both one after the other), so "20×" means an hour in 3 minutes.

**Limits of the spike.** The thresholds were set by reasoning, then checked on the synthetic clips; they were not tuned on them, but the clips were made knowing what the detectors look for, so the synthetic scores are an upper bound. Real game sound is far busier than the synthetic beds. The passes run one after the other on one thread each besides the decoder's; the editor would run the sound pass and the picture pass at the same time.

## 4. Result

**Synthetic clips on a 2-thread cloud machine** (no GPU; FFmpeg 8.1 libraries; software decoding). Three clips of 4 minutes, 1440p 60, with 61 planted moments and 19 decoys. The long clips are H.264, because making H.265 at 1440p ran at a fifteenth of real time on this machine; one minute of the arena clip was also made in H.265 for the decoding speed. The machine was shared with other work during the runs, so speeds move by a fifth from run to run; what holds is the order of magnitude between passes.

**What each approach found**, all clips together, medium sensitivity, ±1 s; recall per kind of moment:

| Approach | Precision | Recall | F1 | False per hour | mic | game | chat | cut | motion | game+motion | Speed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A loudness | 100 % | 64 % | 0.78 | 0 | 20/20 | 9/9 | 5/5 | 0/15 | 0/7 | 5/5 | 86× |
| B flux, combined | 58 % | 25 % | 0.34 | 55 | 2/20 | 8/9 | 1/5 | 0/15 | 0/7 | 4/5 | 86× |
| C every frame | 90 % | 31 % | 0.46 | 10 | 0/20 | 0/9 | 0/5 | 15/15 | 3/7 | 1/5 | 4.7× |
| C every frame, no deblocking | 90 % | 31 % | 0.46 | 10 | 0/20 | 0/9 | 0/5 | 15/15 | 3/7 | 1/5 | 5.0× |
| C key frames, then the stretch | 100 % | 30 % | 0.46 | 0 | 0/20 | 0/9 | 0/5 | 13/15 | 4/7 | 1/5 | 18× |
| **A + C** | 97 % | 93 % | **0.95** | 10 | 20/20 | 9/9 | 5/5 | 15/15 | 3/7 | 5/5 | 4.4× |
| B + C | 71 % | 52 % | 0.60 | 65 | 2/20 | 8/9 | 1/5 | 15/15 | 2/7 | 4/5 | 4.4× |
| **A + C key frames** | 100 % | 92 % | **0.96** | 0 | 20/20 | 9/9 | 5/5 | 13/15 | 4/7 | 5/5 | 15× |

**Sensitivity** (the same clips; F1 at ±3 s in the last column):

| Approach | Sensitivity | Precision | Recall | False per hour | F1 | F1 ±3 s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| A loudness | low / medium / high | 100 / 100 / 98 % | 52 / 64 / 64 % | 0 / 0 / 5 | 0.69 / 0.78 / 0.77 | the same |
| B flux, combined | low / medium / high | 86 / 58 / 27 % | 10 / 25 / 31 % | 5 / 55 / 260 | 0.18 / 0.34 / 0.29 | 0.21 / 0.41 / 0.47 |
| A + C | low / medium / high | 96 / 97 / 93 % | 75 / 93 / 93 % | 10 / 10 / 20 | 0.84 / 0.95 / 0.93 | the same |
| A + C key frames | low / medium / high | 100 / 100 / 87 % | 67 / 92 / 100 % | 0 / 0 / 45 | 0.80 / 0.96 / 0.93 | the same |

**Speed of each pass**, as a multiple of real time:

| Pass | H.264 1440p 60 | H.265 1440p 60 |
| --- | ---: | ---: |
| sound, all streams in one pass | 71 to 101× | 112× |
| picture, every frame | 4.6 to 4.8× | 1.1× |
| picture, every frame, no deblocking | 4.6 to 5.4× | 1.2× |
| picture, key frames only | 190 to 525× | 102× |
| key frames, then the stretches with a cut (2 s key frames) | 35 to 37× | 21× |
| the same with key frames 8 s apart | 9.4× | |

What this says:

- **The mic is the signal, and plain loudness reads it.** A found every shout and laugh on the mic and the chat and every explosion, with no false positives at medium and high: the coughs (about 10 dB over talking) only start to show at high. Its 64 % recall is all cuts and motion, which sound can't see.
- **Spectral flux on its own stream fails on the mic.** A mic alternates between talking and silence, so its flux swings 30 dB all the time, and a robust spread measured over that swing hides the shouts (2 of 20 found). It does find game sounds (8 of 9), but no better than A, with 55 false positives an hour where A has none. Gating it to the stretches with sound would turn it into A with extra steps. B is out.
- **The picture adds what sound can't hear**: all 15 cuts, including the 4 cuts between two views of one scene, which keep their colours and are caught by the one-frame jump with calm frames around it.
- **Sudden motion is the weak part.** Only 3 or 4 of 7 whip pans were found. Frame difference measures how much the picture changes, not how fast it moves, and over smooth surfaces (gradients, large flat areas) a whip pan changes little. In a first version of the clips one scene of fine, even texture lasted three and a half minutes, and its pans were invisible altogether: at 64×36 fine detail averages to flat grey. Real games are mostly in between. The only false positives of A + C were motion in the first 6 s of two clips, where there is little "usual" to compare with yet. A measure of movement itself, the shift between two tiny pictures found by matching (one number per frame, still cheap), is the next thing to try if real recordings miss turns and shakes.
- **Key frames, then the stretch, is nearly as good and 4 to 20 times cheaper.** It found 13 of 15 cuts (the two cuts within one view keep their colours across the key frames) and even 4 whip pans, because over a 2 s stretch the colours of a panned smooth scene change too. With key frames 8 s apart each stretch is 8 s of decoding, so it slows to 9×; still faster than decoding everything.
- **Without deblocking** the moments found are identical, and the CPU decoder is barely faster. Not worth a setting; the Windows run leaves it out (`--frames nodeblock` measures it).
- **The sound pass is cheap**: 70 to 110 times real time, an hour in under a minute, including reading every byte of the file to find the sound.
- **Decoding every frame of H.265 1440p** ran at 1.1 times real time on this machine; the caching note measured GPU decoding on the test machine at several times that. On the 24-thread desktop decoding everything is plausible but not certain to make 10×, and it competes with proxy building and playback. Key frames make 20× even here.

### Preliminary recommendation

**A + C key frames**, with the picture pass upgraded where every frame is decoded anyway:

1. **Sound first** (approach A on every stream): fast enough to run as soon as a recording is imported, so mic, game and chat moments appear as suggested labels within a minute of an hour's recording.
2. **Picture from key frames** (approach C on key frames, decoding only the stretches where colours change): cuts and some motion, at 15 to 20 times real time or better.
3. **Picture from every frame when a proxy is built**: the proxy build decodes every frame already; computing the tiny pictures there adds cuts within a view and sudden motion for free, and replaces the key-frame result for that file.
4. Moments within 1.5 s are one suggestion, labelled with what found it ("mic+cut"). One sensitivity setting moves A's margin and C's thresholds together. Medium is right on these clips; low drops a quarter of the moments, high adds false positives.

This is **preliminary**. The synthetic clips were made knowing what the detectors look for, their game sound is far quieter and simpler than a real game's, and the thresholds were chosen by reasoning, not by real moments. What real recordings must settle:

- whether A's false positives stay low with a real mic (keyboard, breathing, a person who talks loudly all the time) and real game sound (constant gunfire, music, a mix of game and mic on one stream);
- whether people's picked moments are mostly reactions on the mic (then A carries it) or game events without a reaction (then game-sound weights and the picture matter more);
- whether sudden motion matters to them at all, and if it does, whether frame difference finds it or a movement measure is needed;
- the speed of each pass on the desktop with the GPU decoder, against the 10× target.

### What the owner does

1. In GitHub, open Actions, pick **Highlights spike**, press **Run workflow**, and download the `highlight-bench-windows` artifact when it is done. Unzip it anywhere.
2. Pick **five recordings from different games** (the gate's test), ideally 20 to 40 minutes each, at least one with voice chat. Copy them into the `recordings` folder. They are never uploaded or committed; only `results.md` is sent back.
3. For each recording, make a text file with the same name and `.csv` beside it (`match-04.mkv` gets `match-04.csv`). Watch the recording in any player and write one line per moment you would keep: the time the player shows when it happens (the shout, the explosion, the kill; not where the clip should start), then a comma and a word for what it is: `mic`, `game`, `chat`, `cut` or `motion` (or anything else; those five line up with the tables). For example `12:41,mic`. A first line `time,kind` is allowed. If both of you mark the same recordings, put each person's lists in a separate copy of the unzipped folder and run both: the difference shows how much two people agree, which is the most any finder can reach.
4. If the audio streams are in another order than game, mic, chat and have no names, say so (the program prints how it read each stream at the top of each recording's section, "as game", "as mic").
5. Double-click `run.bat`. It makes three test clips the first time, then measures everything: every frame with the GPU decoder and with the CPU, and key frames only. Plan for 10 to 15 minutes per half hour of recording and leave the PC alone meanwhile.
6. Send back `results.md`. The `found` folder holds what each approach found in each recording, with times, to look at in the player when a number is surprising.


## Sources

- J. P. Bello, L. Daudet, S. Abdallah, C. Duxbury, M. Davies, M. B. Sandler, "A tutorial on onset detection in music signals", IEEE Transactions on Speech and Audio Processing 13(5), 2005: energy and spectral-flux onset functions, adaptive thresholds by median, peak picking.
- S. Dixon, "Onset detection revisited", Proceedings of the 9th International Conference on Digital Audio Effects (DAFx), 2006: spectral flux with a rectified difference is as good as anything more elaborate.
- ITU-R BS.1770, "Algorithms to measure audio programme loudness and true-peak audio level", and EBU Tech 3341, "Loudness metering": momentary (400 ms) and short-term (3 s) loudness, the model for short against long loudness in A.
- H. J. Zhang, A. Kankanhalli, S. W. Smoliar, "Automatic partitioning of full-motion video", Multimedia Systems 1(1), 1993: cuts from histogram differences and pixel differences, with thresholds.
- A. Hanjalic, "Shot-boundary detection: unraveled and resolved?", IEEE Transactions on Circuits and Systems for Video Technology 12(2), 2002: thresholds relative to the local neighbourhood, so motion doesn't read as cuts.
- Y. Rui, A. Gupta, A. Acero, "Automatically extracting highlights for TV baseball programs", ACM Multimedia 2000: the commentator's excited speech (energy and pitch) as the strongest single highlight signal.
- C. Ringer, M. A. Nicolaou, "Deep unsupervised multi-view detection of video game stream highlights", Foundations of Digital Games 2018: highlights in game streams from the streamer's face and the game picture, without per-game training.
- Q. Kong, Y. Cao, T. Iqbal, Y. Wang, W. Wang, M. D. Plumbley, "PANNs: Large-scale pretrained audio neural networks for audio pattern recognition", IEEE/ACM Transactions on Audio, Speech, and Language Processing 28, 2020: small sound classifiers trained on a large public set of labelled clips, the learned approach left for later.
- [FFmpeg filters](https://ffmpeg.org/ffmpeg-filters.html): `scdet` and the `select` filter's scene score; `ebur128`.
- [caching.md](caching.md): no lower-resolution decoding in FFmpeg's H.264 and H.265 decoders; the proxy build that decodes every frame anyway.
