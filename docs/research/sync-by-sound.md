# Sync by sound

Lining up a separately recorded sound (a mic recorded on its own, a friend's recording of the same match) with the game recording by what both hear. Build plan step 8.3, and Gate 8's "lines up a separate mic recording to within one frame".

Status: a short comparison (working rule 7, light: no spike, the comparison is a test that runs with the others) and the chosen method, built in `crates/engine/src/sync.rs`.

## 1. Problem

| Need | Target |
| --- | --- |
| Accuracy | within one frame (16.7 ms at 60 per second, Gate 8); to the sample where the sound allows |
| Different devices | another mic with more or less bass and treble, another level, its own hiss and room noise, game sound in one of them |
| Where to look | offsets of a few minutes either way, partial overlap, a clip that is a part of a longer recording |
| Unrelated sound | must come out "unsure" and move nothing unless asked |
| Interface | on a worker with progress and cancel; nothing over ~2 ms on the interface thread |

## 2. Approaches

- **Plain cross-correlation** of the waveforms: the textbook answer, but loud low frequencies (game booms, a mic's rumble) and level differences dominate it.
- **Generalised cross-correlation with phase transform** (whitening): divide the cross-spectrum by its own size so every frequency counts the same; the peak becomes sharp and level and colour stop mattering. Known to suffer in heavy noise, which a weight a little below full whitening (the size to a power under 1) and keeping to the band a voice carries in (150 to 3 500 Hz) eases.
- **Envelope or onset correlation**: compare where sound starts (rises in loudness) rather than the waveform; immune to colour and phase, robust to reverberation, but only as fine as its frame step and blind to which frequencies the rises are in.

## 3. Comparison

Both are run by the test `engine::sync::tests::whitened_comparison_beats_onsets_on_made_up_recordings` on the same made-up recordings at 8 000 samples per second: a minute of speech-like sound (voiced syllables with a pitch and vowel colours, hiss for consonants, pauses) over game sound (rumble, a chord, bangs) as the game recording, and the same speech recorded again through other "devices".

| Case | Whitened (voice band, power 0.8) | Onsets (1 ms steps) |
| --- | --- | --- |
| Other mic (less bass and treble, quarter the level), started 37 s earlier | exact; ratio 18.6, prominence 105: Sure | wrong (13 s off), Unsure |
| Noisy room plus game sound through speakers, 41 s later | exact; 1.63, 9.8: Likely | wrong, Unsure |
| Only the last 20 s overlap | exact; 6.7, 38.7: Sure | wrong, Unsure |
| Clock 50 ppm fast (a minute drifts 3 ms) | 31 samples (3.9 ms) off; 1.53, 9.1: Likely | wrong, Unsure |
| Unrelated speech | 1.01, 5.4: Unsure | Unsure |
| Unrelated speech over game sound | 1.08, 6.3: Unsure | Unsure |
| Unrelated game sound | 1.05, 6.6: Unsure | Unsure |

Ratio is the best peak over the best one more than 40 ms away; prominence is the best peak over the correlation's root mean square. Whitened: 7 of 7 right; onsets: 3 of 7 (only the unrelated ones). The game sound under the voice in the reference is what defeats the onsets: its rises in loudness are mostly the game's, and they can't be told apart by frequency.

Plain full whitening (power 1, every frequency from 0 to 4 kHz) was also tried while choosing: as good on clean pairs, but it lost the drifting clock (it picked a peak 22 s off) and gave the noisy room a lower ratio, so the voice band and power 0.8 were kept.

## 4. Result

Chosen: whitened correlation in the voice band at 8 000 per second over the whole stretches (the reference: up to 2 minutes of what its clip plays, the part beside the other clip when they overlap; the other: what its clip plays up to 3 minutes either way of where it sits), then six seconds where both are loudest read again at 48 000 per second and compared within 10 ms of the rough fit (150 Hz to 8 kHz), which gives the sample. Sureness from the rough fit: Sure at a ratio of 2 and prominence of 12, Likely at 1.4 and 8, else Unsure.

Measured: a separately recorded mic (other filters, level and hiss, cut from a longer recording), written as files and read through the media decoder, lines up 0 samples off at 48 000 per second (`sound_files_line_up_within_a_frame`); from memory, likewise 0 samples (`a_mic_recorded_separately_lines_up_to_a_fraction_of_a_frame`). Comparing a two-minute reference with three minutes of the other took about 0.2 s on the 2-core cloud machine; reading the sound takes longer than comparing it.

Not done: correcting clock drift (a long recording from a drifting device lines up where the comparison was made and drifts from there; a stretch would be needed), and real recordings from the owner's machines, which the Gate 8 check uses.
