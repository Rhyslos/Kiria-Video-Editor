use super::*;
use crate::compose::Composing;
use crate::mix::Mixer;
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
        range: Some((Time::seconds(1), Time::seconds(2))),
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
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
    // And it was measured as it was written.
    assert!(
        done.loudness.is_finite() && done.loudness < 0.0,
        "{}",
        done.loudness
    );
    assert!(
        done.true_peak.is_finite() && done.true_peak < 1.0,
        "{}",
        done.true_peak
    );

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
        range: Some((Time::ZERO, Time::seconds(1))),
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project.clone(), seq, settings, Some(devices), || {});
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
        range: Some((Time::ZERO, Time::seconds(1))),
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
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

/// Exports the first second of `project`'s timeline `seq`, returning the
/// file's path (kept in `dir`).
fn export_second(project: Project, seq: SequenceId, devices: Devices, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let settings = ExportSettings {
        range: Some((Time::ZERO, Time::seconds(1))),
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let Outcome::Done(_) = wait(&export) else {
        panic!("the export did not finish well");
    };
    path
}

#[test]
fn a_nested_timeline_and_an_adjustment_layer_are_in_the_exported_file() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    // The recording nested in a timeline of its own, picture and sound,
    // and its picture there at half size on the left.
    let (project, seq) = recording();
    let clip = project.active().unwrap().video_tracks[0].clips[0].id;
    let project = Command::NestClips {
        sequence: seq,
        clips: vec![clip],
        name: "Nested".into(),
    }
    .apply(&project)
    .unwrap();
    let nested = project.active().unwrap().video_tracks[0].clips[0].clone();
    assert!(nested.nested().is_some());
    let project = Command::SetLook {
        sequence: seq,
        clip: nested.id,
        look: project::Look {
            x: -250_000,
            scale: 500_000,
            ..project::Look::PLAIN
        },
    }
    .apply(&project)
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let plain = export_second(project.clone(), seq, devices.clone(), dir.path(), "nested.mp4");
    let (luma, width) = luma_at(&plain, 0.5);
    let (left, right) = thirds(&luma, width);
    assert!(
        left > right + 10.0,
        "the nested picture on the left: {left} against {right}"
    );
    assert!(right < 20.0, "black on the right: {right}");
    // The nested timeline's sound is the file's.
    let made = media::describe(&plain).unwrap();
    let rate = project.active().unwrap().settings.sample_rate;
    let mut sound = media::AudioDecoder::open(&plain, made.info.audio[0].index, rate).unwrap();
    let mut heard = vec![0.0f32; rate as usize];
    sound.seek(0.25);
    let got = sound.read(&mut heard);
    let loud = (heard[..got * 2].iter().map(|v| v * v).sum::<f32>() / (got * 2).max(1) as f32).sqrt();
    assert!(loud > 0.001, "the nested sound is heard: {loud}");

    // An adjustment layer two stops darker over the whole second darkens
    // what is below it, the nested picture.
    let project = Command::AddAdjustment {
        sequence: seq,
        at: Time::ZERO,
        length: Time::seconds(2),
        track: None,
    }
    .apply(&project)
    .unwrap();
    let layer = project.active().unwrap().video_tracks[1].clips[0].id;
    let project = Command::SetLook {
        sequence: seq,
        clip: layer,
        look: project::Look {
            exposure: -2 * ONE,
            ..project::Look::PLAIN
        },
    }
    .apply(&project)
    .unwrap();
    let darker = export_second(project, seq, devices, dir.path(), "adjusted.mp4");
    let (luma, width) = luma_at(&darker, 0.5);
    let (dark_left, _) = thirds(&luma, width);
    assert!(
        dark_left < left * 0.8,
        "darker under the adjustment layer: {dark_left} against {left}"
    );
}

#[test]
fn a_cancelled_export_leaves_nothing_behind() {
    let Some(devices) = devices() else { return };
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        range: None,
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(dir.path().join("out.mp4"))
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
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
        range: None,
        quality: Quality::Medium,
        software: true,
        ..ExportSettings::mp4(dir.path().join("out.mp4"))
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
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
        measuring: false,
        paused: false,
    };
    assert_eq!(progress.share(), 0.25);
    assert_eq!(progress.time_left(), Some(Duration::from_secs(30)));
    let early = Progress {
        frames_done: 3,
        ..progress.clone()
    };
    assert_eq!(early.time_left(), None, "no pace to go by yet");
    let paused = Progress {
        paused: true,
        ..progress.clone()
    };
    assert_eq!(paused.time_left(), None, "paused: no time left to say");
    assert_eq!(
        part_path(Path::new("/videos/My cut.mp4")),
        PathBuf::from("/videos/My cut.exporting.mp4")
    );
}

/// A 128×32, 120 fps recording, two seconds long, whose every frame shows
/// its own number in binary: eight bars 16 pixels wide, the first the
/// lowest bit, white for 1 and black for 0.
fn counted() -> (Project, SequenceId) {
    let settings = SequenceSettings {
        width: 128,
        height: 32,
        frame_rate: project::Rate::FPS_60,
        sample_rate: 48_000,
    };
    let project = Project::new("T", settings);
    let seq = project.active_sequence;
    let media = media::describe(&fixture("counted-120.mkv")).unwrap();
    (
        Command::Import { sequence: seq, media }.apply(&project).unwrap(),
        seq,
    )
}

/// How bright each of the eight bars is in the middle of the picture.
fn bars(luma: &[u8], width: usize) -> [u8; 8] {
    let row = &luma[width * 16..width * 17];
    std::array::from_fn(|bar| row[bar * 16 + 8])
}

/// The frame number the bars say, when every bar is clearly black or white.
fn number(bars: [u8; 8]) -> Option<u32> {
    bars.iter().enumerate().try_fold(0, |n, (bit, luma)| match luma {
        0..=50 => Some(n),
        200.. => Some(n | 1 << bit),
        _ => None,
    })
}

/// Every frame of the exported `path` (`count` of them at 60 fps), as bars.
fn exported_bars(path: &Path, count: u64) -> Vec<[u8; 8]> {
    let made = media::describe(path).unwrap();
    let video = made.info.video.expect("a picture stream");
    let mut decoder = media::VideoDecoder::open(path, video.index, 0, Decoding::Software).unwrap();
    (0..count)
        .map(|frame| {
            let at = (frame as f64 + 0.5) / 60.0;
            let media::Decoded::Yuv(picture) = decoder.picture_at(at, video.width, video.height).unwrap()
            else {
                panic!("planes expected");
            };
            let width = picture.width as usize;
            let luma = picture.into_planes();
            bars(luma.as_ref(), width)
        })
        .collect()
}

fn export_of(
    project: Project,
    seq: SequenceId,
    devices: Devices,
    frames: i64,
    name: &str,
) -> (PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    let settings = ExportSettings {
        range: Some((Time::ZERO, project::Rate::FPS_60.frame_start(frames))),
        quality: Quality::Small,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish well");
    };
    assert_eq!(done.frames, frames as u64);
    (path, dir)
}

#[test]
fn export_never_reads_proxies() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = counted();
    let recording = project.media.values().next().unwrap().clone();
    // A "proxy" of the counted recording that shows something else
    // altogether (another file's picture), with proxies On.
    let dir = tempfile::tempdir().unwrap();
    let shelf = cache::proxy::Shelf::new(dir.path(), cache::proxy::ProxyMode::On);
    let other = fixture("colour-709-full.mp4");
    let steps = media::proxy::Steps {
        cancelled: &|| false,
        progress: &|_| {},
        pause: &|| {},
    };
    shelf
        .build(
            &other,
            0,
            1.0,
            &recording.fingerprint,
            Decoding::Software,
            1,
            &steps,
        )
        .unwrap();
    // The viewer's worker, handed the shelf, shows it.
    let mut preview = crate::Preview::start(
        Decoding::Software,
        Convert::WhileDrawing { largest_side: 8192 },
        None,
        || {},
    );
    preview.set_proxies(Some(std::sync::Arc::clone(&shelf)));
    preview.show(&project, seq, Time::ZERO, 128, 32);
    let shown = preview.wait(Duration::from_secs(20)).expect("a picture");
    assert_eq!(shown.method.as_deref(), Some(crate::preview::PROXY_ONLY));
    // The export of the same timeline reads the recording itself: every
    // frame shows its own number (two recorded frames a timeline frame).
    let (path, _out) = export_of(project, seq, devices, 12, "out.mp4");
    let numbers: Vec<Option<u32>> = exported_bars(&path, 12).into_iter().map(number).collect();
    let expected: Vec<Option<u32>> = (0..12).map(|frame| Some(frame * 2)).collect();
    assert_eq!(numbers, expected);
}

#[test]
fn a_speed_ramp_exports_exactly_the_recorded_frames_it_reaches() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = counted();
    let clip = project.active().unwrap().video_tracks[0].clips[0].id;
    // Full speed for half a second (two recorded frames a timeline frame),
    // easing to half speed by one second (then one recorded frame a
    // timeline frame: clean slow motion from 120 fps on 60 fps).
    let ramp = project::Ramp::new([
        project::SpeedKey {
            at: Time::from_ratio(1, 2).unwrap(),
            speed: project::Speed::NORMAL,
            ease: project::Ease::Smooth,
        },
        project::SpeedKey {
            at: Time::seconds(1),
            speed: project::Speed::new(1, 2).unwrap(),
            ease: project::Ease::Linear,
        },
    ]);
    let project = Command::SetRamp {
        sequence: seq,
        clip,
        ramp,
    }
    .apply(&project)
    .unwrap();
    let ramped = project.active().unwrap().video_tracks[0].clips[0].clone();
    let frames = 120;
    let (path, _dir) = export_of(project, seq, devices, frames, "ramp.mp4");
    let shown = exported_bars(&path, frames as u64);
    let rate = project::Rate::FPS_60;
    for (frame, bars) in shown.iter().enumerate() {
        let source = ramped.source_at(rate.frame_start(frame as i64)).as_secs_f64();
        // The last recorded frame due by then. The file keeps times to the
        // millisecond, and the decoder counts a frame stored up to half a
        // millisecond late as due (its timestamps' own slack).
        let stored = |k: u32| (f64::from(k) * 1000.0 / 120.0).round() / 1000.0;
        let expected = (0..240)
            .take_while(|k| stored(*k) <= source + 0.0005 + 1e-9)
            .last()
            .unwrap_or(0);
        assert_eq!(number(*bars), Some(expected), "timeline frame {frame}: {bars:?}");
    }
    // Every other recorded frame at first, then every one.
    assert_eq!(number(shown[10]), Some(20));
    let slow: Vec<u32> = shown[70..76].iter().filter_map(|b| number(*b)).collect();
    assert!(slow.windows(2).all(|pair| pair[1] == pair[0] + 1), "{slow:?}");
}

#[test]
fn frame_blending_mixes_the_recorded_frames_around_each_moment() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = counted();
    let clip = project.active().unwrap().video_tracks[0].clips[0].id;
    // A quarter speed: two timeline frames a recorded one, the second
    // half way to the next recorded frame.
    let slow = Command::SetSpeed {
        sequence: seq,
        clip,
        speed: project::Speed::new(1, 4).unwrap(),
    }
    .apply(&project)
    .unwrap();
    let blended = Command::SetFrameBlend {
        sequence: seq,
        clip,
        blend: project::FrameBlend::Blend,
    }
    .apply(&slow)
    .unwrap();
    let (path, _dir) = export_of(blended, seq, devices, 8, "blended.mp4");
    let shown = exported_bars(&path, 8);
    for (frame, bars) in shown.iter().enumerate() {
        if frame % 2 == 0 {
            assert_eq!(
                number(*bars),
                Some(frame as u32 / 2),
                "on a recorded frame: {bars:?}"
            );
        } else {
            // The lowest bit differs between two neighbouring frames: half
            // of white's light, about 0.75 of the way up in video values.
            assert!((140..=215).contains(&bars[0]), "frame {frame} blended: {bars:?}");
        }
    }
}

#[test]
fn an_export_starts_on_the_exact_frame_never_the_key_frame_before_it() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    // The counted recording has a key frame every 30 recorded frames; the
    // stretch starts at recorded frame 82, between those at 60 and 90.
    let (project, seq) = counted();
    let rate = project::Rate::FPS_60;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("late.mp4");
    let settings = ExportSettings {
        range: Some((rate.frame_start(41), rate.frame_start(48))),
        quality: Quality::High,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish well");
    };
    assert_eq!(done.frames, 7);
    let numbers: Vec<Option<u32>> = exported_bars(&path, 7).into_iter().map(number).collect();
    let expected: Vec<Option<u32>> = (41..48).map(|frame| Some(frame * 2)).collect();
    assert_eq!(numbers, expected);
}

/// A one-second recording, 192×108 at 30 fps, of stripes one pixel wide,
/// alternately black and white: detail that any smaller picture loses.
fn stripes(dir: &Path) -> PathBuf {
    use media::encode::{VideoSettings, Writer};
    let path = dir.join("stripes.mkv");
    let video = VideoSettings {
        codec: VideoCodec::H264,
        width: 192,
        height: 108,
        rate: (30, 1),
        bit_rate: 8_000_000,
        software: true,
    };
    let mut writer = Writer::create(&path, Container::Mkv, Some(&video), None).unwrap();
    let mut picture: Vec<u8> = (0..192 * 108)
        .map(|i| if i % 2 == 0 { 16 } else { 235 })
        .collect();
    picture.extend(std::iter::repeat_n(128u8, 192 * 108 / 2));
    for _ in 0..30 {
        writer.write_picture(&picture).unwrap();
    }
    writer.finish().unwrap();
    path
}

#[test]
fn the_exported_picture_is_the_recording_at_full_size() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    // Whatever the viewer shows (a smaller picture to keep playing, a
    // proxy, a key frame first), the export is the recording at the
    // timeline's own size, pixel for pixel: stripes one pixel wide stay
    // stripes, where any smaller picture would make them grey.
    let dir = tempfile::tempdir().unwrap();
    let source = stripes(dir.path());
    let settings = SequenceSettings {
        width: 192,
        height: 108,
        frame_rate: project::Rate::FPS_30,
        sample_rate: 48_000,
    };
    let project = Project::new("T", settings);
    let seq = project.active_sequence;
    let media = media::describe(&source).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let path = dir.path().join("full.mp4");
    let settings = ExportSettings {
        quality: Quality::High,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let Outcome::Done(_) = wait(&export) else {
        panic!("the export did not finish well");
    };
    let (exported, width) = luma_at(&path, 0.5);
    let (original, original_width) = luma_at(&source, 0.5);
    assert_eq!((width, original_width), (192, 192));
    let apart = exported
        .iter()
        .zip(&original)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / exported.len() as f64;
    assert!(apart < 12.0, "on average {apart} levels apart");
    let contrast = exported
        .windows(2)
        .map(|pair| f64::from(pair[0].abs_diff(pair[1])))
        .sum::<f64>()
        / (exported.len() - 1) as f64;
    assert!(contrast > 150.0, "neighbours only {contrast} levels apart");
}

/// A 2-second recording, 192×108 at 30 fps, black but for frames 30 to 32,
/// which are white, with silence but for a tone from exactly 1 s to 1.1 s:
/// written losslessly enough (H.264 at a high rate, FLAC) to find both.
fn flash_and_beep(dir: &Path) -> PathBuf {
    use media::encode::{AudioSettings, VideoSettings, Writer};
    let path = dir.join("flash.mkv");
    let video = VideoSettings {
        codec: VideoCodec::H264,
        width: 192,
        height: 108,
        rate: (30, 1),
        bit_rate: 4_000_000,
        software: true,
    };
    let audio = AudioSettings {
        codec: AudioCodec::Flac,
        sample_rate: 48_000,
        bit_rate: 0,
    };
    let mut writer = Writer::create(&path, Container::Mkv, Some(&video), Some(&audio)).unwrap();
    for frame in 0..60 {
        let luma = if (30..33).contains(&frame) { 235 } else { 16 };
        let mut picture = vec![luma; 192 * 108];
        picture.extend(std::iter::repeat_n(128u8, 192 * 108 / 2));
        writer.write_picture(&picture).unwrap();
        let sound: Vec<f32> = (frame * 1600..(frame + 1) * 1600)
            .flat_map(|i| {
                let s = if (48_000..52_800).contains(&i) {
                    (i as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5
                } else {
                    0.0
                };
                [s, s]
            })
            .collect();
        writer.write_sound(&sound).unwrap();
    }
    writer.finish().unwrap();
    path
}

/// The moment the sound in `path` first gets loud, in seconds.
fn beep_at(path: &Path) -> f64 {
    let made = media::describe(path).unwrap();
    let stream = &made.info.audio[0];
    assert_eq!(stream.channels, 2, "{}", path.display());
    let rate = stream.sample_rate;
    let mut sound = media::AudioDecoder::open(path, stream.index, rate).unwrap();
    let mut heard = vec![0.0f32; rate as usize * 2 * 2];
    let got = sound.read(&mut heard);
    let first = heard[..got * 2]
        .chunks_exact(2)
        .position(|pair| pair[0].abs() > 0.1 && pair[1].abs() > 0.1)
        .expect("the beep");
    first as f64 / f64::from(rate)
}

/// The moment the picture in `path` first turns white, and how many frames it has.
fn flash_at(path: &Path) -> (f64, u32) {
    let made = media::describe(path).unwrap();
    let video = made.info.video.expect("a picture stream");
    let mut decoder = media::VideoDecoder::open(path, video.index, 0, Decoding::Software).unwrap();
    let mut flash = None;
    let mut frames = 0;
    for frame in 0..80u32 {
        let Ok(media::Decoded::Yuv(picture)) =
            decoder.picture_at((f64::from(frame) + 0.5) / 30.0, video.width, video.height)
        else {
            break;
        };
        // Past the end the last frame comes again.
        if (picture.time - f64::from(frame) / 30.0).abs() > 0.002 {
            break;
        }
        frames += 1;
        let middle = picture.into_planes().as_ref()[(108 / 2) * 192 + 96];
        if middle > 128 && flash.is_none() {
            flash = Some(f64::from(frame) / 30.0);
        }
    }
    (flash.expect("the flash"), frames)
}

#[test]
fn every_format_this_build_encodes_exports_in_sync() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let encoders = Encoders::detect();
    let all = encoders.combinations();
    // The pinned FFmpeg build has an encoder for every one of them.
    assert_eq!(all, combinations());
    let dir = tempfile::tempdir().unwrap();
    let source = flash_and_beep(dir.path());
    let settings = SequenceSettings {
        width: 192,
        height: 108,
        frame_rate: project::Rate::FPS_30,
        sample_rate: 44_100,
    };
    let project = Project::new("T", settings);
    let seq = project.active_sequence;
    let media = media::describe(&source).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    for (container, video, audio) in all {
        let what = format!("{container:?} {video:?} {audio:?}");
        let path = dir.path().join(format!("out.{}", container.extension()));
        let settings = ExportSettings {
            path: path.clone(),
            range: None,
            container,
            video,
            audio,
            quality: Quality::Small,
            software: true,
            normalise: None,
            limit: false,
            size: None,
            rate: None,
            sound_beside: false,
        };
        let export = Export::start(project.clone(), seq, settings, Some(devices.clone()), || {});
        let done = match wait(&export) {
            Outcome::Done(done) => done,
            Outcome::Failed(err) => panic!("{what}: {}", err.report()),
            Outcome::Cancelled => panic!("{what}: cancelled"),
        };
        assert_eq!(done.frames, 60, "{what}");
        assert!(!done.on_graphics_card, "{what}");
        let made = media::describe(&path).unwrap();
        let seconds = made.duration.as_secs_f64();
        assert!((seconds - 2.0).abs() < 0.08, "{what}: {seconds} s");
        assert_eq!(made.info.audio.len(), 1, "{what}");
        assert_eq!(
            made.info.audio[0].sample_rate,
            audio.sample_rate_for(44_100),
            "{what}"
        );
        let beep = beep_at(&path);
        assert!((beep - 1.0).abs() < 0.005, "{what}: the beep at {beep} s");
        match video {
            Some(_) => {
                let (flash, frames) = flash_at(&path);
                assert_eq!(frames, 60, "{what}");
                assert!((flash - 1.0).abs() < 0.001, "{what}: the flash at {flash} s");
                assert!(
                    (beep - flash).abs() < 0.5 / 30.0,
                    "{what}: {beep} against {flash}"
                );
            }
            None => assert!(made.info.video.is_none(), "{what}"),
        }
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn a_sound_only_file_needs_no_graphics_card() {
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sound.wav");
    let settings = ExportSettings {
        range: Some((Time::seconds(1), Time::seconds(2))),
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project.clone(), seq, settings, None, || {});
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish well");
    };
    assert_eq!(done.encoder, "pcm_s24le");
    assert!(done.loudness.is_finite());
    let made = media::describe(&path).unwrap();
    assert!(made.info.video.is_none());
    assert_eq!(made.info.audio[0].codec, "pcm_s24le");
    // A video without a graphics card is refused with the way out.
    let settings = ExportSettings::mp4(dir.path().join("video.mp4"));
    let export = Export::start(project, seq, settings, None, || {});
    let Outcome::Failed(err) = wait(&export) else {
        panic!("a video can't be made without a graphics card");
    };
    assert!(err.report().contains("sound only"), "{}", err.report());
}

#[test]
fn settings_that_dont_go_together_are_refused_before_anything_is_written() {
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        audio: AudioCodec::Flac,
        ..ExportSettings::mp4(dir.path().join("x.mp4"))
    };
    let err = settings.check().unwrap_err();
    assert!(
        err.report().contains("can't hold H.264 pictures with FLAC sound"),
        "{}",
        err.report()
    );
    assert!(err.report().contains("AAC, Opus"), "{}", err.report());
    let export = Export::start(project, seq, settings, None, || {});
    assert!(matches!(wait(&export), Outcome::Failed(_)));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn each_codec_and_quality_has_its_bit_rate_and_size() {
    let hd = (1920, 1080);
    let h264 = Quality::Medium.video_bit_rate(VideoCodec::H264, hd, Rate::FPS_60);
    assert_eq!(h264, Quality::Medium.bit_rate(hd, Rate::FPS_60));
    let h265 = Quality::Medium.video_bit_rate(VideoCodec::H265, hd, Rate::FPS_60);
    let av1 = Quality::Medium.video_bit_rate(VideoCodec::Av1, hd, Rate::FPS_60);
    assert!(av1 < h265 && h265 < h264);
    for codec in AudioCodec::ALL {
        let rates: Vec<Option<u64>> = Quality::ALL.iter().map(|q| q.audio_bit_rate(codec)).collect();
        if codec.lossless() {
            assert!(rates.iter().all(Option::is_none));
        } else {
            assert!(rates[0] > rates[1] && rates[1] > rates[2], "{codec:?}");
        }
    }
    // Ten minutes of 1080p 60 H.264 with AAC at Medium: about 1.1 GB.
    let ten = Duration::from_secs(600);
    let mp4 = ExportSettings::mp4(PathBuf::from("a.mp4"));
    let bytes = mp4.estimated_bytes(hd, Rate::FPS_60, 48_000, ten);
    assert_eq!(bytes, (h264 + 192_000) * 600 / 8);
    // A WAV of the same: 24-bit stereo at 48 kHz, 172.8 MB.
    let wav = ExportSettings {
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        ..mp4
    };
    assert_eq!(wav.estimated_bytes(hd, Rate::FPS_60, 48_000, ten), 172_800_000);
}

#[test]
fn encoders_are_said_by_where_they_work() {
    assert_eq!(encoder_place("libx265"), Place::Processor);
    assert_eq!(encoder_place("av1_nvenc"), Place::GraphicsCard);
    assert_eq!(encoder_place("hevc_mf"), Place::System);
    assert_eq!(encoder_place("aac"), Place::Processor);
}

/// The recording's sound turned up by `db`, as a project.
fn louder(db: i32) -> (Project, SequenceId) {
    let (project, seq) = recording();
    let sounds = project
        .active()
        .unwrap()
        .audio_tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| {
            let sound = project::Sound {
                gain_mdb: db * 1000,
                ..project::Sound::PLAIN
            };
            (c.id, sound)
        })
        .collect();
    let project = Command::SetSound {
        sequence: seq,
        sounds,
    }
    .apply(&project)
    .unwrap();
    (project, seq)
}

/// Exports two seconds of `project` as WAV with these loudness settings.
fn wav(
    project: Project,
    seq: SequenceId,
    normalise: Option<f32>,
    limit: bool,
) -> (Exported, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sound.wav");
    let settings = ExportSettings {
        range: Some((Time::ZERO, Time::seconds(2))),
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        normalise,
        limit,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, None, || {});
    match wait(&export) {
        Outcome::Done(done) => (done, path, dir),
        Outcome::Failed(err) => panic!("{}", err.report()),
        Outcome::Cancelled => panic!("cancelled"),
    }
}

/// The integrated loudness and highest true peak of a sound file, decoded.
fn measured(path: &Path) -> (f32, f32) {
    let made = media::describe(path).unwrap();
    let stream = &made.info.audio[0];
    let mut sound = media::AudioDecoder::open(path, stream.index, stream.sample_rate).unwrap();
    let mut meter = crate::loudness::Loudness::new(stream.sample_rate);
    let mut block = vec![0.0f32; 4096];
    loop {
        let got = sound.read(&mut block);
        if got == 0 {
            break;
        }
        meter.feed(&block[..got * 2]);
    }
    (meter.measured().integrated, meter.measured().true_peak_max)
}

#[test]
fn normalising_brings_the_sound_to_the_loudness_aimed_at_under_the_ceiling() {
    for (db, target) in [(0, -14.0f32), (0, -23.0), (12, -14.0)] {
        let (project, seq) = louder(db);
        let (plain, _, _plain_dir) = wav(project.clone(), seq, None, false);
        let (done, path, _dir) = wav(project, seq, Some(target), true);
        let normalised = done.normalised.expect("normalised");
        // The first pass measured what an export without it writes.
        assert!(
            (normalised.measured - plain.loudness).abs() < 0.05,
            "{} against {}",
            normalised.measured,
            plain.loudness
        );
        assert!((normalised.gain_db - (target - plain.loudness)).abs() < 0.01);
        // What was written is on target (the limiter may take a little
        // off) and its true peak under the ceiling, as written and as the
        // file reads back.
        assert!(
            (done.loudness - target).abs() < 1.0,
            "{db} dB louder, aiming at {target}: {} LUFS",
            done.loudness
        );
        assert!(done.true_peak <= TRUE_PEAK_CEILING, "{} dBTP", done.true_peak);
        let (loudness, peak) = measured(&path);
        assert!((loudness - done.loudness).abs() < 0.05, "{loudness} read back");
        assert!(peak <= TRUE_PEAK_CEILING + 0.01, "{peak} dBTP read back");
        assert!(done.limited_db.is_some());
    }
}

#[test]
fn without_normalising_the_mix_is_exported_sample_for_sample() {
    let (project, seq) = louder(12);
    let (plain, path, _dir) = wav(project.clone(), seq, None, false);
    assert!(plain.normalised.is_none() && plain.limited_db.is_none());
    // Twelve decibels up, the recording peaks over full scale: left as it is.
    assert!(plain.true_peak > 0.0, "{}", plain.true_peak);
    let rate = project.active().unwrap().settings.sample_rate;
    let made = media::describe(&path).unwrap();
    let mut sound = media::AudioDecoder::open(&path, made.info.audio[0].index, rate).unwrap();
    let mut heard = vec![0.0f32; rate as usize * 2];
    sound.read(&mut heard);
    // Mixed a frame at a time (60 a second), as the export mixes.
    let mut mixed = vec![0.0f32; rate as usize * 2];
    let mut mixer = Mixer::new(rate);
    for (n, chunk) in mixed.chunks_mut(1600).enumerate() {
        mixer.mix(
            &project,
            project.active().unwrap(),
            n as f64 * 800.0 / f64::from(rate),
            chunk,
        );
    }
    let off = heard
        .iter()
        .zip(&mixed)
        .map(|(a, b)| (a - b.clamp(-1.0, 1.0)).abs())
        .fold(0f32, f32::max);
    assert!(off < 1e-6, "{off}");
    // The limiter alone keeps it under the ceiling, the loudness about as it was.
    let (limited, _, _limited_dir) = wav(project, seq, None, true);
    assert!(limited.true_peak <= TRUE_PEAK_CEILING, "{}", limited.true_peak);
    assert!(limited.limited_db.is_some_and(|db| db < 0.0));
}

#[test]
fn normalising_silence_or_quiet_turns_it_up_no_more_than_allowed() {
    assert_eq!(Normalised::for_target(f32::NEG_INFINITY, -14.0).gain_db, 0.0);
    assert_eq!(Normalised::for_target(-60.0, -14.0).gain_db, MOST_GAIN_DB);
    assert_eq!(Normalised::for_target(-10.0, -14.0).gain_db, -4.0);
}

#[test]
fn a_video_export_measures_first_then_writes_on_target() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loud.mp4");
    let settings = ExportSettings {
        range: Some((Time::seconds(1), Time::seconds(2))),
        software: true,
        normalise: Some(-16.0),
        limit: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish well");
    };
    assert!(done.normalised.is_some());
    assert!((done.loudness + 16.0).abs() < 1.0, "{}", done.loudness);
    assert!(done.true_peak <= TRUE_PEAK_CEILING);
    assert!(!export.progress().measuring);
    let (exported, _) = measured(&path);
    assert!((exported + 16.0).abs() < 1.0, "{exported} read back");
}

/// The exported sound of a recording, mixed a frame at a time, is its
/// sound streams each read straight through and added up, to the sample
/// (to the 24-bit file's rounding): from the start and from the middle.
/// The recording's Opus sound is timed in rounded milliseconds; that never
/// makes the mix jump.
#[test]
fn exported_sound_is_the_recording_read_straight() {
    let (project, seq) = recording();
    let rate = project.active().unwrap().settings.sample_rate;
    let media = project.media.values().next().unwrap().clone();
    for from in [Time::ZERO, Time::from_ratio(7, 10).unwrap()] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sound.wav");
        let settings = ExportSettings {
            range: Some((from, from + Time::seconds(1))),
            container: Container::Wav,
            video: None,
            audio: AudioCodec::Pcm24,
            normalise: None,
            limit: false,
            ..ExportSettings::mp4(path.clone())
        };
        let export = Export::start(project.clone(), seq, settings, None, || {});
        if let Outcome::Failed(err) = wait(&export) {
            panic!("{}", err.report());
        }
        let mut written = media::AudioDecoder::open(&path, 0, rate).unwrap();
        let mut heard = vec![0f32; rate as usize * 2];
        assert_eq!(written.read(&mut heard), rate as usize);
        let mut sum = vec![0f32; heard.len()];
        for stream in &media.info.audio {
            let mut straight = media::AudioDecoder::open(&media.path, stream.index, rate).unwrap();
            if from != Time::ZERO {
                straight.seek(from.as_secs_f64());
            }
            let mut read = vec![0f32; heard.len()];
            assert_eq!(straight.read(&mut read), rate as usize);
            for (s, r) in sum.iter_mut().zip(&read) {
                *s += r;
            }
        }
        let off = heard
            .iter()
            .zip(&sum)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(off < 1e-6, "from {from:?}: off by {off}");
    }
}

#[test]
fn the_picture_is_fitted_into_another_shape_with_even_black_bars() {
    // The same shape: the whole frame.
    assert_eq!(picture_size((1920, 1080), (1280, 720)), (1280, 720));
    assert_eq!(picture_size((1920, 1080), (3840, 2160)), (3840, 2160));
    // Wide into tall: bars above and below, an even margin each side.
    let (w, h) = picture_size((1920, 1080), (1080, 1920));
    assert_eq!(w, 1080);
    assert!((606..=612).contains(&h), "{h}");
    assert_eq!(((1920 - h) / 2) % 2, 0, "the margin is even");
    assert_eq!(h % 2, 0);
    // Tall into wide: bars at the sides.
    let (w, h) = picture_size((1080, 1920), (1920, 1080));
    assert_eq!(h, 1080);
    assert!((604..=612).contains(&w), "{w}");
    // Odd sizes asked for are made even.
    assert_eq!(picture_size((1921, 1081), (1281, 721)), (1280, 720));

    // A 4×2 picture in an 8×6 frame: black around it, colour lined up.
    let mut picture = vec![200u8; 8];
    picture.extend([10, 20, 30, 40]);
    let mut out = Vec::new();
    letterbox(&picture, (4, 2), (8, 6), &mut out);
    assert_eq!(out.len(), 8 * 6 * 3 / 2);
    let luma = &out[..48];
    for (row, line) in luma.chunks(8).enumerate() {
        let expected: Vec<u8> = (0..8)
            .map(|x| {
                if row == 2 || row == 3 {
                    if (2..6).contains(&x) { 200 } else { 16 }
                } else {
                    16
                }
            })
            .collect();
        assert_eq!(line, expected.as_slice(), "row {row}");
    }
    let chroma = &out[48..];
    assert_eq!(chroma[..8], [128; 8]);
    assert_eq!(chroma[8..16], [128, 128, 10, 20, 30, 40, 128, 128]);
    assert_eq!(chroma[16..], [128; 8]);
}

#[test]
fn an_export_at_another_size_and_rate_has_the_timeline_inside_bars() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let (project, seq) = recording();
    let timeline = project.active().unwrap().settings;
    assert!(timeline.width > timeline.height, "a wide recording");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vertical.mp4");
    let settings = ExportSettings {
        range: Some((Time::seconds(1), Time::seconds(2))),
        quality: Quality::Small,
        software: true,
        size: Some((180, 320)),
        rate: Some(Rate::FPS_30),
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let done = match wait(&export) {
        Outcome::Done(done) => done,
        Outcome::Failed(err) => panic!("{}", err.report()),
        Outcome::Cancelled => panic!("cancelled"),
    };
    assert_eq!(done.frames, 30, "a second at 30 frames a second");
    assert_eq!(done.size, Some((180, 320)));
    let made = media::describe(&path).unwrap();
    let video = made.info.video.expect("a picture stream");
    assert_eq!((video.width, video.height), (180, 320));
    assert_eq!(video.frame_rate.nominal_fps(), 30);
    let seconds = made.duration.as_secs_f64();
    assert!((seconds - 1.0).abs() < 0.08, "{seconds} s");
    // Black bars above and below, the picture between.
    let (luma, width) = luma_at(&path, 0.5);
    let row_mean = |row: usize| {
        luma[row * width..(row + 1) * width]
            .iter()
            .map(|&v| f64::from(v))
            .sum::<f64>()
            / width as f64
    };
    let top = (0..40).map(row_mean).sum::<f64>() / 40.0;
    let bottom = (280..320).map(row_mean).sum::<f64>() / 40.0;
    assert!(top < 20.0 && bottom < 20.0, "bars: {top} and {bottom}");
}

#[test]
fn the_sound_beside_is_the_same_samples_as_a_wav_export() {
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let range = Some((Time::seconds(1), Time::seconds(2)));
    let plain = dir.path().join("plain.wav");
    let only = ExportSettings {
        range,
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        ..ExportSettings::mp4(plain.clone())
    };
    assert!(matches!(
        wait(&Export::start(project.clone(), seq, only, None, || {})),
        Outcome::Done(_)
    ));
    let path = dir.path().join("song.mp3");
    let settings = ExportSettings {
        range,
        container: Container::Mp3,
        video: None,
        audio: AudioCodec::Mp3,
        sound_beside: true,
        ..ExportSettings::mp4(path.clone())
    };
    assert_eq!(settings.sound_beside_path(), Some(dir.path().join("song.wav")));
    let done = match wait(&Export::start(project, seq, settings, None, || {})) {
        Outcome::Done(done) => done,
        Outcome::Failed(err) => panic!("{}", err.report()),
        Outcome::Cancelled => panic!("cancelled"),
    };
    let beside = done.sound_beside.expect("the sound beside");
    assert_eq!(beside, dir.path().join("song.wav"));
    assert!(path.exists());
    // The same bytes after the header: the same samples.
    let (a, b) = (std::fs::read(&plain).unwrap(), std::fs::read(&beside).unwrap());
    assert_eq!(a.len(), b.len());
    assert!(a[a.len() - 4096..] == b[b.len() - 4096..]);
    // Nothing half-written is left over.
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["plain.wav", "song.mp3", "song.wav"]);
    // A WAV export writes no second WAV.
    let wav = ExportSettings {
        container: Container::Wav,
        sound_beside: true,
        ..ExportSettings::mp4(dir.path().join("x.wav"))
    };
    assert_eq!(wav.sound_beside_path(), None);
}

#[test]
fn a_paused_export_waits_and_its_time_paused_does_not_count() {
    let (project, seq) = recording();
    let dir = tempfile::tempdir().unwrap();
    let settings = ExportSettings {
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        ..ExportSettings::mp4(dir.path().join("sound.wav"))
    };
    let export = Export::start(project, seq, settings, None, || {});
    export.pause();
    std::thread::sleep(Duration::from_millis(150));
    let held = export.progress();
    assert!(held.paused && export.is_paused());
    std::thread::sleep(Duration::from_millis(300));
    let still = export.progress();
    assert_eq!(still.frames_done, held.frames_done, "nothing done while paused");
    assert!(still.elapsed < Duration::from_millis(250), "{:?}", still.elapsed);
    export.resume();
    let Outcome::Done(done) = wait(&export) else {
        panic!("the export did not finish after resuming");
    };
    assert!(done.took < Duration::from_secs(60));
    assert!(!export.progress().paused);

    // Cancelled while paused: it stops.
    let (project, seq) = recording();
    let settings = ExportSettings {
        container: Container::Wav,
        video: None,
        audio: AudioCodec::Pcm24,
        ..ExportSettings::mp4(dir.path().join("other.wav"))
    };
    let export = Export::start(project, seq, settings, None, || {});
    export.pause();
    export.cancel();
    assert!(matches!(wait(&export), Outcome::Cancelled));
    assert!(!dir.path().join("other.wav").exists());
}

#[test]
fn the_mix_is_measured_as_an_export_without_normalising_writes_it() {
    let (project, seq) = louder(6);
    let (plain, _, _dir) = wav(project.clone(), seq, None, false);
    let (cancel, done) = (AtomicBool::new(false), AtomicU32::new(0));
    let found = measure_mix(
        &project,
        seq,
        Some((Time::ZERO, Time::seconds(2))),
        &cancel,
        &done,
    )
    .expect("measured");
    assert!(
        (found.integrated - plain.loudness).abs() < 0.05,
        "{} against {}",
        found.integrated,
        plain.loudness
    );
    assert!(
        (found.true_peak - plain.true_peak).abs() < 0.05,
        "{} against {}",
        found.true_peak,
        plain.true_peak
    );
    assert_eq!(done.load(Ordering::Relaxed), 1000);
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(measure_mix(&project, seq, None, &cancel, &done), None);
}
