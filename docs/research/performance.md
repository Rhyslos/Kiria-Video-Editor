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

## Every timed process (third pass)

A review of every piece of work the app times or waits for, ranked by what it costs the person. The first five, measured here and fixed:

| Where | Before | After | Why |
|---|---|---|---|
| A project open and nobody touching it | 1.6 CPU threads busy for as long as it stays open (a sync of the library index and a redraw every frame) | 0.08 | A short list kept inline (no media labels or custom lists yet: most projects) is copied, not shared, when a project is cloned, so comparing pointers always said "changed". Short lists are now compared by contents (`project::same`, as cheap as the pointers), and the index only wakes the window when an answer changed |
| Closing the editor | 5 to 20 s after the window went, on a big project | the job running, then out | Closing ran every queued job first (proxy checks, clip pictures, loudness). Now queued jobs are given up when the pools close, except those that write something that must be on disk (`JobSystem::submit_write`: saves, the timeline layout, a saved frame or version, clearing the cache) |
| Remove silence, 500 stretches (2 516 clips) | 476 ms on the interface thread | 3.3 ms | Each stretch walked every clip twice and moved everything after it. Now each track is worked from its end with searches, and clips are moved once at the end by the stretches before them, added up. The same cuts in the same order give the same new ids, so journals replay as before |
| Ripple delete of 200 clips; moving 976 | 17 ms; 47 ms | 1.2 ms; 1.1 ms | Lists searched once per clip became sets; a moved clip is checked against its neighbours by a search, and the tracks merged |
| Select all on that timeline | 19 ms of interface work per frame (median, test build) | 3.8 ms (2.3 ms with nothing selected) | Whether each clip was selected, and kept, was a search through the selection: 2 516 × 2 516. Now sets |
| Measuring the mix before an export (Export page's check, Normalise) | 120 s of the timeline in 1.66 s, and Normalise mixed it all again | 1.36 s, and Normalise takes the check's figure | A mix being measured skips every track's and the master's meter (`Mixer::without_meters`: the same sound); an export of the snapshot the page measured normalises with that figure when its frame and sample rates are the timeline's (`Export::start_knowing`) |

Each edit was checked against the code before it on the big timeline: 390 edits (stretches inside, across and on the edges of clips, past the end, selections of up to 300 clips, moves, splits) give the same project, ids included. One set of stretches the old code refused as damaging the project (it left clips out of order) now gives what each clip's parts outside the stretches, moved back, should be; a test checks that model.

Left for later, in the order of the review: each sound stream opening and reading its file on its own (playback's feeders and export), the Library's loudness per stream, drawing waveforms at every peak in view, the import check opening each file several times, per-edit cost growing with the project file, the launcher's picture decoded after every save, clip pictures and Library cards worked out again every session.

### Opening sound streams (item 6)

| Where | Before | After | Why |
|---|---|---|---|
| Opening a sound stream of a 1440p recording again (a jump to another of its clips, every start of playback) | 21 to 34 ms | under 1 ms | Each open probed the file, decoding some pictures. A file's stream start times (all a decoder takes from probing) are now kept for the session by path, size and change time, and later opens read only the header when it holds the sound's codec, rate and channels |
| A jump across clips on the big timeline (three tracks, test timing) | about 70 ms | about 7 ms | The same: the jump opened each track's decoder for the new clip |
| `AudioStreams` (waveforms), five 10-minute streams | 1.70 s | 1.40 s (two threads) | Packets are read a second at a time and each stream decoded on a thread of its own; on more cores it scales with the streams |

Priming the tracks on threads of their own when playback starts was tried and left out: with the opens this quick it saves little, and what a track mixes depended on the order threads ran in by a few millionths, which playback's sample-exact tests rightly refuse. One demuxer handing packets to every stream's decoder during playback (a fifth of the reading for five streams) is left for later: tracks may play different moments of the same file, so they can't simply share one reader.

### The Library's loudness (item 7)

Each sound stream of a file was measured in a pass of its own (a five-stream 1440p recording read five times) and again every session. Now every stream is measured in one pass (`media::AudioStreams`, the streams decoded side by side) and the readings are kept by the file's fingerprint in the cache folder's `loudness/`, so a file is read for its loudness once, ever. The readings are exactly those of measuring each stream alone (tested).
### Drawing long waveforms (item 8)

Each pixel column of a waveform took the loudest of every peak under it, so zoomed out over hours a frame went over every peak (100 a second, so 720 000 for two hours). Now each set of peaks gets levels (the loudest of each two, of each two of those, and so on), made once on a thread of their own and kept while it is drawn, and a column takes a few steps. Two 2-hour waveforms all on screen: 0.86 ms a frame before, 0.22 ms after (`timeline_cost`'s `long_waveform_frame_cost`, release build); the cost now follows the width, so five 3-hour streams cost what two do. The Library's rows, cards and detail panel draw through the same levels.
### The launcher's picture after a save (item 11)

Every save (an autosave each 30 s of editing) decoded the frame at the playhead again for the launcher's picture: about 290 ms of processor on a worker for a long-GOP recording, and the same on closing. The editor now remembers what the picture it wrote shows (`engine::thumbnail::shows`: the file, stream and moment, worked out without reading anything) and writes it again only when that changed, as when the playhead moved to another frame. Closing waits for none when nothing changed; a picture under way when the app closes still finishes (it is a write).

### The import check (item 9)

The check opened a file three times plus once per sound stream, each with a full probe, and decoded the first picture with a decoder of every thread. Now it opens the file once for describing it and checking its codecs, keeps what that probe found for the file's sound decoders (they read only the header, under a millisecond each, also when the file is played later), and decodes the first picture on one thread. A 1440p recording with three sound streams: 104 ms to 88 ms; a 1080p phone clip 28 ms to 21 ms. What is left is the fingerprint and the picture decoder's own open, which probes the file again: a picture decoder takes facts only probing finds (the pixel format, how far frames are reordered), so it is left to probe.

### Clip pictures kept (item 12)

The pictures inside video clips (and the Library's cards, which use the same strips) were read from every recording again each session, a picture at a time (6.6 ms each for a long recording's key frames here). Now each strip is kept in the cache folder's `pictures/` by the recording's fingerprint, the pictures as small JPEGs (a few hundred kilobytes a recording, 512 MB at most, used longest ago dropped first), and read back in milliseconds the next time any project shows that recording. A strip also opens its decoder once where it used to open it twice. Settings › Cache clears them with the waveforms.
### A Library card's picture first

A card waited for its file's whole strip before showing anything: 6.8 s for a one-minute 1440p recording here, so a new import's cards stayed empty for a long time. Each card's resting picture (a third of the way in, on a picture of the strip) is now read on its own at a higher priority, ahead of every strip, in 29 ms for the same file, and kept beside the strips; the strip still follows for skimming.

### Small things (items 15 and 16, and one from proxies)

- **The search palette** named every clip of the timeline each frame, even with nothing typed, and looked each clip up in the list of files found one by one: 2.5 to 2.8 ms a frame on the big timeline. Now nothing is named until something is typed, and found files are a set.
- **Missing files** (`relink::watch`) made a list of every file's path each frame to see whether the media changed; now a pointer comparison, the list made only when it did.
- **The launcher's cards** counted every card being read for each card drawn; now only for a card that would start reading.
- **New files in your folders:** a set of the files found instead of a search through them for each file seen.
- **"Proxy needed?"** judged a recording once per media entry, so one placed thirty times was measured thirty times (0.1 to 1 s each at the start); now once per recording and stream a session.

Looked at and left: the true peak's ring of samples as a flat double buffer (the same result to the bit, but no gain measured here); finding encoders side by side and the preview's four open decoders, which can only be measured on a machine with a real graphics card and layered timelines.

### The project file (item 10, first part)

The project was written indented, a field per line: 3.1 MB for the 2 516-clip timeline, a third of it spaces and line breaks. It is now written as a line per header field (the launcher still reads only those) and the project on one line: 1.0 MB, turned into text in 5 ms (13 before), read back in 80 ms (91). The format is the same to every RON reader, so it needs no new format version, and files written indented open as before. Left for a decision: leaving default values out of the file (a format version and a migration; several times smaller again), and checking only the tracks an edit touched.
