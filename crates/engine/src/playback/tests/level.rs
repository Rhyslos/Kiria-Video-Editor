//! The level a file is heard at: from the file to the sound card, a stereo
//! stream at default settings (clip, track and master at 0 dB, centred,
//! every effect off) is the file's own samples, left and right, to the bit,
//! whatever the number of channels the card has. Several streams are their
//! sum, nothing turned down to make room; over full scale the card is
//! handed full scale (see `docs/research/sound-quality.md`).

use super::*;
use media::AudioDecoder;

/// A recording laid out as screen recorders write them: five stereo AAC
/// streams of different sound, each wide (left and right unrelated).
fn five_streams() -> Project {
    imported("five-streams.mkv")
}

/// `frames` of `project` as a sound card with `channels` channels is handed
/// them from `from`, after the mixer's fixed delay: its first two channels
/// (left and right) and whether every other channel was silent.
fn heard(project: &Project, from: Time, frames: usize, channels: usize) -> (Vec<f32>, bool) {
    let clock = Arc::new(Clock::new());
    let (mut session, mut output) = prepare(
        project.clone(),
        project.active_sequence,
        from,
        (RATE, channels),
        &clock,
    )
    .unwrap();
    clock.rate.store(u64::from(RATE), Ordering::Release);
    clock.follow_sound();
    let (_send, inbox) = channel();
    let latency = output.latency();
    let mut out = vec![0f32; (frames + latency) * channels];
    for part in out.chunks_mut(480 * channels) {
        session.step(&inbox, &clock);
        output.render(part, channels);
    }
    let mut stereo = Vec::with_capacity(frames * 2);
    let mut rest_silent = true;
    for frame in out.chunks_exact(channels).skip(latency) {
        stereo.extend_from_slice(&frame[..2]);
        rest_silent &= frame[2..].iter().all(|s| *s == 0.0);
    }
    (stereo, rest_silent)
}

/// The file's own samples of `streams` from `from`, added up in track
/// order, as a player decodes them.
fn file_sum(name: &str, streams: &[u32], from: Time, frames: usize) -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name);
    let mut sum = vec![0f32; frames * 2];
    for &stream in streams {
        let mut decoder = AudioDecoder::open(&path, stream, RATE).unwrap();
        decoder.seek_to_sample((from.as_secs_f64() * f64::from(RATE)).round() as i64);
        let mut read = vec![0f32; frames * 2];
        decoder.read(&mut read);
        for (s, r) in sum.iter_mut().zip(&read) {
            *s += r;
        }
    }
    sum
}

fn rms_db(samples: &[f32]) -> f64 {
    let power = samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64;
    10.0 * power.max(1e-30).log10()
}

/// How alike left and right are (1 the same, 0 unrelated).
fn correlation(stereo: &[f32]) -> f64 {
    let (mut lr, mut ll, mut rr) = (0.0, 0.0, 0.0);
    for f in stereo.chunks_exact(2) {
        let (l, r) = (f64::from(f[0]), f64::from(f[1]));
        lr += l * r;
        ll += l * l;
        rr += r * r;
    }
    lr / (ll * rr).sqrt().max(1e-30)
}

/// The file's streams of `project` (its first file) in track order.
fn streams(project: &Project) -> Vec<u32> {
    project
        .active()
        .unwrap()
        .audio_tracks
        .iter()
        .map(|t| t.clips[0].stream)
        .collect()
}

#[test]
fn one_stereo_stream_is_heard_as_the_file_holds_it() {
    let mut project = five_streams();
    // Only the first stream heard.
    for track in project.sequences[0].audio_tracks.iter_mut().skip(1) {
        track.muted = true;
    }
    let first = streams(&project)[0];
    let from = Time::from_ratio(1, 4).unwrap();
    let frames = RATE as usize;
    let file = file_sum("five-streams.mkv", &[first], from, frames);
    for channels in [2, 6, 8] {
        let (played, rest_silent) = heard(&project, from, frames, channels);
        assert!(rms_db(&file) > -30.0, "the stream sounds");
        assert!(
            played == file,
            "{channels} channels: {:+.3} dB against the file, off by up to {}",
            rms_db(&played) - rms_db(&file),
            most_apart(&played, &file)
        );
        assert!(
            rest_silent,
            "only left and right sound on a {channels}-channel card"
        );
    }
    assert!(correlation(&file).abs() < 0.2, "wide: {}", correlation(&file));
}

#[test]
fn five_streams_are_heard_as_their_sum() {
    let project = five_streams();
    let all = streams(&project);
    assert_eq!(all.len(), 5);
    for from in [Time::ZERO, Time::from_ratio(6, 5).unwrap()] {
        let frames = RATE as usize / 2;
        let file = file_sum("five-streams.mkv", &all, from, frames);
        let (played, _) = heard(&project, from, frames, 2);
        let level = rms_db(&played) - rms_db(&file);
        assert!(
            most_apart(&played, &file) < 1e-6,
            "from {from:?}: {level:+.4} dB against the streams' sum, off by up to {}",
            most_apart(&played, &file)
        );
        assert!(level.abs() < 1e-4, "{level} dB");
        assert!(
            (correlation(&played) - correlation(&file)).abs() < 1e-4,
            "as wide as the file"
        );
    }
}

/// Five streams turned up 12 dB each go over full scale: the card is handed
/// full scale there (as it would play it), nothing else changes, and no
/// limiter turns the rest down unless the timeline's master asks for one.
#[test]
fn over_full_scale_the_card_gets_full_scale() {
    let mut project = five_streams();
    for track in project.sequences[0].audio_tracks.iter_mut() {
        track.gain_db = 12.0;
    }
    let from = Time::from_ratio(1, 4).unwrap();
    let frames = RATE as usize / 2;
    let file: Vec<f32> = file_sum("five-streams.mkv", &streams(&project), from, frames);
    let (played, _) = heard(&project, from, frames, 2);
    let louder = crate::mix::volume(12_000);
    let over = played.iter().filter(|s| s.abs() >= 1.0).count();
    assert!(over > 0, "some samples go over");
    assert!(played.iter().all(|s| s.abs() <= 1.0));
    // Under full scale, each sample is the file's sum turned up 12 dB.
    let under = played
        .iter()
        .zip(&file)
        .filter(|(p, _)| p.abs() < 1.0)
        .map(|(p, f)| (p - f * louder).abs())
        .fold(0f32, f32::max);
    assert!(under < 1e-5, "{under}");
}

/// A device mixing more than two channels gets a stereo stream when it
/// takes one; one that won't, or has one or two channels, its own number.
#[test]
fn the_stream_is_stereo_when_the_device_takes_it() {
    assert_eq!(stream_channels(8, || true), 2);
    assert_eq!(stream_channels(6, || true), 2);
    assert_eq!(stream_channels(8, || false), 8);
    assert_eq!(stream_channels(2, || panic!("not asked")), 2);
    assert_eq!(stream_channels(1, || panic!("not asked")), 1);
}
