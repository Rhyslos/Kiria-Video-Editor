# Performance review, November 2026

A pass over the whole app for work that grows faster than it should, measured on the same machine (2 CPU threads, software graphics, so absolute numbers are high; the ratios are what count). Each change keeps what the app shows and plays the same; each has a test that says so.

## How it was measured

- **Interface thread:** `editor <project> --responsiveness-test` on a project made for it: a 1440p60 recording with three sound streams placed 30 times and cut every 3 s (2 516 clips over 30 minutes), against the same recording placed once. `perf record` on the editor showed where the interface thread's time went.
- **Background work:** small timing programs (not kept) on the same recordings: reading waveforms, clip pictures and jumps in a 10-minute 1080p60 H.264 recording with a key frame every 250 frames and B-frames, as game recorders write by default.

## What was found and changed

| Where | Before | After | Why |
|---|---|---|---|
| Timeline drawn each frame (2 516 clips) | 7.1 ms of interface work per frame (median) | 2.7 ms (one clip: 0.4 ms) | The audio lines were matched to their clips with a search per clip, and each video clip's tooltip looked for its sound across every audio track: both grew with the square of the clips. Now one walk per track, and the tracks a link's sound is on worked out once a frame; a file's summary is made once a frame, not once per clip |
| Clip pictures of a long recording | 263 ms a picture (15 pictures 37 s apart: 3.9 s) | 6.6 ms a picture (0.1 s) | Each picture was the exact frame, decoded from the key frame before it. Where key frames are at most half the pictures' spacing apart, each picture is now the key frame nearest its moment: one frame decoded |
| Jumps in a recording with B-frames | 12 jumps: 3.5 s, 1 630 frames decoded | 2.6 s, 1 213 frames | Frames due more than half a second before the first frame that counts are decoded with `AVDISCARD_NONREF`: frames no other frame is made from are left out |
| Jumps on the window's graphics device | every frame passed copied for keeping (a tester's card: 2 ms a frame, 234 frames a jump) | only the last 2 s before the moment | The preview keeps 2 s behind the playhead; frames further back were copied and then dropped. Not measurable here (no video decoding on this graphics); the copy per frame is an image allocated, a copy submitted and waited for |
| Waveforms of a file with several sound streams | the whole file read once per stream, resampled to 16 kHz, every session | the file read once for all its streams, at their own rate, and kept on disk by fingerprint | For a three-hour recording with five streams that is a fifth of the reading, and none the next time. Resampling was a fifth of the work and changes nothing a peak shows |
| Every sound decoder | read every packet of the file and dropped the picture's | the demuxer skips the other streams | Less copying for playback's feeders (one per sound stream, each reading the whole file), export and the checks |

## Looked at and left as they are

- **Applying an edit** to the 2 516-clip project: a split takes 31 µs; opening the project 137 ms. Snapshots are shared trees (`imbl`), so undo history costs little.
- **Saving:** the snapshot is taken on the interface thread (a pointer copy) and written on a worker.
- **The mixer:** 32 tracks at under 1 ms a block (5.3 ms of sound) on this machine; the clean-up's windows only add the sound back when it is off.
- **`Playback::follow`** compares a few numbers per track each frame.

## Next, if more is needed

- Reading a file's loudness in the Library (one pass per stream, like waveforms were): `media::AudioStreams` does it in one.
- Making the timeline's clip data only when the project, the selection or a reading changes, instead of every frame.
- Clip pictures kept on disk like waveforms.
- A pool of the images frames are copied into on the window's device, and no wait after each copy.

## Proxies and export (second pass)

Proxy building and export had the same shape: one long chain of steps per frame, one after another.

- **Proxies.** A build decoded a frame, copied it back from the graphics card, shrank it on the processor and encoded it, in one line of work per file and one file at a time. Measured here (2 threads, software): 1440p60 builds at 4 to 5 times real time with the processor full, about half of it the one thread shrinking and encoding. Now:
  - several files at once on a processor with many cores (`app::proxies::BuildPace`);
  - a long file in stretches between key frames, each with its own decoder, shrinker and encoder, joined at the end by copying packets (`media::proxy::write`'s `workers`). A test checks that a proxy built in three stretches has exactly the frames, times and order of one built in one pass, with groups of pictures open and closed. On this 2-thread machine it can't be faster (the processor is already full: 3.2 to 4.0 times real time against 3.9 to 4.1); on a 24-thread processor the stretches use the cores one pass leaves idle;
  - the processor's encoder with as many threads as it finds useful (it had one);
  - "Build proxies at full speed": more at once, not waiting while the timeline plays.
  - Left: keeping the whole chain on the graphics card (decode, shrink and encode there, with no copy back). That needs the card's own scaling and an encoder fed from it, different for each card maker's encoder, and can only be built and measured on the owner's PC. A key frame every 16 frames instead of 8 was looked at and left: the encoder at `ultrafast` spends little on key frames, and jumps inside a proxy would cost more.
- **Export.** For each frame it asked for the picture and waited, read it back and waited, encoded it and waited, then mixed the sound. Now the encoder has a thread of its own with a few frames' room, the next frame is asked for before the current one is read back, and read-back buffers are reused. The export tests (frame-exact pictures, sound to the sample, letterboxing) pass unchanged. Timing here means nothing (software graphics take both threads, and other work was running): it needs the owner's PC, where decoding, putting together, read-back and encoding use different parts of the machine and can overlap. Left: handing frames to a graphics card's encoder without the copy back to memory.
