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

/// Sound read after a jump is exactly the sound a straight read from the
/// start passes at that time. The containers round each frame's time (to a
/// millisecond in MKV); the decoder counts samples instead, so the two agree
/// to the sample. (Opus needs a little sound before the target to settle;
/// the jump decodes half a second before it, so it agrees exactly too.)
#[test]
fn a_jump_gives_the_samples_that_reading_on_reaches() {
    for (file, stream, within) in [
        ("game-3-audio.mkv", 1, 0.0),
        ("game-3-audio.mkv", 3, 0.0),
        ("voice.mp3", 0, 0.0),
        ("sound-offset.mkv", 1, 0.0),
        ("sound-offset.mkv", 2, 0.0),
    ] {
        let mut straight = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
        let mut all = vec![0f32; 48_000 * 2];
        let got = straight.read(&mut all);
        assert!(got > 38_000, "{file}: {got}");
        for at in [0.1, 0.25, 0.5, 0.7313, 0.123_456] {
            let first = (at * 48_000.0_f64).round() as usize;
            let mut jumped = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
            jumped.seek(at);
            assert_eq!(jumped.next_sample(), first as i64);
            let mut buf = vec![0f32; 4_800 * 2];
            // To the end of the file at most.
            let frames = (got - first).min(4_800);
            assert_eq!(jumped.read(&mut buf), frames, "{file} at {at}");
            assert_eq!(jumped.next_sample(), (first + frames) as i64);
            let n = frames * 2;
            let off = buf[..n]
                .iter()
                .zip(&all[first * 2..first * 2 + n])
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(off <= within, "{file} stream {stream} at {at}: off by {off}");
        }
    }
}

/// Which sample of its stream a sample of `sound-offset.mkv` is: its left
/// channel counts up within each tenth of a second, its right channel steps
/// up every tenth. `None` for silence.
fn sample_number(left: f32, right: f32) -> Option<i64> {
    if right == 0.0 {
        return None;
    }
    let tenths = ((f64::from(right) + 0.25) * 64.0).round() as i64;
    let within = ((f64::from(left) + 0.5) * 4_800.0).round() as i64;
    Some(tenths * 4_800 + within)
}

/// `sound-offset.mkv`: the picture starts at 0.1 s, one sound stream at
/// 0 s (heard 0.1 s before the picture's first frame), the other at 0.35 s
/// (0.25 s after it). Read from the picture's start, the first is already
/// 4 800 samples in; the second is silent for 12 000 samples, then starts.
#[test]
fn sound_is_heard_against_the_picture_as_the_file_has_it() {
    for (stream, first_heard) in [(1, 4_800i64), (2, -12_000)] {
        for seek in [false, true] {
            let mut d = AudioDecoder::open(&fixture("sound-offset.mkv"), stream, 48_000).unwrap();
            if seek {
                d.seek(0.0);
            }
            let mut buf = vec![0f32; 24_000 * 2];
            assert_eq!(d.read(&mut buf), 24_000);
            for (i, s) in buf.chunks_exact(2).enumerate() {
                let expected = i as i64 + first_heard;
                let heard = sample_number(s[0], s[1]);
                if expected < 0 {
                    assert_eq!(
                        heard, None,
                        "stream {stream} sample {i}: silence before the sound"
                    );
                } else {
                    assert_eq!(heard, Some(expected), "stream {stream} sample {i} (seek {seek})");
                }
            }
        }
    }
}

/// Read a little at a time, the position is the samples read, exactly,
/// though the file's timestamps are rounded; and the samples are the ones
/// one long read gives.
#[test]
fn reading_counts_samples_not_timestamps() {
    for (file, stream) in [("game-3-audio.mkv", 2), ("sound-offset.mkv", 1)] {
        let mut whole = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
        let mut all = vec![0f32; 38_400 * 2];
        assert_eq!(whole.read(&mut all), 38_400);
        let mut d = AudioDecoder::open(&fixture(file), stream, 48_000).unwrap();
        let mut pieces = Vec::new();
        for block in 0..48 {
            let mut buf = vec![0f32; 800 * 2];
            assert_eq!(d.read(&mut buf), 800);
            assert_eq!(d.next_sample(), (block + 1) * 800, "{file}");
            pieces.extend(buf);
        }
        assert!(pieces == all, "{file}: the same samples");
        assert_eq!(d.seeks(), 0);
    }
}
