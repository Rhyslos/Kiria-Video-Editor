//! Files as the screen and game recorder used beside the app writes them:
//! several named PCM sound streams (one may be a muted source, pure digital
//! silence), files a crash left without a seek index or a length, and a
//! wide 144 fps AV1 picture its encoder padded.

#![allow(clippy::unwrap_used)]

use media::proxy::{Built, Steps};
use media::{AudioDecoder, Decoding, VideoDecoder, check, probe};
use project::{MediaKind, Rate};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A sound stream's whole samples, interleaved stereo at 48 kHz.
fn samples(path: &Path, stream: u32) -> Vec<f32> {
    let mut decoder = AudioDecoder::open(path, stream, 48_000).unwrap();
    let mut all = Vec::new();
    let mut block = vec![0.0f32; 4096];
    loop {
        let got = decoder.read(&mut block);
        if got == 0 {
            return all;
        }
        all.extend_from_slice(&block[..got * 2]);
    }
}

#[test]
fn pcm_streams_import_by_name_and_a_muted_one_is_silent() {
    let path = fixture("recorder-pcm.mkv");
    let media = check(&path).unwrap_or_else(|r| panic!("refused: {}", r.error.report()));
    assert_eq!(media.kind, MediaKind::Video);
    let names: Vec<&str> = media.info.audio.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Game", "Voice chat", "Mic", "Muted"]);
    for stream in &media.info.audio {
        assert_eq!(
            (stream.sample_rate, stream.channels),
            (48_000, 2),
            "{}",
            stream.name
        );
        let all = samples(&path, stream.index);
        // A tenth of a second, both channels.
        assert!(
            all.len().abs_diff(9_600) <= 2 * 1_024,
            "{}: {} samples",
            stream.name,
            all.len()
        );
        let peak = all.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        if stream.name == "Muted" {
            assert_eq!(peak, 0.0, "digital silence stays exactly silent");
        } else {
            assert!(peak > 0.05, "{}: peak {peak}", stream.name);
        }
    }
}

/// The frame number `counted-*` fixtures show in their bars.
fn counted(frame: &media::Frame) -> u32 {
    let width = frame.width as usize;
    (0..8).fold(0, |n, bar| {
        let red = frame.rgba[(width * 16 + bar * 16 + 8) * 4];
        if red > 127 { n | 1 << bar } else { n }
    })
}

#[test]
fn a_file_without_an_index_or_length_imports_and_seeks() {
    // `counted-120.mkv` as a crash leaves a recording: no Cues, no Duration.
    let path = fixture("no-index.mkv");
    let media = check(&path).unwrap_or_else(|r| panic!("refused: {}", r.error.report()));
    let seconds = media.duration.as_secs_f64();
    assert!(
        (seconds - 2.0).abs() < 0.05,
        "the length is found by reading: {seconds} s"
    );
    let mut d = VideoDecoder::open(&path, 0, 0, Decoding::Software).unwrap();
    // Far ahead, back, and ahead again: each is the frame due then.
    for frame in [200u32, 12, 150, 1, 239] {
        let t = f64::from(frame) / 120.0 + 0.001;
        let got = counted(&d.frame_at(t, 128, 32).unwrap());
        assert_eq!(got, frame % 256, "at {t:.3} s");
    }
}

fn has_padding(frame: &media::Frame) -> bool {
    frame
        .rgba
        .chunks_exact(4)
        .any(|p| p[0] > 150 && p[1] < 110 && p[2] < 110)
}

#[test]
fn a_wide_144_fps_av1_recording_decodes_and_gets_a_proxy() {
    // Stored 3456×1440, as the encoder pads 3440, with the crop written.
    let path = fixture("wide-144.mkv");
    let p = probe(&path).unwrap();
    let v = p.info.video.unwrap();
    assert_eq!((v.width, v.height), (3440, 1440));
    assert_eq!(v.frame_rate, Rate::new(144, 1).unwrap());
    assert!(!v.variable_frame_rate);

    let mut d = VideoDecoder::open(&path, 0, 0, Decoding::Best).unwrap();
    for t in [0.0, 3.0 / 144.0, 1.0 / 144.0] {
        let f = d.frame_at(t + 0.0005, 4000, 4000).unwrap();
        assert_eq!((f.width, f.height), (3440, 1440), "at {t}");
        assert!(!has_padding(&f), "at {t}: the padding shows");
    }

    let dir = std::env::temp_dir().join(format!("wide-proxy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("wide.mp4");
    let steps = Steps {
        cancelled: &|| false,
        progress: &|_| {},
        pause: &|| {},
    };
    let length = p.duration.as_secs_f64();
    let built = media::proxy::write(&path, 0, length, &out, Decoding::Software, true, 2, &steps).unwrap();
    let Built::Written(written) = built else {
        panic!("never stopped");
    };
    assert_eq!(written.size, media::proxy::size_for((3440, 1440)));
    let mut proxy = VideoDecoder::open_proxy(&out, 0).unwrap();
    let f = proxy.frame_at(0.0, 4000, 4000).unwrap();
    assert_eq!((f.width, f.height), written.size);
    assert!(!has_padding(&f), "the padding is in the proxy");
    std::fs::remove_dir_all(&dir).unwrap();
}
