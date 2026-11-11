use super::*;
use crate::compose::Composing;
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
