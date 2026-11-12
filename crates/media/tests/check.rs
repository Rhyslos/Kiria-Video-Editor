//! Import checks: good files pass, and broken ones are refused with the
//! right explanation. The broken files are made from a good fixture here:
//! cut in half (end missing), its picture's codec name changed to one that
//! doesn't exist (too new), and its data overwritten after the headers
//! (damaged).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use media::{Refusal, check};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn made(dir: &Path, name: &str, change: impl Fn(Vec<u8>) -> Vec<u8>) -> PathBuf {
    let bytes = std::fs::read(fixture("game-3-audio.mkv")).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, change(bytes)).unwrap();
    path
}

fn refusal(path: &Path) -> (Refusal, String) {
    let refused = check(path).expect_err("the file is refused");
    let report = refused.error.report();
    assert!(refused.error.why_text().is_some(), "says why: {report}");
    assert!(!refused.error.fix_text().is_empty(), "says what to do: {report}");
    (refused.refusal, report)
}

#[test]
fn good_files_pass() {
    for name in [
        "game-3-audio.mkv",
        "phone-rotated.mp4",
        "voice.mp3",
        "still.png",
        "vfr-60.mkv",
    ] {
        let media = check(&fixture(name)).unwrap_or_else(|r| panic!("{name}: {}", r.error.report()));
        assert_eq!(media.path, fixture(name));
    }
}

#[test]
fn not_media_and_missing_files_are_explained() {
    assert_eq!(refusal(&fixture("not-media.mkv")).0, Refusal::NotMedia);
    assert_eq!(refusal(&fixture("no-such-file.mkv")).0, Refusal::Unreadable);
}

#[test]
fn a_recording_cut_short_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cut = made(dir.path(), "cut.mkv", |b| b[..b.len() / 2].to_vec());
    let (refusal, report) = refusal(&cut);
    assert_eq!(refusal, Refusal::CutShort, "{report}");
    assert!(report.contains("end is missing"), "{report}");

    // Imported anyway, it is as long as what is really there.
    let whole = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let kept = media::salvage(&cut).unwrap();
    let (real, says) = (kept.duration.as_secs_f64(), kept.info.stated_length.unwrap());
    assert_eq!(says, whole.duration, "it keeps the length the file says it has");
    assert!(
        (0.9..1.2).contains(&real),
        "about the first second is there: {real}"
    );
    let words = media::end_missing_words(&kept).unwrap();
    assert!(words.contains("2.0 s"), "{words}");
    assert!(media::end_missing_words(&whole).is_none());
    assert_eq!(
        media::salvage(&fixture("voice.mp3")).unwrap().info.stated_length,
        None,
        "a whole file stays whole"
    );
}

#[test]
fn an_unknown_codec_is_too_new() {
    let dir = tempfile::tempdir().unwrap();
    let unknown = made(dir.path(), "unknown.mkv", |b| {
        let from = b"V_MPEG4/ISO/AVC";
        let at = b
            .windows(from.len())
            .position(|w| w == from)
            .expect("the codec name");
        let mut b = b;
        b[at..at + from.len()].copy_from_slice(b"V_ZZZZZ/ISO/ZZZ");
        b
    });
    let (refusal, report) = refusal(&unknown);
    assert_eq!(refusal, Refusal::TooNew, "{report}");
    assert!(report.contains("picture"), "says which part: {report}");
}

#[test]
fn a_file_with_broken_data_is_damaged() {
    let dir = tempfile::tempdir().unwrap();
    let garbled = made(dir.path(), "garbled.mkv", |b| {
        // A fixed pattern, so the test is the same every time.
        let mut b = b;
        for (i, byte) in b.iter_mut().enumerate().skip(1500) {
            *byte = (i.wrapping_mul(2_654_435_761) >> 13) as u8;
        }
        b
    });
    let (refusal, report) = refusal(&garbled);
    assert_eq!(refusal, Refusal::Damaged, "{report}");
    assert!(report.contains("first picture"), "{report}");
}
