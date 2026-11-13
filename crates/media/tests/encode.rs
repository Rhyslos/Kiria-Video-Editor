//! Writing a file and reading it back with the crate's own probe and decoder.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use media::encode::{AudioCodec, AudioSettings, Container, VideoCodec, VideoSettings, Writer};
use media::{Decoded, Decoding, VideoDecoder};

const SIZE: (u32, u32) = (320, 180);

/// A grey picture whose brightness says which frame it is.
fn picture(frame: u32) -> Vec<u8> {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut nv12 = vec![(40 + frame * 6) as u8; w * h];
    nv12.extend(std::iter::repeat_n(128u8, w * h / 2));
    nv12
}

fn settings() -> VideoSettings {
    VideoSettings {
        codec: VideoCodec::H264,
        width: SIZE.0,
        height: SIZE.1,
        rate: (30, 1),
        bit_rate: 2_000_000,
        // Whatever machine runs this has a CPU.
        software: true,
    }
}

#[test]
fn a_written_file_has_its_pictures_and_sound_where_they_belong() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.mp4");
    let sound = AudioSettings {
        codec: AudioCodec::Aac,
        sample_rate: 48_000,
        bit_rate: 128_000,
    };
    let mut writer = Writer::create(&path, Container::Mp4, Some(&settings()), Some(&sound)).unwrap();
    assert_eq!(writer.video_encoder(), Some("libx264"));
    assert_eq!(writer.audio_encoder(), Some("aac"));
    assert!(!writer.on_graphics_card());
    // One second: 30 pictures, and a tone handed over in uneven pieces.
    let tone: Vec<f32> = (0..48_000)
        .flat_map(|i| {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            [s, s]
        })
        .collect();
    let mut pieces = tone.chunks(2 * 1601);
    for frame in 0..30 {
        writer.write_picture(&picture(frame)).unwrap();
        if let Some(piece) = pieces.next() {
            writer.write_sound(piece).unwrap();
        }
    }
    for piece in pieces {
        writer.write_sound(piece).unwrap();
    }
    writer.finish().unwrap();

    let media = media::describe(&path).unwrap();
    let video = media.info.video.expect("a picture stream");
    assert_eq!((video.width, video.height), SIZE);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.frame_rate.nominal_fps(), 30);
    assert_eq!(media.info.audio.len(), 1);
    assert_eq!(
        (media.info.audio[0].sample_rate, media.info.audio[0].channels),
        (48_000, 2)
    );
    let seconds = media.duration.as_secs_f64();
    assert!((seconds - 1.0).abs() < 0.08, "{seconds} s");

    // Frame 20 is where frame 20 was put, and as bright as it was made.
    let mut decoder = VideoDecoder::open(&path, video.index, 0, Decoding::Software).unwrap();
    let Decoded::Yuv(frame) = decoder.picture_at(20.0 / 30.0, SIZE.0, SIZE.1).unwrap() else {
        panic!("planes expected");
    };
    assert!((frame.time - 20.0 / 30.0).abs() < 0.001, "at {}", frame.time);
    let luma = frame.into_planes().as_ref()[(SIZE.0 * 90 + 160) as usize];
    assert!(luma.abs_diff(40 + 20 * 6) <= 4, "brightness {luma}");
}

#[test]
fn a_file_without_sound_and_pictures_of_the_wrong_size_are_handled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent.mp4");
    let mut writer = Writer::create(&path, Container::Mp4, Some(&settings()), None).unwrap();
    writer.write_sound(&[0.0; 64]).unwrap(); // nowhere to go: ignored
    assert!(writer.write_picture(&[0u8; 100]).is_err());
    for frame in 0..5 {
        writer.write_picture(&picture(frame)).unwrap();
    }
    writer.finish().unwrap();
    let media = media::describe(&path).unwrap();
    assert!(media.info.audio.is_empty());
    assert!(media.info.video.is_some());

    // Odd sizes can't hold colour at half size.
    let odd = VideoSettings {
        width: 321,
        ..settings()
    };
    let err = Writer::create(&dir.path().join("odd.mp4"), Container::Mp4, Some(&odd), None).unwrap_err();
    assert!(err.report().contains("even"), "{}", err.report());
    // A folder that isn't there.
    let nowhere = dir.path().join("missing").join("out.mp4");
    assert!(Writer::create(&nowhere, Container::Mp4, Some(&settings()), None).is_err());
}

#[test]
fn the_cpu_encoder_is_always_among_those_present() {
    assert!(media::encode::encoders_present(VideoCodec::H264).contains(&"libx264"));
    assert!(media::encode::encoder_description("libx264").is_some());
    // What this machine can encode: at least the processor, for every codec
    // the pinned build has.
    for codec in VideoCodec::ALL {
        let probed = media::encode::probe_video(codec);
        assert!(probed.any(), "{codec:?}: {probed:?}");
    }
    for codec in AudioCodec::ALL {
        assert!(media::encode::audio_encoder(codec).is_some(), "{codec:?}");
    }
}

/// A tone, interleaved stereo, `seconds` long at `rate`.
fn tone(rate: u32, seconds: f64) -> Vec<f32> {
    (0..(f64::from(rate) * seconds) as usize)
        .flat_map(|i| {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
            [s, -s]
        })
        .collect()
}

/// Writes one second (30 pictures at 30 fps when there are pictures) in
/// `container` with `video` and `audio`, and reads it back.
fn written(container: Container, video: Option<VideoCodec>, audio: AudioCodec) -> media::Probe {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("out.{}", container.extension()));
    let rate = audio.sample_rate_for(44_100);
    let sound = AudioSettings {
        codec: audio,
        sample_rate: rate,
        bit_rate: 128_000,
    };
    let picture_settings = video.map(|codec| VideoSettings { codec, ..settings() });
    let mut writer = Writer::create(&path, container, picture_settings.as_ref(), Some(&sound))
        .unwrap_or_else(|err| panic!("{container:?} {video:?} {audio:?}: {}", err.report()));
    let samples = tone(rate, 1.0);
    let mut pieces = samples.chunks(2 * 777);
    for frame in 0..30 {
        if video.is_some() {
            writer.write_picture(&picture(frame)).unwrap();
        }
        if let Some(piece) = pieces.next() {
            writer.write_sound(piece).unwrap();
        }
    }
    for piece in pieces {
        writer.write_sound(piece).unwrap();
    }
    writer.finish().unwrap();
    let probe = media::probe(&path).unwrap();
    // The tone comes back, in both channels, at its own pitch and opposite.
    let stream = probe.info.audio[0].index;
    let mut decoder = media::AudioDecoder::open(&path, stream, rate).unwrap();
    decoder.seek(0.25);
    let mut heard = vec![0.0f32; rate as usize];
    assert_eq!(decoder.read(&mut heard), rate as usize / 2);
    let (left, right): (Vec<f32>, Vec<f32>) = heard.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
    let level = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    let (l, r) = (level(&left), level(&right));
    assert!(
        (l - 0.354).abs() < 0.05 && (r - 0.354).abs() < 0.05,
        "{container:?} {audio:?}: {l} {r}"
    );
    let opposite: f32 = left.iter().zip(&right).map(|(a, b)| a * b).sum();
    assert!(
        opposite < 0.0,
        "{container:?} {audio:?}: the channels are kept apart"
    );
    probe
}

fn codec_name(audio: AudioCodec) -> &'static str {
    match audio {
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "opus",
        AudioCodec::Flac => "flac",
        AudioCodec::Pcm24 => "pcm_s24le",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Vorbis => "vorbis",
    }
}

#[test]
fn every_combination_writes_a_file_that_reads_back() {
    for (container, video, audio) in media::encode::combinations() {
        let probe = written(container, video, audio);
        let sound = &probe.info.audio;
        assert_eq!(sound.len(), 1, "{container:?} {video:?} {audio:?}");
        assert_eq!(sound[0].channels, 2);
        assert_eq!(sound[0].codec, codec_name(audio));
        assert_eq!(sound[0].sample_rate, audio.sample_rate_for(44_100));
        let seconds = probe.duration.as_secs_f64();
        assert!(
            (seconds - 1.0).abs() < 0.1,
            "{container:?} {video:?} {audio:?}: {seconds} s"
        );
        match video {
            Some(codec) => {
                let picture = probe.info.video.as_ref().expect("a picture stream");
                let name = match codec {
                    VideoCodec::H264 => "h264",
                    VideoCodec::H265 => "hevc",
                    VideoCodec::Av1 => "av1",
                };
                assert_eq!(picture.codec, name);
                assert_eq!((picture.width, picture.height), SIZE);
                assert_eq!(picture.frame_rate.nominal_fps(), 30);
            }
            None => assert!(probe.info.video.is_none()),
        }
    }
}

#[test]
fn formats_a_container_cant_hold_are_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let sound = |codec| AudioSettings {
        codec,
        sample_rate: 48_000,
        bit_rate: 128_000,
    };
    let refused = |container, video: Option<&VideoSettings>, audio: Option<&AudioSettings>| {
        Writer::create(&dir.path().join("x"), container, video, audio)
            .unwrap_err()
            .report()
    };
    assert!(
        refused(Container::Mp4, Some(&settings()), Some(&sound(AudioCodec::Flac)))
            .contains("can't hold FLAC")
    );
    assert!(
        refused(Container::Wav, Some(&settings()), Some(&sound(AudioCodec::Pcm24))).contains("sound only")
    );
    assert!(refused(Container::Mkv, None, Some(&sound(AudioCodec::Aac))).contains("no picture settings"));
    assert!(refused(Container::Mp3, None, None).contains("no sound settings"));
    // Opus is only made at the rates it has.
    let odd = AudioSettings {
        sample_rate: 44_100,
        ..sound(AudioCodec::Opus)
    };
    assert!(refused(Container::Ogg, None, Some(&odd)).contains("44100"));
    // Nothing was left behind.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
