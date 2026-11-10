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
