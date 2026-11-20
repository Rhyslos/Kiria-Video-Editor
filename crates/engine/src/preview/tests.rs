use super::composed::preview_size;
use super::fill::{KEPT_BEHIND, fill_up_to, kept_within};
use super::stand_in::{KEY_FIRST, PROXY_FIRST};
use super::*;

#[test]
fn an_rgba_picture_is_sent_opaque() {
    let mut picture = Picture::black(Time::ZERO);
    picture.frame = Some(Decoded::Rgba(media::Frame {
        width: 2,
        height: 1,
        rgba: vec![10, 20, 30, 0, 40, 50, 60, 128],
        time: 0.0,
    }));
    picture.make_opaque();
    let Some(Decoded::Rgba(frame)) = picture.frame else {
        panic!("an RGBA picture");
    };
    assert_eq!(frame.rgba, [10, 20, 30, 255, 40, 50, 60, 255]);
}
use crate::frame;
use cache::proxy::ProxyMode;
use project::Command;
use std::path::Path;

const PLANES: Convert = Convert::WhileDrawing { largest_side: 8192 };

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name)
}

/// The next answer, past any first picture shown before it.
fn wait(preview: &Preview) -> Picture {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(p) = preview.latest().filter(|p| !p.stand_in) {
            return p;
        }
        assert!(Instant::now() < deadline, "no picture in time");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pictures_for_clips_and_black_for_gaps() {
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let preview = Preview::start(Decoding::Software, Convert::OnCpu, None, || {});

    preview.show(&project, seq, Time::seconds(1), 160, 160);
    let p = wait(&preview);
    let frame = p.frame.expect("a picture");
    assert!(matches!(frame, Decoded::Rgba(_)));
    assert_eq!(frame.size(), (160, 90));
    assert!((frame.time() - 1.0).abs() < 0.02);
    assert!(p.error.is_none());

    preview.show(&project, seq, Time::seconds(30), 160, 160);
    let gap = wait(&preview);
    assert!(
        gap.frame.is_none() && gap.error.is_none(),
        "after the last clip is black"
    );
}

/// The device a window would be drawn with, here without a window (a
/// software one will do). `None` when this machine has none at all.
fn composing() -> Option<Composing> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Composing {
        device,
        queue,
        wrap: None,
    })
}

/// Red, green, blue and alpha of one pixel of a composed frame.
fn pixel(composing: &Composing, composed: &Composed, (x, y): (u32, u32)) -> [f32; 4] {
    let buffer = composing.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = composing
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: composed.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    composing.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    composing
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let bytes = buffer.get_mapped_range(..).unwrap();
    // Four 16-bit floats.
    let half = |i: usize| {
        let bits = u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
        let (exponent, fraction) = (i32::from((bits >> 10) & 0x1f), f32::from(bits & 0x3ff));
        let value = if exponent == 0 {
            fraction / 1024.0 * 2f32.powi(-14)
        } else {
            (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15)
        };
        if bits & 0x8000 != 0 { -value } else { value }
    };
    [half(0), half(1), half(2), half(3)]
}

#[test]
fn the_timelines_whole_picture_is_put_together_on_the_device() {
    let Some(composing) = composing() else {
        eprintln!("no graphics device here: putting pictures together isn't checked");
        return;
    };
    // A wide recording on a tall timeline: bars above and below it.
    let project = Project::new("T", project::SequenceSettings::VERTICAL_SHORT);
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    for convert in [PLANES, Convert::OnCpu] {
        let preview =
            Preview::start_composing(Decoding::Software, convert, None, Some(composing.clone()), || {});
        preview.show(&project, seq, Time::seconds(1), 400, 480);
        let p = wait(&preview);
        assert!(
            p.frame.is_none() && p.error.is_none() && !p.refused,
            "{convert:?}"
        );
        let composed = p.composed.expect("a picture");
        // The timeline's shape, as large as fits the room.
        assert_eq!(composed.size(), (270, 480), "{convert:?}");
        assert!(!composed.on_device);
        assert_eq!(
            pixel(&composing, &composed, (135, 20)),
            [0.0, 0.0, 0.0, 1.0],
            "above the picture"
        );
        assert_eq!(
            pixel(&composing, &composed, (135, 460)),
            [0.0, 0.0, 0.0, 1.0],
            "below it"
        );
        let middle = pixel(&composing, &composed, (135, 240));
        assert!(
            middle[..3].iter().any(|&light| light > 0.01),
            "{convert:?}: {middle:?}"
        );
        assert!(p.method.is_some() && p.work.is_some());

        preview.show(&project, seq, Time::seconds(30), 400, 480);
        let gap = wait(&preview);
        assert!(
            gap.composed.is_none() && gap.error.is_none(),
            "after the last clip is black"
        );
    }
}

#[test]
fn a_picture_that_shows_through_bars_has_the_one_below_under_it() {
    let Some(composing) = composing() else {
        return;
    };
    // On a tall timeline the clip on top leaves bars; what is below it
    // is decoded too, at the same moment, and both are in the picture.
    let project = Project::new("T", project::SequenceSettings::VERTICAL_SHORT);
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let mut project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    project = Command::AddTrack {
        sequence: seq,
        kind: project::TrackKind::Video,
        name: "V2".into(),
    }
    .apply(&project)
    .unwrap();
    let sequence = project.active().unwrap();
    let (track, mut clip) = (
        sequence.video_tracks[1].id,
        sequence.video_tracks[0].clips[0].clone(),
    );
    clip.id = project.allocate_id();
    clip.link = None;
    let project = Command::PlaceClip {
        sequence: seq,
        track,
        clip,
    }
    .apply(&project)
    .unwrap();
    let described = frame::describe(&project, project.active().unwrap(), Time::seconds(1));
    assert_eq!(
        described.layers.len(),
        2,
        "the top clip has bars, so the lower one is seen"
    );

    let preview = Preview::start_composing(Decoding::Software, PLANES, None, Some(composing.clone()), || {});
    preview.show(&project, seq, Time::seconds(1), 270, 480);
    let p = wait(&preview);
    assert!(p.error.is_none());
    let composed = p.composed.expect("a picture");
    let middle = pixel(&composing, &composed, (135, 240));
    assert!(middle[..3].iter().any(|&light| light > 0.01), "{middle:?}");
    assert_eq!(pixel(&composing, &composed, (135, 20))[3], 1.0);
}

#[test]
fn the_preview_has_the_timelines_shape_and_never_more_than_its_size() {
    assert_eq!(preview_size((1920, 1080), (960, 960)), (960, 540));
    assert_eq!(preview_size((1080, 1920), (400, 480)), (270, 480));
    assert_eq!(
        preview_size((1920, 1080), (4000, 4000)),
        (1920, 1080),
        "never enlarged"
    );
    assert_eq!(
        preview_size((1920, 1080), (0, 0)),
        (2, 2),
        "a viewer with no room yet"
    );
}

#[test]
fn planes_for_the_viewer_to_convert() {
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let preview = Preview::start(Decoding::Software, PLANES, None, || {});

    preview.show(&project, seq, Time::seconds(1), 320, 320);
    let p = wait(&preview);
    let Some(Decoded::Yuv(frame)) = p.frame else {
        panic!("planes, got {:?}", p.frame);
    };
    assert_eq!((frame.width, frame.height), (320, 180));
    assert!((frame.time - 1.0).abs() < 0.02);
    // A smaller viewer gets the planes halved.
    preview.show(&project, seq, Time::seconds(1), 160, 160);
    assert_eq!(wait(&preview).frame.expect("a picture").size(), (160, 90));
}

#[test]
fn planes_too_big_for_a_texture_come_as_rgba() {
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let small = Convert::WhileDrawing { largest_side: 256 };
    let preview = Preview::start(Decoding::Software, small, None, || {});
    preview.show(&project, seq, Time::seconds(1), 320, 320);
    let frame = wait(&preview).frame.expect("a picture");
    assert!(matches!(frame, Decoded::Rgba(_)), "{frame:?}");
    assert_eq!(frame.size(), (320, 180));
}

#[test]
fn each_file_goes_down_the_ladder_one_way_at_a_time() {
    use Level::{Device, Hardware, Software};
    let full = ladder(Decoding::Best, PLANES, true);
    assert_eq!(full, [Device, Hardware, Software]);
    assert_eq!(below(full, Device), [Hardware, Software]);
    assert_eq!(below(full, Hardware), [Software]);
    assert!(below(full, Software).is_empty(), "nothing under the CPU");
    // Without the device, or with a viewer that can't draw from it, or
    // when only the CPU is wanted, the device is never tried.
    assert_eq!(ladder(Decoding::Best, PLANES, false), [Hardware, Software]);
    assert_eq!(ladder(Decoding::Best, Convert::OnCpu, true), [Hardware, Software]);
    assert_eq!(ladder(Decoding::Software, PLANES, true), [Software]);
    // "The best way" may itself have ended on the CPU: nothing is below that.
    assert!(below(ladder(Decoding::Best, PLANES, false), Software).is_empty());
}

/// Counts what the worker tells the watcher.
#[derive(Default)]
struct Counting {
    about_to: std::sync::atomic::AtomicU32,
    answered: std::sync::atomic::AtomicU32,
    errors: std::sync::atomic::AtomicU32,
}

impl DeviceWatch for Counting {
    fn about_to_decode(&self) {
        self.about_to.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn answered(&self) {
        self.answered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn reported_an_error(&self) {
        self.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[test]
fn a_file_the_device_turns_down_is_decoded_the_next_way_without_an_error() {
    // A device without a window, on whatever Vulkan this machine has. It
    // decodes no video, so every file is turned down by it.
    let Some(device) = video_device::testing::device() else {
        return;
    };
    let device = WindowDevice::new(device).unwrap();
    let watch = Arc::new(Counting::default());
    let on_device = OnDevice {
        device,
        watch: Arc::clone(&watch) as Arc<dyn DeviceWatch>,
        frames_kept: 64 << 20,
    };
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let preview = Preview::start(Decoding::Best, PLANES, Some(on_device), || {});
    preview.show(&project, seq, Time::seconds(1), 320, 320);
    let p = wait(&preview);
    assert!(p.error.is_none(), "{:?}", p.error.map(|e| e.report()));
    assert!(matches!(p.frame, Some(Decoded::Yuv(_))), "{:?}", p.frame);
    assert!(!p.method.unwrap().contains("stays on the GPU"));
    let count = |n: &std::sync::atomic::AtomicU32| n.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!((count(&watch.about_to), count(&watch.errors)), (1, 1));
    assert_eq!(
        count(&watch.answered),
        0,
        "turned down, which is an answer of its own"
    );
    // The file stays on the way that worked: the device isn't asked again.
    preview.show(&project, seq, Time::from_ratio(3, 2).unwrap(), 320, 320);
    assert!(wait(&preview).frame.is_some());
    assert_eq!((count(&watch.about_to), count(&watch.errors)), (1, 1));
}

#[test]
fn a_file_the_device_failed_for_is_not_tried_there_again() {
    use Level::{Device, Hardware, Software};
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let (failed, other) = ((media.id, 0), (media.id, 1));
    let mut worker = Worker {
        ladder: ladder(Decoding::Best, PLANES, true),
        convert: PLANES,
        on_device: None,
        decoders: VecDeque::new(),
        off_device: HashSet::new(),
        gave_up_last: false,
        composing: None,
        composer: None,
        frames: FrameCache::new(0),
        playhead: None,
        filler: None,
        unfillable: HashSet::new(),
        stuck: None,
        playing: false,
        key_frame_first: true,
        proxies: Proxies::default(),
        glancer: None,
        last_glance: None,
        titles: VecDeque::new(),
    };
    assert_eq!(worker.ladder_for(failed), [Device, Hardware, Software]);
    worker.off_device.insert(failed);
    // Also after its decoder was closed to make room for other files.
    assert_eq!(worker.ladder_for(failed), [Hardware, Software]);
    assert_eq!(worker.ladder_for(other), [Device, Hardware, Software]);
}

#[test]
fn closing_waits_for_the_worker_and_asks_for_nothing_more() {
    let p = Project::new_automatic("T");
    let seq = p.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
    let mut preview = Preview::start(Decoding::Software, Convert::OnCpu, None, || {});
    preview.show(&p, seq, Time::seconds(1), 640, 360);
    assert!(wait(&preview).frame.is_some());
    // One more is asked for and may be under way when it closes.
    preview.show(&p, seq, Time::from_ratio(3, 2).unwrap(), 640, 360);
    assert!(preview.close(Duration::from_secs(20)), "the worker ended");
    let _ = preview.latest();
    preview.show(&p, seq, Time::seconds(2), 640, 360);
    std::thread::sleep(Duration::from_millis(100));
    assert!(preview.latest().is_none(), "nothing is decoded after closing");
    assert!(preview.close(Duration::from_millis(10)), "closing twice is fine");
}

/// A project with the two-second fixture on the timeline, and a worker
/// that decodes on the CPU.
fn worker_and_project() -> (Worker, Project, SequenceId) {
    let p = Project::new_automatic("T");
    let seq = p.active_sequence;
    let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    let p = Command::Import { sequence: seq, media }.apply(&p).unwrap();
    let worker = Worker {
        ladder: ladder(Decoding::Software, Convert::OnCpu, false),
        convert: Convert::OnCpu,
        on_device: None,
        decoders: VecDeque::new(),
        off_device: HashSet::new(),
        gave_up_last: false,
        composing: None,
        composer: None,
        frames: FrameCache::new(0),
        playhead: None,
        filler: None,
        unfillable: HashSet::new(),
        stuck: None,
        playing: false,
        key_frame_first: true,
        proxies: Proxies::default(),
        glancer: None,
        last_glance: None,
        titles: VecDeque::new(),
    };
    (worker, p, seq)
}

fn request(p: &Project, seq: SequenceId, millis: i64) -> Request {
    Request {
        project: p.clone(),
        sequence: seq,
        at: Time::from_ratio(millis, 1000).unwrap(),
        max_width: 160,
        max_height: 160,
        proxies: None,
        playing: false,
        key_frame_first: true,
        scope: false,
    }
}

/// A shelf in `mode` holding a proxy of the fixture in `p`.
fn shelf_with_proxy(p: &Project, mode: ProxyMode) -> (Arc<Shelf>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let shelf = Shelf::new(dir.path(), mode);
    let media = p.media.values().next().unwrap();
    let steps = media::proxy::Steps {
        cancelled: &|| false,
        progress: &|_| {},
        pause: &|| {},
    };
    shelf
        .build(
            &media.path,
            0,
            media.duration.as_secs_f64(),
            &media.fingerprint,
            Decoding::Software,
            1,
            &steps,
        )
        .unwrap();
    (shelf, dir)
}

fn with_shelf(mut r: Request, shelf: &Arc<Shelf>) -> Request {
    r.proxies = Some(Arc::clone(shelf));
    r
}

/// Runs one request, returning what was shown first and the answer.
fn ask(worker: &mut Worker, r: &Request, waiting: &mut Waiting<'_>) -> (Vec<Picture>, Option<Picture>) {
    let mut first = Vec::new();
    let answer = worker.picture(r, waiting, &mut |p| {
        first.push(p);
        true
    });
    (first, answer)
}

#[test]
fn a_jump_shows_the_proxy_frame_first_then_the_exact_one() {
    let (mut worker, p, seq) = worker_and_project();
    let (shelf, _dir) = shelf_with_proxy(&p, ProxyMode::Auto);
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    let (first, answer) = ask(
        &mut worker,
        &with_shelf(request(&p, seq, 1800), &shelf),
        &mut waiting,
    );
    let answer = answer.expect("the exact picture");
    assert_eq!(first.len(), 1, "one first picture");
    assert!(first[0].stand_in);
    assert_eq!(first[0].method.as_deref(), Some(PROXY_FIRST));
    let (proxy, exact) = (first[0].frame.as_ref().unwrap(), answer.frame.as_ref().unwrap());
    assert!(
        (proxy.time() - exact.time()).abs() < 1e-6,
        "the right frame, softer"
    );
    assert!(!answer.stand_in && answer.method.as_deref() != Some(PROXY_FIRST));
    // Playing, or a moment just ahead of the decoder: nothing first.
    let (first, _) = ask(
        &mut worker,
        &with_shelf(request(&p, seq, 1850), &shelf),
        &mut waiting,
    );
    assert!(first.is_empty(), "near: decoded on at once");
    let mut playing = with_shelf(request(&p, seq, 400), &shelf);
    playing.playing = true;
    let (first, answer) = ask(&mut worker, &playing, &mut waiting);
    assert!(
        first.is_empty() && answer.is_some(),
        "playing decodes on, frame by frame"
    );
}

#[test]
fn with_proxies_on_the_original_is_never_decoded() {
    let (mut worker, p, seq) = worker_and_project();
    let (shelf, _dir) = shelf_with_proxy(&p, ProxyMode::On);
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    for millis in [1800, 300, 1500] {
        let (first, answer) = ask(
            &mut worker,
            &with_shelf(request(&p, seq, millis), &shelf),
            &mut waiting,
        );
        let answer = answer.unwrap();
        assert!(first.is_empty());
        assert_eq!(answer.method.as_deref(), Some(PROXY_ONLY));
        assert!(answer.frame.is_some() && !answer.stand_in);
    }
    assert!(worker.decoders.is_empty(), "no decoder for the original");
    // Proxies Off: the original, and nothing from the proxy.
    shelf.set_mode(ProxyMode::Off);
    let (first, answer) = ask(
        &mut worker,
        &with_shelf(request(&p, seq, 1200), &shelf),
        &mut waiting,
    );
    assert!(first.iter().all(|f| f.method.as_deref() != Some(PROXY_FIRST)));
    assert_ne!(answer.unwrap().method.as_deref(), Some(PROXY_ONLY));
}

#[test]
fn without_a_proxy_the_key_frame_before_comes_first() {
    let (mut worker, p, seq) = worker_and_project();
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    let (first, answer) = ask(&mut worker, &request(&p, seq, 1800), &mut waiting);
    let exact = answer.unwrap().frame.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].method.as_deref(), Some(KEY_FIRST));
    let key = first[0].frame.as_ref().unwrap();
    assert!(
        key.time() <= exact.time() + 1e-6,
        "{} after {}",
        key.time(),
        exact.time()
    );
    // A small step back: no picture from far earlier first.
    let (first, _) = ask(&mut worker, &request(&p, seq, 1700), &mut waiting);
    assert!(first.is_empty());
}

#[test]
fn with_key_frames_first_off_only_the_exact_frame_or_a_proxy_comes() {
    let (mut worker, p, seq) = worker_and_project();
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    let mut off = request(&p, seq, 1800);
    off.key_frame_first = false;
    let (first, answer) = ask(&mut worker, &off, &mut waiting);
    assert!(first.is_empty(), "no key frame first");
    let answer = answer.unwrap();
    assert!(!answer.stand_in && answer.frame.is_some(), "the exact frame");
    // A proxy frame still comes first: the setting is about key frames only.
    let (mut worker, p, seq) = worker_and_project();
    let (shelf, _dir) = shelf_with_proxy(&p, ProxyMode::Auto);
    let mut off = with_shelf(request(&p, seq, 1800), &shelf);
    off.key_frame_first = false;
    let (first, answer) = ask(&mut worker, &off, &mut waiting);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].method.as_deref(), Some(PROXY_FIRST));
    assert!(!answer.unwrap().stand_in);
}

#[test]
fn waiting_for_a_request_ends_when_nothing_comes() {
    let (_, p, seq) = worker_and_project();
    let (requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    let patience = Duration::from_millis(20);
    assert!(matches!(waiting.next_within(patience), Next::Idle));
    requests.send(request(&p, seq, 100)).unwrap();
    requests.send(request(&p, seq, 200)).unwrap();
    let Next::Request(newest) = waiting.next_within(patience) else {
        panic!("a request was waiting");
    };
    assert_eq!(Some(newest.at), Time::from_ratio(200, 1000));
    assert!(matches!(waiting.next_within(patience), Next::Idle));
    drop(requests);
    assert!(matches!(waiting.next_within(patience), Next::Closed));
}

#[test]
fn while_playing_only_the_last_few_frames_passed_are_kept() {
    // A decode that fell behind while playing passes many frames: making
    // each a picture on the card held it back further. A few frames at 60
    // frames a second, at least the one asked for at 24.
    let playing = kept_within(true);
    assert!((1.0 / 60.0 * 2.0..=0.1).contains(&playing), "{playing}");
    assert_eq!(kept_within(false), KEPT_BEHIND);
}

#[test]
fn the_stretch_behind_the_playhead_is_filled_in_when_little_is_kept() {
    let tick = 0.0005;
    // The playhead is 0.3 s into the frames kept, which begin at 12 s:
    // decode up to the frame just before them.
    let up_to = fill_up_to(12.3, 12.0, tick).unwrap();
    assert!(up_to < 12.0 - tick && up_to > 12.0 - 0.004, "{up_to}");
    // Two seconds or more are kept behind it: enough.
    assert_eq!(fill_up_to(14.0, 12.0, tick), None);
    // The frames kept begin at the start of the file: nothing before them.
    assert_eq!(fill_up_to(0.5, 0.0, tick), None);
}

#[test]
fn with_nothing_kept_there_is_nothing_to_fill_in() {
    let (mut worker, p, seq) = worker_and_project();
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    assert!(!worker.fill(&mut waiting), "no playhead yet");
    // A picture decoded on the CPU is not kept, so nothing is filled in behind it.
    worker
        .picture(&request(&p, seq, 1000), &mut waiting, &mut |_| true)
        .unwrap();
    assert!(worker.playhead.is_some());
    assert!(!worker.fill(&mut waiting));
    // In a gap there is no file to fill in.
    worker
        .picture(&request(&p, seq, 30_000), &mut waiting, &mut |_| true)
        .unwrap();
    assert!(worker.playhead.is_none());
}

#[test]
fn only_the_newest_request_waiting_counts() {
    let (_, p, seq) = worker_and_project();
    let (requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    for millis in [100, 200, 300] {
        requests.send(request(&p, seq, millis)).unwrap();
    }
    assert_eq!(waiting.next().map(|r| r.at), Time::from_ratio(300, 1000));
    requests.send(request(&p, seq, 400)).unwrap();
    assert!(waiting.take_in(), "a newer one arrived");
    assert!(!waiting.take_in(), "nothing since");
    assert_eq!(waiting.next().map(|r| r.at), Time::from_ratio(400, 1000));
    // Nobody asks any more: what was still waiting is not worked on.
    requests.send(request(&p, seq, 500)).unwrap();
    drop(requests);
    assert!(waiting.next().is_none());
}

#[test]
fn a_jump_overtaken_by_a_request_elsewhere_is_given_up() {
    let (mut worker, p, seq) = worker_and_project();
    let (requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    // While 1.8 s is decoded, a moment after the clip's end (a gap) is asked for.
    requests.send(request(&p, seq, 30_000)).unwrap();
    assert!(
        worker
            .picture(&request(&p, seq, 1800), &mut waiting, &mut |_| true)
            .is_none()
    );
    // The newer request is still waiting, and is answered next.
    let next = waiting.next().unwrap();
    assert_eq!(Some(next.at), Time::from_ratio(30_000, 1000));
    let gap = worker.picture(&next, &mut waiting, &mut |_| true).unwrap();
    assert!(gap.frame.is_none() && gap.error.is_none());
}

#[test]
fn the_decode_after_one_given_up_is_finished_whatever_arrives() {
    let (mut worker, p, seq) = worker_and_project();
    let (requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    let elsewhere = 30_000; // after the clip's end
    worker
        .picture(&request(&p, seq, 1900), &mut waiting, &mut |_| true)
        .unwrap();
    // A jump back, overtaken by a request this decode can't serve: given up.
    requests.send(request(&p, seq, elsewhere)).unwrap();
    assert!(
        worker
            .picture(&request(&p, seq, 500), &mut waiting, &mut |_| true)
            .is_none()
    );
    // The next decode is finished, though it is overtaken the same way.
    requests.send(request(&p, seq, elsewhere + 1)).unwrap();
    let picture = worker
        .picture(&request(&p, seq, 1000), &mut waiting, &mut |_| true)
        .unwrap();
    assert_eq!(Some(picture.at), Time::from_ratio(1000, 1000));
    assert!(picture.frame.is_some());
    assert!(
        picture.work.is_some_and(|work| work.frames > 30),
        "{:?}",
        picture.work
    );
    // After a finished one, giving up is allowed again.
    requests.send(request(&p, seq, elsewhere + 2)).unwrap();
    assert!(
        worker
            .picture(&request(&p, seq, 300), &mut waiting, &mut |_| true)
            .is_none()
    );
}

/// A frame kept on the graphics card is shown without a decoder being
/// opened. Runs where there is a Vulkan driver (a software one will do).
#[test]
fn a_frame_kept_on_the_card_is_shown_without_decoding() {
    let Some(device) = video_device::testing::device() else {
        return;
    };
    let (mut worker, p, seq) = worker_and_project();
    let (_requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    worker.frames = FrameCache::new(64 << 20);
    let (key, moment) = source_of(&request(&p, seq, 500)).unwrap();
    let planes = Arc::new(video_device::PlaneImages::new(&device, (64, 36)).unwrap());
    let frame = GpuFrame {
        planes,
        colour: media::Colour {
            matrix: media::Matrix::Bt709,
            range: media::Range::Limited,
        },
        rotation: 0,
        time: moment,
    };
    worker
        .frames
        .put(key, moment, moment + 1.0 / 60.0, 0.0005, 64 * 36 * 3 / 2, frame);
    let picture = worker
        .picture(&request(&p, seq, 500), &mut waiting, &mut |_| true)
        .unwrap();
    assert!(matches!(picture.frame, Some(Decoded::Gpu(_))));
    assert_eq!(picture.method.as_deref(), Some(KEPT));
    assert!(
        picture.work.is_none() && worker.decoders.is_empty(),
        "no decoder was needed"
    );
    // A moment that isn't kept is decoded as always.
    let picture = worker
        .picture(&request(&p, seq, 900), &mut waiting, &mut |_| true)
        .unwrap();
    assert!(matches!(picture.frame, Some(Decoded::Rgba(_))));
    assert_eq!(worker.decoders.len(), 1);
}

#[test]
fn a_request_reached_on_the_way_gets_the_picture() {
    let (mut worker, p, seq) = worker_and_project();
    let (requests, inbox) = channel::<Request>();
    let mut waiting = Waiting::new(&inbox);
    // While 1.8 s is decoded from the start, 0.5 s is asked for: the
    // decode stops there, and the picture is that request's.
    requests.send(request(&p, seq, 500)).unwrap();
    let picture = worker
        .picture(&request(&p, seq, 1800), &mut waiting, &mut |_| true)
        .unwrap();
    assert_eq!(Some(picture.at), Time::from_ratio(500, 1000));
    let frame = picture.frame.unwrap();
    assert!((frame.time() - 0.5).abs() < 0.02, "{}", frame.time());
    assert!(waiting.newest.is_none(), "answered: not decoded a second time");
    // A later moment on the way does not stop the one before it.
    requests.send(request(&p, seq, 1900)).unwrap();
    let picture = worker
        .picture(&request(&p, seq, 1200), &mut waiting, &mut |_| true)
        .unwrap();
    assert_eq!(Some(picture.at), Time::from_ratio(1200, 1000));
    assert_eq!(
        waiting.newest.as_ref().map(|r| r.at),
        Time::from_ratio(1900, 1000)
    );
}

#[test]
fn a_missing_file_is_explained() {
    let project = Project::new_automatic("T");
    let seq = project.active_sequence;
    let mut media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
    media.path = fixture("moved-away.mkv");
    let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
    let preview = Preview::start(Decoding::Software, PLANES, None, || {});
    preview.show(&project, seq, Time::seconds(1), 160, 160);
    let p = wait(&preview);
    assert!(p.frame.is_none());
    assert!(p.error.unwrap().what().contains("can't be shown"));
}
