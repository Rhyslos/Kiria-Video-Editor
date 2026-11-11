//! Decoding sound: the fixtures carry steady tones (440, 660 and 880 Hz).

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::unwrap_used)]

use media::AudioDecoder;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Rough pitch: zero crossings of the left channel per second, halved.
fn pitch(samples: &[f32], rate: f64) -> f64 {
    let left: Vec<f32> = samples.chunks_exact(2).map(|s| s[0]).collect();
    let crossings = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f64 / (left.len() as f64 / rate)
}

#[test]
fn sound_streams_decode_to_stereo_at_any_rate() {
    for (stream, hz) in [(1, 440.0), (2, 660.0), (3, 880.0)] {
        let mut d = AudioDecoder::open(&fixture("game-3-audio.mkv"), stream, 44_100).unwrap();
        d.seek(0.5);
        let mut buf = vec![0f32; 44_100 / 2 * 2];
        assert_eq!(d.read(&mut buf), 44_100 / 2);
        let p = pitch(&buf, 44_100.0);
        assert!((p - hz).abs() < hz * 0.03, "stream {stream}: {p} Hz");
        assert!((d.position() - 1.0).abs() < 0.01, "{}", d.position());
    }
}

#[test]
fn reading_to_the_end_and_seeking_back() {
    let mut d = AudioDecoder::open(&fixture("voice.mp3"), 0, 48_000).unwrap();
    let mut buf = vec![0f32; 48_000 * 2 * 3];
    let got = d.read(&mut buf);
    assert!((70_000..=74_000).contains(&got), "about 1.5 s, got {got}");
    assert!(d.read(&mut buf) == 0, "nothing after the end");
    d.seek(1.0);
    assert!((d.position() - 1.0).abs() < 1e-9);
    let got = d.read(&mut buf);
    assert!(
        (22_000..=26_000).contains(&got),
        "the last half second, got {got}"
    );
}

/// Sound read after a jump is the sound a longer read passes at that time.
/// The container rounds each frame's time to a millisecond, so the two may
/// be up to that far apart (48 samples); within that they are the same
/// samples exactly.
#[test]
fn a_jump_gives_the_samples_that_reading_on_reaches() {
    const FROM: f64 = 0.03;
    for (file, stream) in [("game-3-audio.mkv", 1), ("game-3-audio.mkv", 3), ("voice.mp3", 0)] {
        let mut straight = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
        straight.seek(FROM);
        let mut all = vec![0f32; 48_000 * 2];
        assert_eq!(straight.read(&mut all), 48_000, "{file}");
        for at in [0.1, 0.25, 0.5, 0.7313] {
            let mut jumped = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
            jumped.seek(at);
            assert!((jumped.position() - at).abs() < 1e-9);
            let mut buf = vec![0f32; 4_800 * 2];
            assert_eq!(jumped.read(&mut buf), 4_800, "{file} at {at}");
            assert!(
                (jumped.position() - at - 0.1).abs() < 1e-9,
                "{file} at {at}: {}",
                jumped.position()
            );
            let first = ((at - FROM) * 48_000.0_f64).round() as usize;
            let off_by = |shift: usize| {
                let from = (first + shift - 48) * 2;
                buf.iter()
                    .zip(&all[from..from + buf.len()])
                    .map(|(a, b)| (a - b).abs())
                    .fold(0f32, f32::max)
            };
            let least = (0..=96).map(off_by).fold(f32::MAX, f32::min);
            assert!(least < 1e-4, "{file} stream {stream} at {at}: off by {least}");
        }
    }
}
