# clarity

The clarity check (8.2): where tracks share frequencies, at the playhead or over the region, worked out on a worker and said in plain words, each finding with one-click fixes. The Audio page draws it (its middle band); nothing here draws.

## Using it

```rust,ignore
use engine::clarity::{Clarity, Outcome, Request};

// Interface thread: starting clones the project (cheap) and queues a job.
let check = Clarity::start(&jobs, project.clone(), sequence, Request::Playhead(playhead));
// Every frame: never waits.
let share = check.progress();
if let Some(Outcome::Done(report)) = check.poll() {
    for finding in &report.findings {
        // finding.words, finding.range, finding.tracks, finding.band_hz, finding.severity
        for fix in &finding.fixes {
            // A button labelled fix.label; pressed:
            let command = fix.apply(&project)?; // an ordinary SetTrackSound: one undo step
        }
    }
}
// A newer request: cancel the old check and start another.
check.cancel();
```

`Request::Playhead(t)` listens one second either side of `t`; `Request::Region(a, b)` listens to a stretch. A `Report` holds the findings (worst first), every heard track's mean level in each band (`tracks`, for drawing where tracks share frequencies), and the stretch listened to; `amount(problem, track)` adds up severity times seconds, for telling whether a fix helped.

## Files

| File | What it holds |
|---|---|
| `mod.rs` | The API: `Request`, `Problem` (`CoversVoice`, `Muddy`), `Finding`, `TrackBands`, `Report`; `analyse` (the work itself, on the caller's thread, with `go_on` and `progress`); `Clarity` (`start` on the jobs' Analysis pool at Interactive priority, `start_with` another `DrySound`, `progress`, `cancel`, `poll`, `wait`) and `Outcome` |
| `listen.rs` | `Listening`: each heard track (mute and solo as the mixer has them) through `audio::strip::Strip`, in the mixer's order (dialogue first, summed into the ducking key, then the others ducked under it), started a second early so envelopes settle, the clean-up's fixed delay taken off; `next_frame` gives each track's power per band for the next 1024 samples (2048-sample windows centred on them). `DrySound`: a track's clips before the track's own effects; `Clips` decodes them with `mix::Mixer::mix_track`, as the feeder does |
| `bands.rs` | The 18 third-octave bands from 160 Hz to 8 kHz, their importance to speech (the published Speech Intelligibility Index standard's numbers for average speech, ANSI S3.5-1997 Table 3), the ranges used (speech 315 Hz to 4 kHz, low middle 200 to 500 Hz, presence 1 to 4 kHz), and `BandMeter` (an FFT of a Hann window, mean-square power per band) |
| `judge.rs` | `Judge`: the measures frame by frame, then stretches, words and the fixes on offer (see How it judges) |
| `fix.rs` | `Fix` and `Remedy`: lower under the dialogue (ducking), cut around 300 Hz, make room for the voice (EQ dips); `Fix::apply` makes the `SetTrackSound` on the project as it is now; `called` names a track in a sentence ("the music on A2") |
| `tests.rs` | Made-up tracks (`audio::test_signal::speech`, a chord over pink noise, tones and band-passed noise between 200 and 500 Hz): music over dialogue is found in its band and time; a clean mix and a muted loud track give nothing; three tracks piling up are found and two are not; every fix offered makes its problem smaller when checked again; the playhead's short stretch; progress and cancelling, also on the Analysis pool; a recording's tracks decoded and measured |

## How it judges

- **Covering the voice.** Only where the dialogue tracks together speak (over -50 dBFS in the speech bands) and only against tracks that aren't dialogue. Per band from 315 Hz to 4 kHz: 6 dB or more under the voice is clear, 6 dB or more over it is covered, a straight line between. A frame's severity is the covered share of the bands the voice has sound in, weighted by importance. Smoothed over 0.3 s; frames over 0.3 make stretches, joined across gaps up to 0.5 s, kept from 0.4 s. A finding's severity is the mean over its speaking frames; its band is the worst band and its neighbours at least half as bad.
- **Muddy.** Three or more tracks over -50 dBFS between 200 and 500 Hz and within 10 dB of the loudest there; then how far the mix's low middle stands over its presence (1 to 4 kHz, per octave: a balanced sound reads 0 dB), nothing at 3 dB, as bad as it gets at 12 dB. Smoothed over 0.5 s, over 0.25, joined across 0.5 s, kept from 0.5 s.
- **Words** use Hz and kHz and nothing else technical: "The music on A2 covers the voice between 500 Hz and 4 kHz: words may be hard to follow." (from severity 0.6; under it, "the voice may sound less clear"), "The music on A1, the sound effects on A3 and the background sound on A2 pile up between 200 and 500 Hz: the mix may sound a little muddy."
- **Fixes on offer.** Covering: lower under the dialogue (unless it already ducks as deep as the fix goes), make room for the voice (when the band reaches 1.6 to 3.15 kHz), cut around 300 Hz (when it reaches down to 630 Hz). Muddy: cut around 300 Hz on up to three of the tracks that aren't dialogue, loudest first. An EQ fix is offered only when the track's EQ has room for it.
- **Fixes' numbers.** Ducking -10 dB with the effect's own hold (300 ms) and speeds (down 40 ms, up 400 ms), its threshold 15 dB under the dialogue's middle speaking level (from -60 to -25 dB); used again, 6 dB deeper, to -24 dB. Cut: -3.5 dB at 300 Hz, Q 1. Room: -2.5 dB at 2.5 kHz, Q 1. Used again, a dip goes deeper by as much, to -9 dB.

## Numbers

Made-up tracks, eight seconds (test build, two-core cloud machine under load): music level with the voice between 2 and 6 s is found from 1.94 to 3.18 s and 3.97 to 5.99 s (the voice pauses between), between 500 Hz and 4 kHz, severity about 0.7. Its amount (severity times seconds) 2.32 goes to 0.84 with ducking, 2.15 with room for the voice, 2.23 with the cut at 300 Hz. Three tracks piling up: severity 0.37 over the whole eight seconds; the cut on the loudest takes the amount from 2.92 to 1.95, on all three to nothing. Checking three tracks over eight seconds took 68 ms (about 3 ms per track per second of sound, decoding apart); the playhead's two seconds 23 ms.
