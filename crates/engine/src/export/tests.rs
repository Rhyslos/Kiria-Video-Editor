use super::*;
use crate::compose::Composing;
use project::look::ONE;
use project::{Command, Region, SequenceSettings};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name)
}

/// The device a window would be drawn with, here without a window (a
/// software one will do). `None` when this machine has none at all.
fn devices() -> Option<Devices> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Devices {
        decoding: Decoding::Software,
        convert: Convert::WhileDrawing { largest_side: 8192 },
        on_device: None,
        composing: Composing {
            device,
            queue,
            wrap: None,
        },
    })
}

fn recording() -> (Project, SequenceId) {
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    (
        Command::Import { sequence: seq, media }.apply(&project).unwrap(),
        seq,
    )
}

fn wait(export: &Export) -> Outcome {
    let give_up = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(outcome) = export.finished() {
            return outcome;
        }
        assert!(Instant::now() < give_up, "the export did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_stretch_of_the_timeline_becomes_a_file_that_matches_it() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = recording();
    let settings_of = project.active().unwrap().settings;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.mp4");
    let settings = ExportSettings {
        path: path.clone(),
        range: Some((Time::seconds(1), Time::seconds(2))),
        quality: Quality::Small,
        software: true,
    };
    let export = Export::start(project, seq, settings, devices, || {});
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish well");
    };
    let frames = u64::from(settings_of.frame_rate.nominal_fps());
    assert_eq!(done.frames, frames, "one second of frames");
    assert_eq!(done.encoder, "libx264");
    assert!(!done.on_graphics_card);
    assert!((done.length.as_secs_f64() - 1.0).abs() < 0.01);
    let progress = export.progress();
    assert_eq!((progress.frames_done, progress.frames), (frames, frames));
    assert_eq!(progress.share(), 1.0);

    // The file: the timeline's size and rate, a second long, with sound,
    // and nothing left over beside it.
    let made = media::describe(&path).unwrap();
    let video = made.info.video.expect("a picture stream");
    assert_eq!(
        (video.width, video.height),
        (settings_of.width, settings_of.height)
    );
    assert_eq!(
        video.frame_rate.nominal_fps(),
        settings_of.frame_rate.nominal_fps()
    );
    assert_eq!(made.info.audio.len(), 1);
    assert_eq!(made.info.audio[0].sample_rate, settings_of.sample_rate);
    let seconds = made.duration.as_secs_f64();
    assert!((seconds - 1.0).abs() < 0.08, "{seconds} s");
    let beside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(beside, vec![std::ffi::OsString::from("out.mp4")]);

    // Its sound is the timeline's: the same loudness as the mix of that second.
    let mut mixed = vec![0.0f32; 48_000];
    let (project, _) = recording();
    Mixer::new(settings_of.sample_rate).mix(&project, project.active().unwrap(), 1.25, &mut mixed);
    let mut heard = vec![0.0f32; 48_000];
    let mut sound =
        media::AudioDecoder::open(&path, made.info.audio[0].index, settings_of.sample_rate).unwrap();
    sound.seek(0.25);
    assert_eq!(
        sound.read(&mut heard),
        heard.len() / 2,
        "half a second, both channels"
    );
    let loudness =
        |samples: &[f32]| (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let (was, is) = (loudness(&mixed), loudness(&heard));
    assert!(was > 0.001, "the recording has sound there ({was})");
    assert!((is / was - 1.0).abs() < 0.25, "mixed {was}, exported {is}");

    // Its pictures are the timeline's: not black.
    let mut decoder = media::VideoDecoder::open(&path, video.index, 0, Decoding::Software).unwrap();
    let media::Decoded::Yuv(frame) = decoder.picture_at(0.5, video.width, video.height).unwrap() else {
        panic!("planes expected");
    };
    let luma_bytes = (frame.width * frame.height) as usize;
    let planes = frame.into_planes();
    let brightest = planes.as_ref()[..luma_bytes].iter().copied().max().unwrap_or(0);
    assert!(brightest > 40, "brightest {brightest}");
}

#[test]
fn an_exported_fade_and_pan_are_in_the_file() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    // Every sound stream all to the left, fading in over the first second.
    let (mut project, seq) = recording();
    let sounds = project
        .active()
        .unwrap()
        .audio_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| {
            let sound = project::Sound {
                pan: -project::Sound::SIDE,
                fade_in: Time::seconds(1),
                ..project::Sound::PLAIN
            };
            (c.id, sound)
        })
        .collect();
    project = Command::SetSound {
        sequence: seq,
        sounds,
    }
    .apply(&project)
    .unwrap();
    let rate = project.active().unwrap().settings.sample_rate;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("faded.mp4");
    let settings = ExportSettings {
        path: path.clone(),
        range: Some((Time::ZERO, Time::seconds(1))),
        quality: Quality::Small,
        software: true,
    };
    let export = Export::start(project.clone(), seq, settings, devices, || {});
    let Outcome::Done(_) = wait(&export) else {
        panic!("the export did not finish well");
    };
    let made = media::describe(&path).unwrap();
    let mut sound = media::AudioDecoder::open(&path, made.info.audio[0].index, rate).unwrap();
    let mut heard = vec![0.0f32; rate as usize * 2];
    let got = sound.read(&mut heard);
    assert!(got > rate as usize * 9 / 10, "about a second: {got}");
    let side = |which: usize, from: f64, to: f64| {
        let (a, b) = ((from * f64::from(rate)) as usize, (to * f64::from(rate)) as usize);
        let s: Vec<f32> = heard
            .iter()
            .skip(which)
            .step_by(2)
            .skip(a)
            .take(b - a)
            .copied()
            .collect();
        (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt()
    };
    let (left, right) = (side(0, 0.1, 0.9), side(1, 0.1, 0.9));
    assert!(left > 0.01, "the left has the sound: {left}");
    assert!(right < left / 50.0, "the right is silent: {right} against {left}");
    // Rising: the start of the second is much quieter than its end.
    let (early, late) = (side(0, 0.05, 0.2), side(0, 0.8, 0.95));
    assert!(early < late * 0.5, "fading in: {early} then {late}");
    // And as loud as the mix says, a stretch at a time.
    let mut mixed = vec![0.0f32; rate as usize / 5 * 2];
    Mixer::new(rate).mix(&project, project.active().unwrap(), 0.75, &mut mixed);
    let was = (mixed.iter().step_by(2).map(|v| v * v).sum::<f32>() / (mixed.len() / 2) as f32).sqrt();
    let is = side(0, 0.75, 0.95);
    assert!((is / was - 1.0).abs() < 0.25, "mixed {was}, exported {is}");
}

/// The luma plane of the exported picture at `seconds`, with its width.
fn luma_at(path: &Path, seconds: f64) -> (Vec<u8>, usize) {
    let made = media::describe(path).unwrap();
    let video = made.info.video.expect("a picture stream");
    let mut decoder = media::VideoDecoder::open(path, video.index, 0, Decoding::Software).unwrap();
    let media::Decoded::Yuv(frame) = decoder.picture_at(seconds, video.width, video.height).unwrap() else {
        panic!("planes expected");
    };
    let (width, luma_bytes) = (frame.width as usize, (frame.width * frame.height) as usize);
    let planes = frame.into_planes();
    (planes.as_ref()[..luma_bytes].to_vec(), width)
}

/// How bright the left and the right third of a picture are, on average.
fn thirds(luma: &[u8], width: usize) -> (f64, f64) {
    let mean = |from: usize, to: usize| {
        let (mut sum, mut count) = (0u64, 0u64);
        for row in luma.chunks(width) {
            for value in &row[from..to] {
                sum += u64::from(*value);
                count += 1;
            }
        }
        sum as f64 / count.max(1) as f64
    };
    (mean(0, width / 3), mean(width * 2 / 3, width))
}

#[test]
fn an_animated_place_and_opacity_change_across_the_exported_frames() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    // A quarter-size picture going from the left to the right over the
    // first second, from faint to fully opaque.
    let (mut project, seq) = recording();
    let clip = project.active().unwrap().video_tracks[0].clips[0].id;
    let keyed = vec![
        project::LookField::X,
        project::LookField::Y,
        project::LookField::Opacity,
    ];
    for at in [Time::ZERO, Time::seconds(1)] {
        project = Command::Keyframe {
            sequence: seq,
            clip,
            at,
            fields: keyed.clone(),
            change: project::animate::KeyframeChange::Add,
        }
        .apply(&project)
        .unwrap();
    }
    for (at, x, opacity) in [(Time::ZERO, -300_000, 200_000), (Time::seconds(1), 300_000, ONE)] {
        project = Command::SetLookAt {
            sequence: seq,
            clip,
            at,
            look: project::Look {
                x,
                scale: 250_000,
                opacity,
                ..project::Look::PLAIN
            },
        }
        .apply(&project)
        .unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moving.mp4");
    let settings = ExportSettings {
        path: path.clone(),
        range: Some((Time::ZERO, Time::seconds(1))),
        quality: Quality::Small,
        software: true,
    };
    let export = Export::start(project, seq, settings, devices, || {});
    let Outcome::Done(_) = wait(&export) else {
        panic!("the export did not finish well");
    };
    let (early, width) = luma_at(&path, 0.05);
    let (late, _) = luma_at(&path, 0.95);
    let (early_left, early_right) = thirds(&early, width);
    let (late_left, late_right) = thirds(&late, width);
    assert!(
        early_left > early_right + 2.0,
        "on the left first: {early_left} against {early_right}"
    );
    assert!(
        late_right > late_left + 2.0,
        "on the right at the end: {late_right} against {late_left}"
    );
    // Fainter at the start than at the end: the brightest part of the
    // picture is much darker while it is barely opaque.
    let brightest = |luma: &[u8]| luma.iter().copied().max().unwrap_or(0);
    assert!(
        u32::from(brightest(&early)) + 40 < u32::from(brightest(&late)),
        "faint {} then opaque {}",
        brightest(&early),
        brightest(&late)
    );
}

#[test]
fn a_cancelled_export_leaves_nothing_behind() {
    let Some(devices) = devices() else { return };
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        path: dir.path().join("out.mp4"),
        range: None,
        quality: Quality::Small,
        software: true,
    };
    let export = Export::start(project, seq, settings, devices, || {});
    export.cancel();
    assert!(matches!(wait(&export), Outcome::Cancelled));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn an_empty_stretch_is_refused_with_an_explanation() {
    let Some(devices) = devices() else { return };
    let project = Project::new("T", SequenceSettings::GAME_CLIP);
    let seq = project.active_sequence;
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        path: dir.path().join("out.mp4"),
        range: None,
        quality: Quality::Medium,
        software: true,
    };
    let export = Export::start(project, seq, settings, devices, || {});
    let Outcome::Failed(err) = wait(&export) else {
        panic!("an empty timeline exported");
    };
    assert!(
        err.report().contains("nothing on that stretch"),
        "{}",
        err.report()
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn the_frames_exported_are_the_stretch_asked_for() {
    let rate = Rate::FPS_60;
    let ten = Time::seconds(10);
    assert_eq!(frames(ten, None, rate), (0, 600));
    let region = Region::between(Time::seconds(2), Time::seconds(5)).unwrap();
    assert_eq!(frames(ten, Some((region.start, region.end)), rate), (120, 300));
    // A stretch that runs past the end stops at the end.
    assert_eq!(
        frames(ten, Some((Time::seconds(9), Time::seconds(20))), rate),
        (540, 600)
    );
    assert_eq!(
        frames(ten, Some((Time::seconds(30), Time::seconds(40))), rate),
        (600, 600)
    );
    // A timeline that ends inside a frame includes that frame.
    let odd = Time::from_ratio(1001, 1000).unwrap();
    assert_eq!(frames(odd, None, rate), (0, 61));
    assert_eq!(frames(Time::ZERO, None, rate), (0, 0));
}

#[test]
fn quality_and_size_set_the_bit_rate() {
    let hd = (1920, 1080);
    let (high, medium, small) = (
        Quality::High.bit_rate(hd, Rate::FPS_60),
        Quality::Medium.bit_rate(hd, Rate::FPS_60),
        Quality::Small.bit_rate(hd, Rate::FPS_60),
    );
    assert!(high > medium && medium > small);
    // About 14 megabits a second for 1080p 60 at Medium.
    assert!((10_000_000..20_000_000).contains(&medium), "{medium}");
    assert!(Quality::Medium.bit_rate((2560, 1440), Rate::FPS_60) > medium);
    // Half the frames need more than half the bits.
    let thirty = Quality::Medium.bit_rate(hd, Rate::FPS_30);
    assert!(thirty < medium && thirty > medium / 2);
}

#[test]
fn progress_says_how_far_and_how_long_still() {
    let progress = Progress {
        frames_done: 300,
        frames: 1200,
        elapsed: Duration::from_secs(10),
        encoder: None,
    };
    assert_eq!(progress.share(), 0.25);
    assert_eq!(progress.time_left(), Some(Duration::from_secs(30)));
    let early = Progress {
        frames_done: 3,
        ..progress.clone()
    };
    assert_eq!(early.time_left(), None, "no pace to go by yet");
    assert_eq!(
        part_path(Path::new("/videos/My cut.mp4")),
        PathBuf::from("/videos/My cut.exporting.mp4")
    );
}
