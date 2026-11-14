use super::correlate::{self, Method};
use super::speech::{Device, game, record, speech};
use super::*;
use project::{ClipId, MediaId};

const MIC: Device = Device {
    gain: 0.15,
    low_cut: 250.0,
    high_cut: 2500.0,
    hiss: 0.005,
    drift_ppm: 0.0,
};

const GAME_RECORDING: Device = Device {
    gain: 0.6,
    low_cut: 80.0,
    high_cut: 3800.0,
    hiss: 0.003,
    drift_ppm: 0.0,
};

/// One made-up pair: the reference, the other recording, and where the
/// other truly fits on it (`None`: it doesn't, the sounds are unrelated).
struct Case {
    name: &'static str,
    other: Vec<f32>,
    shift: Option<i64>,
    /// How far off the fit may be (samples): its clock drifts, so the
    /// fit is smeared over a few milliseconds and only the frame counts.
    drifts: bool,
}

/// The game recording (speech over game sound) and the other recordings
/// of the comparison, at `rate`: different mics, levels and noise, an
/// offset of seconds to a minute, partial overlap, a drifting clock, and
/// sound that has nothing to do with it.
fn cases(rate: u32) -> (Vec<f32>, Vec<Case>) {
    let r = rate as i64;
    let at = |s: i64| s * r;
    let shared = speech(1, 200.0, rate);
    let reference = record(
        &shared,
        &game(2, 70.0, rate),
        GAME_RECORDING,
        0,
        at(60) as usize,
        rate,
        3,
    );
    let quiet_game: Vec<f32> = game(7, 120.0, rate).iter().map(|v| v * 0.5).collect();
    let mut cases = vec![
        Case {
            name: "other mic, quieter, mic started 37 s earlier",
            other: record(&shared, &[], MIC, -at(37) - 2345, at(120) as usize, rate, 4),
            shift: Some(-at(37) - 2345),
            drifts: false,
        },
        Case {
            name: "noisy room and game sound through speakers",
            other: record(
                &shared,
                &quiet_game,
                Device {
                    gain: 0.35,
                    hiss: 0.02,
                    high_cut: 3000.0,
                    ..MIC
                },
                at(41) + 17,
                at(100) as usize,
                rate,
                5,
            ),
            shift: Some(at(41) + 17),
            drifts: false,
        },
        Case {
            name: "only the last 20 s overlap",
            other: record(&shared, &[], MIC, at(40), at(60) as usize, rate, 6),
            shift: Some(at(40)),
            drifts: false,
        },
        Case {
            name: "clock 50 ppm fast",
            other: record(
                &shared,
                &[],
                Device {
                    drift_ppm: 50.0,
                    ..MIC
                },
                -at(20),
                at(100) as usize,
                rate,
                7,
            ),
            shift: Some(-at(20)),
            drifts: true,
        },
    ];
    for (seed, name) in [(99, "unrelated speech"), (5, "unrelated speech over game sound")] {
        let own = if seed == 5 {
            game(9, 100.0, rate)
        } else {
            Vec::new()
        };
        cases.push(Case {
            name,
            other: record(
                &speech(seed, 120.0, rate),
                &own,
                MIC,
                0,
                at(100) as usize,
                rate,
                8,
            ),
            shift: None,
            drifts: false,
        });
    }
    cases.push(Case {
        name: "unrelated game sound",
        other: record(&game(11, 100.0, rate), &[], MIC, 0, at(100) as usize, rate, 8),
        shift: None,
        drifts: false,
    });
    (reference, cases)
}

/// The comparison of the research note (`docs/research/sync-by-sound.md`):
/// both methods on the same made-up recordings. The whitened one must find
/// every related pair to the sample and be no worse than onsets anywhere;
/// unrelated sound must come out unsure with it.
#[test]
fn whitened_comparison_beats_onsets_on_made_up_recordings() {
    let rate = LOW_RATE;
    let (reference, cases) = cases(rate);
    let mut right = [0usize; 2];
    for case in &cases {
        for (m, method) in [Method::Whitened, Method::Onsets].into_iter().enumerate() {
            let fit = correlate::fit(&reference, &case.other, rate, method, 2 * rate as usize)
                .unwrap()
                .unwrap();
            let error = case.shift.map(|s| fit.shift - s);
            let sure = Sureness::of(&fit);
            println!(
                "{:48} {method:?}: off by {error:?} samples at {rate}/s, ratio {:.2}, prominence {:.1}, {sure:?}",
                case.name, fit.ratio, fit.prominence
            );
            let frame = i64::from(rate) / 60;
            let good = match error {
                // Within a millisecond (what the onsets can find), or a
                // frame when the clock drifts.
                Some(e) => e.abs() <= if case.drifts { frame } else { 8 } && sure > Sureness::Unsure,
                None => sure == Sureness::Unsure,
            };
            right[m] += usize::from(good);
            if method == Method::Whitened {
                assert!(good, "{}: whitened gave {error:?}, {sure:?}", case.name);
                if let Some(e) = error.filter(|_| !case.drifts) {
                    assert!(e.abs() <= 1, "{}: to the sample at the low rate", case.name);
                }
            }
        }
    }
    println!(
        "right: whitened {}, onsets {} of {}",
        right[0],
        right[1],
        cases.len()
    );
    assert!(right[0] >= right[1]);
}

/// Sound kept in memory at the full rate, read at the low rate by
/// averaging each six samples.
struct Memory(Vec<f32>);

impl Sound for Memory {
    fn read(&mut self, from: i64, length: usize, rate: u32, _: &Watch) -> diag::Result<Vec<f32>> {
        let step = (FULL_RATE / rate) as i64;
        Ok((0..length as i64)
            .map(|i| {
                let first = (from + i) * step;
                (first..first + step)
                    .map(|j| {
                        usize::try_from(j)
                            .ok()
                            .and_then(|j| self.0.get(j))
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .sum::<f32>()
                    / step as f32
            })
            .collect())
    }
}

fn quietly() -> Watch<'static> {
    Watch {
        progress: &|_| {},
        cancelled: &|| false,
    }
}

/// Lines up `other` with `reference`, whole, from memory.
fn line_up_memory(reference: Vec<f32>, other: Vec<f32>) -> Found {
    let span = |x: &Vec<f32>| Span {
        from: 0,
        length: x.len() as i64,
    };
    let (rs, os) = (span(&reference), span(&other));
    let mut reference = Memory(reference);
    let low = read_low(&mut reference, rs, &quietly()).unwrap();
    line_up(&mut reference, rs, &low, &mut Memory(other), os, &quietly()).unwrap()
}

/// The exact fit: within a tenth of a millisecond of where the other
/// recording truly is, far inside a frame (16.7 ms at 60 per second).
/// The mic's filters move its sound by a few samples themselves.
#[test]
fn a_mic_recorded_separately_lines_up_to_a_fraction_of_a_frame() {
    let rate = FULL_RATE;
    let r = i64::from(rate);
    let shared = speech(21, 40.0, rate);
    let reference = record(
        &shared,
        &game(22, 25.0, rate),
        GAME_RECORDING,
        0,
        20 * r as usize,
        rate,
        23,
    );
    // The mic started 9.5 s and 123 samples before the game recording.
    let delay = -(9 * r + r / 2 + 123);
    let other = record(&shared, &[], MIC, delay, 30 * r as usize, rate, 24);
    let found = line_up_memory(reference, other);
    // A moment at reference sample m is at other sample m - delay.
    let error = found.offset.unwrap() - delay;
    println!("off by {error} samples at {rate}/s ({:?})", found.sureness);
    assert!(error.abs() <= 5, "off by {error} samples");
    assert_eq!(found.sureness, Sureness::Sure);
}

#[test]
fn unrelated_sounds_are_unsure_and_silence_finds_nothing() {
    let rate = FULL_RATE;
    let r = rate as usize;
    let reference = record(&speech(31, 20.0, rate), &[], GAME_RECORDING, 0, 15 * r, rate, 1);
    let other = record(
        &speech(32, 20.0, rate),
        &game(33, 20.0, rate),
        MIC,
        0,
        15 * r,
        rate,
        2,
    );
    let found = line_up_memory(reference.clone(), other);
    assert_eq!(found.sureness, Sureness::Unsure, "{found:?}");

    let found = line_up_memory(reference, vec![0.0; 10 * r]);
    assert_eq!(found, Found::nothing());
}

fn clip(start: f64, source_in: f64, duration: f64) -> Clip {
    let t = |s: f64| Time::from_ratio((s * 1000.0).round() as i64, 1000).unwrap();
    Clip {
        id: ClipId(1),
        media: MediaId(1),
        stream: 1,
        start: t(start),
        source_in: t(source_in),
        duration: t(duration),
        link: None,
        speed: Default::default(),
        look: Default::default(),
        sound: Default::default(),
        keyframes: Default::default(),
        ramp: Default::default(),
        blend: Default::default(),
        kind: Default::default(),
        lines: Default::default(),
        effects: Default::default(),
    }
}

#[test]
fn the_stretches_compared_are_what_the_clips_play_near_where_they_are() {
    let s = |x: i64| x * i64::from(FULL_RATE);
    // A ten-minute game recording from 0; a mic clip cut from 30 s into
    // its file, placed at 60 s for a minute.
    let game = clip(0.0, 0.0, 600.0);
    let mic = clip(60.0, 30.0, 60.0);
    let (r, o) = spans(&game, &mic);
    // The reference's two minutes start where the mic does.
    assert_eq!(
        r,
        Span {
            from: s(60),
            length: s(120)
        }
    );
    // The mic: all it plays (its file from 30 s to 90 s).
    assert_eq!(
        o,
        Span {
            from: s(30),
            length: s(60)
        }
    );

    // Near the reference's end, its two minutes end with it.
    let late = clip(590.0, 0.0, 60.0);
    let (r, _) = spans(&game, &late);
    assert_eq!(
        r,
        Span {
            from: s(480),
            length: s(120)
        }
    );

    // A long mic placed after the start of a short game clip: the
    // game's whole two minutes, and the mic searched three minutes either
    // way of them as it sits (from its own start, so 200 s of it).
    let long = clip(100.0, 0.0, 1000.0);
    let short_game = clip(0.0, 5.0, 120.0);
    let (r, o) = spans(&short_game, &long);
    assert_eq!(
        r,
        Span {
            from: s(5),
            length: s(120)
        }
    );
    assert_eq!(
        o,
        Span {
            from: 0,
            length: s(200)
        }
    );

    // Where the mic goes: a moment 5 s into the game's file (at 0 on
    // the timeline) is 12 s into the mic's file.
    let offset = s(5) - s(12);
    let start = lined_up_start(&short_game, &long, offset);
    // The mic's file at 12 s is then at 0: it starts at 0 - 12 + its in point (0).
    assert_eq!(start, Time::seconds(-12));
}

/// Writes a mono 16-bit sound file at the full rate.
fn write_wav(path: &Path, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&FULL_RATE.to_le_bytes());
    bytes.extend_from_slice(&(FULL_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for v in samples {
        bytes.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Gate 8's line, with files: a separately recorded mic, cut from a longer
/// recording, read with the media decoder and lined up within a frame.
#[test]
fn sound_files_line_up_within_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let rate = FULL_RATE;
    let r = i64::from(rate);
    let shared = speech(41, 70.0, rate);
    let game_file = dir.path().join("game.wav");
    let mic_file = dir.path().join("mic.wav");
    let unrelated_file = dir.path().join("other.wav");
    write_wav(
        &game_file,
        &record(
            &shared,
            &game(42, 40.0, rate),
            GAME_RECORDING,
            0,
            30 * r as usize,
            rate,
            43,
        ),
    );
    // The mic started 12.25 s before the game.
    let delay = -(12 * r + r / 4);
    write_wav(
        &mic_file,
        &record(&shared, &[], MIC, delay, 60 * r as usize, rate, 44),
    );
    write_wav(
        &unrelated_file,
        &record(&speech(45, 40.0, rate), &[], MIC, 0, 30 * r as usize, rate, 46),
    );

    // On the timeline: the game from 0 for 30 s; the mic clip plays its
    // file from 5 s, placed at 0 too.
    let game_clip = clip(0.0, 0.0, 30.0);
    let mic_clip = clip(0.0, 5.0, 50.0);
    let (rs, os) = spans(&game_clip, &mic_clip);
    let sound = |path: &Path, span| ClipSound {
        path: path.to_path_buf(),
        stream: 0,
        span,
    };
    let reference = sound(&game_file, rs);
    let shares = std::sync::Mutex::new(Vec::new());
    let progress = |share: f32| shares.lock().unwrap().push(share);
    let watch = Watch {
        progress: &progress,
        cancelled: &|| false,
    };
    let found = line_up_files(
        &reference,
        &[sound(&mic_file, os), sound(&unrelated_file, os)],
        &watch,
    )
    .unwrap();
    let mic = found[0].as_ref().unwrap();
    // The game's file at m is the mic's file at m - delay.
    let error = mic.offset.unwrap() - delay;
    let frame = r / 60;
    println!(
        "file: off by {error} samples ({:.3} ms), {:?}",
        error as f64 * 1000.0 / r as f64,
        mic.sureness
    );
    assert!(error.abs() < frame / 10, "off by {error} samples");
    assert_eq!(mic.sureness, Sureness::Sure);
    // Moved there, the mic's file at 12.25 s plays at 0 with the game's.
    let start = lined_up_start(&game_clip, &mic_clip, mic.offset.unwrap());
    let want = Time::from_ratio(5 * 4 - 49, 4).unwrap();
    assert!((start - want).as_secs_f64().abs() < 1.0 / 60.0, "{start:?}");
    assert_eq!(found[1].as_ref().unwrap().sureness, Sureness::Unsure);
    let shares = shares.lock().unwrap();
    assert!(
        shares.windows(2).all(|w| w[0] <= w[1] + 1e-6),
        "progress only goes up"
    );
    assert_eq!(shares.last(), Some(&1.0));

    // A stop is an error, not a result.
    let stop = Watch {
        progress: &|_| {},
        cancelled: &|| true,
    };
    assert!(line_up_files(&reference, &[sound(&mic_file, os)], &stop).is_err());
}
