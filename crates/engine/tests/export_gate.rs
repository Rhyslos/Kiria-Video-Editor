//! Gate 6, measured: exported pictures against the viewer's, sync over a
//! long export, and how fast a large timeline exports. The golden-frame
//! check runs with the other tests; the long ones are run by hand:
//!
//! ```text
//! cargo test -p engine --release --test export_gate -- --ignored --nocapture
//! GATE_MINUTES=60 cargo test -p engine --release --test export_gate long -- --ignored --nocapture
//! GATE_SECONDS=60 cargo test -p engine --release --test export_gate speed -- --ignored --nocapture
//! ```
//!
//! `GATE_KEEP=<folder>` keeps the exported files there, to look at them
//! with other tools. The numbers are in `docs/build-plan.md` under Gate 6.

// Test helpers may panic: a failure here is a failing test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use engine::export::{AudioCodec, Container, Devices, Outcome, Quality, VideoCodec};
use engine::{Composing, Convert, Export, ExportSettings, Preview};
use media::Decoding;
use media::encode::{AudioSettings, VideoSettings, Writer};
use project::look::ONE;
use project::{Command, Project, Rate, SequenceId, SequenceSettings, Time, TrackKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The device a window would be drawn with, here without a window (a
/// software one will do). `None` when this machine has none at all.
fn devices() -> Option<Devices> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    eprintln!("graphics device: {}", adapter.get_info().name);
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

fn wait(export: &Export, patience: Duration) -> engine::export::Exported {
    let give_up = Instant::now() + patience;
    loop {
        match export.finished() {
            Some(Outcome::Done(done)) => return done,
            Some(Outcome::Failed(err)) => panic!("the export failed: {}", err.report()),
            Some(Outcome::Cancelled) => panic!("the export was cancelled"),
            None => {}
        }
        assert!(Instant::now() < give_up, "the export did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Where exported files go: `GATE_KEEP` when set, else `dir`.
fn out_dir(dir: &Path) -> PathBuf {
    match std::env::var_os("GATE_KEEP") {
        Some(keep) => {
            let keep = PathBuf::from(keep);
            std::fs::create_dir_all(&keep).unwrap();
            keep
        }
        None => dir.to_path_buf(),
    }
}

/// An NV12 picture of `size` whose brightness and colour come from `pixel`
/// (Y, U, V for each pixel; colour taken at the top left of each 2×2).
fn nv12(size: (u32, u32), pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let (w, h) = size;
    let mut out = Vec::with_capacity((w * h * 3 / 2) as usize);
    for y in 0..h {
        for x in 0..w {
            out.push(pixel(x, y)[0]);
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let [_, u, v] = pixel(x, y);
            out.extend([u, v]);
        }
    }
    out
}

// ----------------------------------------------------------------------
// Golden frames

const GOLDEN: (u32, u32) = (640, 360);

/// Two seconds of smooth colour at 30 fps, written nearly losslessly: a
/// diagonal sweep of brightness with colour changing slowly across it
/// (`hue` sets where), moving a little each frame.
fn gradient(path: &Path, hue: u32) {
    let video = VideoSettings {
        codec: VideoCodec::H264,
        width: GOLDEN.0,
        height: GOLDEN.1,
        rate: (30, 1),
        bit_rate: 20_000_000,
        software: true,
    };
    let mut writer = Writer::create(path, Container::Mkv, Some(&video), None).unwrap();
    for frame in 0..60u32 {
        let picture = nv12(GOLDEN, |x, y| {
            let luma = 16 + (x + y + frame) * 219 / (GOLDEN.0 + GOLDEN.1 + 60);
            let wave = |at: u32, period: f32| (at as f32 * std::f32::consts::TAU / period).sin() * 80.0;
            let u = 128.0 + wave(x + hue, 400.0);
            let v = 128.0 + wave(y * 2 + hue * 2, 500.0);
            [luma as u8, u as u8, v as u8]
        });
        writer.write_picture(&picture).unwrap();
    }
    writer.finish().unwrap();
}

/// A timeline with two layers, the top one placed, sized, turned, faded
/// and coloured (and moving: its place is keyframed), under an adjustment
/// layer that changes the colour of both.
fn layered(dir: &Path) -> (Project, SequenceId) {
    let (under, over) = (dir.join("under.mkv"), dir.join("over.mkv"));
    gradient(&under, 0);
    gradient(&over, 70);
    let settings = SequenceSettings {
        width: GOLDEN.0,
        height: GOLDEN.1,
        frame_rate: Rate::FPS_30,
        sample_rate: 48_000,
    };
    let mut project = Project::new("Golden", settings);
    let seq = project.active_sequence;
    project = Command::Import {
        sequence: seq,
        media: media::describe(&under).unwrap(),
    }
    .apply(&project)
    .unwrap();
    project = Command::AddTrack {
        sequence: seq,
        kind: TrackKind::Video,
        name: "Over".into(),
    }
    .apply(&project)
    .unwrap();
    project = Command::AddMedia {
        media: media::describe(&over).unwrap(),
    }
    .apply(&project)
    .unwrap();
    let over_id = *project
        .media
        .iter()
        .find(|(_, m)| m.path == over)
        .map(|(id, _)| id)
        .unwrap();
    let track = project.active().unwrap().video_tracks[1].id;
    project = Command::DropMedia {
        sequence: seq,
        media: over_id,
        at: Time::ZERO,
        track: Some(track),
        insert: false,
    }
    .apply(&project)
    .unwrap();
    let clip = project.active().unwrap().video_tracks[1].clips[0].id;
    let look = project::Look {
        scale: 550_000,
        turn: 12 * ONE,
        opacity: 800_000,
        saturation: 400_000,
        temperature: 300_000,
        exposure: ONE / 3,
        ..project::Look::PLAIN
    };
    project = Command::SetLook {
        sequence: seq,
        clip,
        look,
    }
    .apply(&project)
    .unwrap();
    // Moving from the left to the right over the two seconds.
    for at in [Time::ZERO, Time::seconds(2)] {
        project = Command::Keyframe {
            sequence: seq,
            clip,
            at,
            fields: vec![project::LookField::X],
            change: project::animate::KeyframeChange::Add,
        }
        .apply(&project)
        .unwrap();
    }
    for (at, x) in [(Time::ZERO, -200_000), (Time::seconds(2), 200_000)] {
        project = Command::SetLookAt {
            sequence: seq,
            clip,
            at,
            look: project::Look { x, ..look },
        }
        .apply(&project)
        .unwrap();
    }
    project = Command::AddAdjustment {
        sequence: seq,
        at: Time::ZERO,
        length: Time::seconds(2),
        track: None,
    }
    .apply(&project)
    .unwrap();
    let adjustment = project.active().unwrap().video_tracks[2].clips[0].id;
    project = Command::SetLook {
        sequence: seq,
        clip: adjustment,
        look: project::Look {
            contrast: 250_000,
            tint: -200_000,
            opacity: 900_000,
            ..project::Look::PLAIN
        },
    }
    .apply(&project)
    .unwrap();
    (project, seq)
}

/// The finished frame the viewer shows at `at`, drawn as the viewer draws
/// it (into an 8-bit target) and read back as RGBA; and the same frame as
/// an export hands it to the encoder (NV12), turned back into RGBA.
fn viewer_picture(
    preview: &Preview,
    devices: &Devices,
    project: &Project,
    seq: SequenceId,
    at: Time,
) -> (Vec<u8>, Vec<u8>) {
    preview.show(project, seq, at, GOLDEN.0, GOLDEN.1);
    let give_up = Instant::now() + Duration::from_secs(60);
    let composed = loop {
        match preview.wait(Duration::from_millis(250)) {
            Some(p) if p.at == at && !p.stand_in => {
                assert!(p.error.is_none(), "{:?}", p.error.map(|e| e.report()));
                break p.composed.expect("a finished frame");
            }
            _ => assert!(Instant::now() < give_up, "no picture"),
        }
    };
    let (device, queue) = (&devices.composing.device, &devices.composing.queue);
    let mut reader = render::Reader::new(device, GOLDEN);
    let mut planes = Vec::new();
    reader
        .read(device, queue, composed.view(), composed.working(), &mut planes)
        .unwrap();
    let target = wgpu::TextureFormat::Rgba8Unorm;
    let shower = render::Shower::new(device, target);
    let (w, h) = GOLDEN;
    let extent = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let window = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = window.create_view(&wgpu::TextureViewDescriptor::default());
    let showing = shower.showing(device, queue, composed.view(), composed.working());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        shower.draw(&mut pass, &showing);
    }
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        window.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let shown = mapped
        .chunks(row as usize)
        .flat_map(|r| r[..w as usize * 4].to_vec())
        .collect();
    (shown, nv12_to_rgba(&planes, GOLDEN))
}

/// NV12 in BT.709 limited range back to RGBA, exactly (each colour sample
/// used for its 2×2 pixels): what a player shows of planes not encoded.
fn nv12_to_rgba(planes: &[u8], (w, h): (u32, u32)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let luma = (f64::from(planes[y * w + x]) - 16.0) / 219.0;
            let c = w * h + (y / 2) * w + (x / 2) * 2;
            let cb = (f64::from(planes[c]) - 128.0) / 224.0;
            let cr = (f64::from(planes[c + 1]) - 128.0) / 224.0;
            let r = luma + 1.5748 * cr;
            let b = luma + 1.8556 * cb;
            let g = (luma - 0.2126 * r - 0.0722 * b) / 0.7152;
            let byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
            out.extend([byte(r), byte(g), byte(b), 255]);
        }
    }
    out
}

/// How far two RGBA pictures are apart, over the colour channels: the
/// largest difference, the mean, and the difference 99 values in 100 stay
/// within.
fn apart(a: &[u8], b: &[u8]) -> (u8, f64, u8) {
    assert_eq!(a.len(), b.len());
    let mut counts = [0u64; 256];
    let mut sum = 0u64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            counts[usize::from(d)] += 1;
            sum += u64::from(d);
        }
    }
    let total: u64 = counts.iter().sum();
    let max = counts.iter().rposition(|&n| n > 0).unwrap_or(0) as u8;
    let mut seen = 0;
    let mut p99 = 0;
    for (d, n) in counts.iter().enumerate() {
        seen += n;
        if seen * 100 >= total * 99 {
            p99 = d as u8;
            break;
        }
    }
    (max, sum as f64 / total as f64, p99)
}

#[test]
fn exported_frames_match_the_viewer_within_the_yuv_round_trip() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (project, seq) = layered(dir.path());
    let path = out_dir(dir.path()).join("golden.mkv");
    let settings = ExportSettings {
        container: Container::Mkv,
        audio: AudioCodec::Flac,
        quality: Quality::High,
        software: true,
        ..ExportSettings::mp4(path.clone())
    };
    let export = Export::start(project.clone(), seq, settings, Some(devices.clone()), || {});
    wait(&export, Duration::from_secs(300));

    let mut preview = Preview::start_composing(
        Decoding::Software,
        devices.convert,
        None,
        Some(devices.composing.clone()),
        || {},
    );
    preview.set_key_frame_first(false);
    let made = media::describe(&path).unwrap();
    let video = made.info.video.expect("a picture stream");
    let mut decoder = media::VideoDecoder::open(&path, video.index, 0, Decoding::Software).unwrap();
    let rate = Rate::FPS_30;
    let mut worst = (0u8, 0.0f64, 0u8);
    for frame in [0, 1, 17, 30, 44, 59] {
        let at = rate.frame_start(frame);
        let (shown, handed) = viewer_picture(&preview, &devices, &project, seq, at);
        let (h_max, h_mean, h_p99) = apart(&shown, &handed);
        eprintln!(
            "frame {frame}, as handed to the encoder: largest difference {h_max}, mean {h_mean:.2}, 99 in 100 within {h_p99}"
        );
        // The file's planes turned into colour as a player's shader does
        // (the viewer's conversion, exactly), not the quicker one used for
        // still pictures.
        let media::Decoded::Yuv(planes) = decoder
            .picture_at(at.as_secs_f64() + 0.5 / 30.0, GOLDEN.0, GOLDEN.1)
            .unwrap()
        else {
            panic!("planes expected");
        };
        let exported = planes.to_rgba();
        assert_eq!((exported.width, exported.height), GOLDEN);
        let (max, mean, p99) = apart(&shown, &exported.rgba);
        if let Some(keep) = std::env::var_os("GATE_KEEP") {
            let keep = PathBuf::from(keep);
            for (name, rgba) in [
                ("viewer", &shown),
                ("handed", &handed),
                ("exported", &exported.rgba),
            ] {
                image::save_buffer(
                    keep.join(format!("golden-{frame}-{name}.png")),
                    rgba,
                    GOLDEN.0,
                    GOLDEN.1,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();
            }
        }
        eprintln!("frame {frame}: largest difference {max}, mean {mean:.2}, 99 in 100 within {p99}");
        // An 8-bit YUV 4:2:0 round trip of smooth colour: about a level on
        // average, a little more after the encoder. The largest differences
        // are at the turned layer's sharp edges between strong colours,
        // where the colour planes at half size can't follow (a quarter of a
        // percent of the pixels, in any 4:2:0 file).
        assert!(
            h_mean < 2.0,
            "frame {frame}: mean {h_mean:.2} levels apart before encoding"
        );
        assert!(
            h_p99 <= 10,
            "frame {frame}: 99 in 100 within {h_p99} levels before encoding"
        );
        assert!(mean < 2.5, "frame {frame}: mean {mean:.2} levels apart");
        assert!(p99 <= 16, "frame {frame}: 99 in 100 within {p99} levels");
        worst = (worst.0.max(max), worst.1.max(mean), worst.2.max(p99));
    }
    eprintln!(
        "worst: largest difference {}, mean {:.2}, 99 in 100 within {}",
        worst.0, worst.1, worst.2
    );
    preview.close(Duration::from_secs(5));
}

// ----------------------------------------------------------------------
// Sync over a long export

/// A recording of `minutes` at 30 fps, 64×36, with a white flash on the
/// first three frames of every second and a 1 kHz beep for the first
/// 100 ms of every second, otherwise black and silent.
fn flashes(path: &Path, minutes: u32) {
    const SIZE: (u32, u32) = (64, 36);
    let video = VideoSettings {
        codec: VideoCodec::H264,
        width: SIZE.0,
        height: SIZE.1,
        rate: (30, 1),
        bit_rate: 1_000_000,
        software: true,
    };
    let audio = AudioSettings {
        codec: AudioCodec::Flac,
        sample_rate: 48_000,
        bit_rate: 0,
    };
    let mut writer = Writer::create(path, Container::Mkv, Some(&video), Some(&audio)).unwrap();
    let white = nv12(SIZE, |_, _| [235, 128, 128]);
    let black = nv12(SIZE, |_, _| [16, 128, 128]);
    let tone: Vec<f32> = (0..1600)
        .flat_map(|i| {
            let s = (i as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            [s, s]
        })
        .collect();
    let quiet = vec![0.0f32; 3200];
    for frame in 0..minutes * 60 * 30 {
        let within = frame % 30;
        writer
            .write_picture(if within < 3 { &white } else { &black })
            .unwrap();
        writer
            .write_sound(if within < 3 { &tone } else { &quiet })
            .unwrap();
    }
    writer.finish().unwrap();
}

/// The flashes in `seconds` of `path` from `from` (whole seconds), as the
/// first frame of each: seconds.
fn flash_times(path: &Path, from: f64, seconds: u32) -> Vec<f64> {
    let made = media::describe(path).unwrap();
    let video = made.info.video.expect("a picture stream");
    let rate = video.frame_rate.as_f64();
    let mut decoder = media::VideoDecoder::open(path, video.index, 0, Decoding::Software).unwrap();
    let mut times = Vec::new();
    let mut was_bright = false;
    let first = (from * rate).round() as i64 - 2;
    for frame in first.max(0)..first.max(0) + (f64::from(seconds) * rate) as i64 {
        let at = (frame as f64 + 0.5) / rate;
        let media::Decoded::Yuv(picture) = decoder.picture_at(at, video.width, video.height).unwrap() else {
            panic!("planes expected");
        };
        let time = picture.time;
        let middle = picture.into_planes().as_ref()[(video.height as usize / 2) * video.width as usize + 4];
        let bright = middle > 128;
        if bright && !was_bright {
            times.push(time);
        }
        was_bright = bright;
    }
    times
}

/// The beeps in `seconds` of `path` from `from`, as where each starts: seconds.
fn beep_times(path: &Path, from: f64, seconds: u32) -> Vec<f64> {
    let made = media::describe(path).unwrap();
    let stream = &made.info.audio[0];
    let rate = stream.sample_rate;
    let mut sound = media::AudioDecoder::open(path, stream.index, rate).unwrap();
    let start = (from - 0.05).max(0.0);
    sound.seek(start);
    let mut heard = vec![0.0f32; (f64::from(rate) * f64::from(seconds)) as usize * 2];
    let got = sound.read(&mut heard);
    let mut times = Vec::new();
    let mut quiet_for = usize::MAX;
    for (i, pair) in heard[..got * 2].chunks_exact(2).enumerate() {
        if pair[0].abs() > 0.1 && pair[1].abs() > 0.1 {
            // A beep starts after at least 0.2 s of quiet.
            if quiet_for > rate as usize / 5 {
                times.push(start + i as f64 / f64::from(rate));
            }
            quiet_for = 0;
        } else {
            quiet_for = quiet_for.saturating_add(1);
        }
    }
    times
}

/// Checks the flashes and beeps of a window of the export: each on its
/// whole second, within a frame. Returns the furthest a flash, a beep and
/// a beep from its flash were: seconds.
fn check_window(path: &Path, from: f64, what: &str) -> (f64, f64, f64) {
    const SECONDS: u32 = 10;
    let flashes = flash_times(path, from, SECONDS);
    let beeps = beep_times(path, from, SECONDS);
    assert!(flashes.len() >= SECONDS as usize - 1, "{what}: {flashes:?}");
    assert!(beeps.len() >= SECONDS as usize - 1, "{what}: {beeps:?}");
    let off = |times: &[f64]| times.iter().map(|t| (t - t.round()).abs()).fold(0.0f64, f64::max);
    let (flash_off, beep_off) = (off(&flashes), off(&beeps));
    let apart = flashes
        .iter()
        .filter_map(|f| beeps.iter().find(|b| (*b - f).abs() < 0.5).map(|b| (b - f).abs()))
        .fold(0.0f64, f64::max);
    eprintln!(
        "{what} from {from} s: {} flashes, {} beeps; furthest from its second: flash {:.2} ms, beep {:.2} ms; beep from flash {:.2} ms",
        flashes.len(),
        beeps.len(),
        flash_off * 1000.0,
        beep_off * 1000.0,
        apart * 1000.0
    );
    let frame = 1.0 / 30.0;
    assert!(flash_off < frame, "{what}: a flash {flash_off} s off");
    assert!(beep_off < frame, "{what}: a beep {beep_off} s off");
    assert!(apart < frame, "{what}: beep and flash {apart} s apart");
    (flash_off, beep_off, apart)
}

#[test]
#[ignore = "long: run by hand (GATE_MINUTES, default 10)"]
fn sound_and_picture_stay_in_sync_over_a_long_export() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let minutes: u32 = std::env::var("GATE_MINUTES")
        .ok()
        .and_then(|m| m.parse().ok())
        .unwrap_or(10);
    // A ten-minute recording laid end to end, as often as it takes.
    let dir = tempfile::tempdir().unwrap();
    let piece = minutes.min(10);
    let source = dir.path().join("flashes.mkv");
    let made = Instant::now();
    flashes(&source, piece);
    eprintln!("a {piece}-minute recording written in {:.1?}", made.elapsed());
    let settings = SequenceSettings {
        width: 64,
        height: 36,
        frame_rate: Rate::FPS_30,
        sample_rate: 48_000,
    };
    let mut project = Project::new("Long", settings);
    let seq = project.active_sequence;
    project = Command::Import {
        sequence: seq,
        media: media::describe(&source).unwrap(),
    }
    .apply(&project)
    .unwrap();
    let media = *project.media.keys().next().unwrap();
    for _ in 1..minutes.div_ceil(piece) {
        project = Command::PlaceMedia { sequence: seq, media }
            .apply(&project)
            .unwrap();
    }
    let length = project.active().unwrap().length().as_secs_f64();
    eprintln!("the timeline is {:.0} s long", length);
    let out = out_dir(dir.path());
    for (container, audio) in [
        (Container::Mp4, AudioCodec::Aac),
        (Container::Mkv, AudioCodec::Opus),
    ] {
        let what = format!("{container:?} H.264 {audio:?}");
        let path = out.join(format!("long.{}", container.extension()));
        let settings = ExportSettings {
            container,
            audio,
            quality: Quality::Medium,
            software: true,
            ..ExportSettings::mp4(path.clone())
        };
        let export = Export::start(project.clone(), seq, settings, Some(devices.clone()), || {});
        let done = wait(&export, Duration::from_secs(6 * 3600));
        eprintln!(
            "{what}: {} frames in {:.1?} ({:.1} times real time)",
            done.frames,
            done.took,
            length / done.took.as_secs_f64()
        );
        let probed = media::describe(&path).unwrap();
        let file_length = probed.duration.as_secs_f64();
        eprintln!("{what}: the file is {file_length:.3} s long");
        assert!((file_length - length).abs() < 0.1, "{what}: {file_length} s");
        for from in [0.0, (length / 2.0).round(), length - 10.0] {
            check_window(&path, from, &what);
        }
    }
}

// ----------------------------------------------------------------------
// Speed

/// `seconds` of a 2560×1440 recording at 60 fps whose every frame differs
/// (a moving pattern of brightness and colour), with a tone.
fn large(path: &Path, seconds: u32) {
    const SIZE: (u32, u32) = (2560, 1440);
    let video = VideoSettings {
        codec: VideoCodec::H264,
        width: SIZE.0,
        height: SIZE.1,
        rate: (60, 1),
        bit_rate: 40_000_000,
        software: true,
    };
    let audio = AudioSettings {
        codec: AudioCodec::Aac,
        sample_rate: 48_000,
        bit_rate: 192_000,
    };
    let mut writer = Writer::create(path, Container::Mp4, Some(&video), Some(&audio)).unwrap();
    let mut n = 0u64;
    for frame in 0..seconds * 60 {
        let picture = nv12(SIZE, |x, y| {
            let luma = 16 + ((x / 4 + y / 3 + frame * 3) % 220);
            let u = 128 + (((x / 16 + frame) % 64) as i32 - 32);
            let v = 128 + (((y / 16 + frame * 2) % 64) as i32 - 32);
            [luma as u8, u as u8, v as u8]
        });
        writer.write_picture(&picture).unwrap();
        let sound: Vec<f32> = (0..800)
            .flat_map(|_| {
                n += 1;
                let s = (n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.2;
                [s, s]
            })
            .collect();
        writer.write_sound(&sound).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
#[ignore = "slow: run by hand (GATE_SECONDS, default 10)"]
fn export_speed_of_a_1440p60_timeline_on_the_processor() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let seconds: u32 = std::env::var("GATE_SECONDS")
        .ok()
        .and_then(|m| m.parse().ok())
        .unwrap_or(10);
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("large.mp4");
    let made = Instant::now();
    large(&source, seconds);
    eprintln!(
        "a {seconds} s 1440p60 recording written in {:.1?}",
        made.elapsed()
    );
    let mut project = Project::new_automatic("Speed");
    let seq = project.active_sequence;
    project = Command::Import {
        sequence: seq,
        media: media::describe(&source).unwrap(),
    }
    .apply(&project)
    .unwrap();
    let s = project.active().unwrap().settings;
    assert_eq!((s.width, s.height, s.frame_rate), (2560, 1440, Rate::FPS_60));
    let path = out_dir(dir.path()).join("speed.mp4");
    let settings = ExportSettings {
        quality: Quality::High,
        software: true,
        ..ExportSettings::mp4(path)
    };
    let export = Export::start(project, seq, settings, Some(devices), || {});
    let done = wait(&export, Duration::from_secs(4 * 3600));
    let pace = f64::from(seconds) / done.took.as_secs_f64();
    eprintln!(
        "{} frames of 2560×1440 at 60 fps with {} in {:.1?}: {:.2} times real time ({:.1} frames a second)",
        done.frames,
        done.encoder,
        done.took,
        pace,
        done.frames as f64 / done.took.as_secs_f64()
    );
}

// ----------------------------------------------------------------------
// What video sites and players are handed

#[test]
#[ignore = "slow: run by hand with GATE_KEEP, then look at the files"]
fn every_built_in_presets_file_for_a_1440p60_timeline() {
    let Some(devices) = devices() else {
        eprintln!("no graphics device here: exporting isn't checked");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("large.mp4");
    large(&source, 3);
    let mut project = Project::new_automatic("Presets");
    let seq = project.active_sequence;
    project = Command::Import {
        sequence: seq,
        media: media::describe(&source).unwrap(),
    }
    .apply(&project)
    .unwrap();
    let out = out_dir(dir.path());
    // As the Export page's built-in presets set them (`app`'s
    // `export_page::presets`), for a 2560×1440 timeline.
    let web = |name: &str, size, video| ExportSettings {
        quality: Quality::High,
        software: true,
        normalise: Some(-14.0),
        limit: true,
        size: Some(size),
        video: Some(video),
        ..ExportSettings::mp4(out.join(name))
    };
    let presets = [
        web("web-1080p.mp4", (1920, 1080), VideoCodec::H264),
        web("web-4k.mp4", (3840, 2160), VideoCodec::H265),
        web("web-4k-h264.mp4", (3840, 2160), VideoCodec::H264),
        web("vertical.mp4", (1080, 1920), VideoCodec::H264),
        ExportSettings {
            container: Container::Mkv,
            video: Some(VideoCodec::H265),
            audio: AudioCodec::Flac,
            quality: Quality::High,
            software: true,
            ..ExportSettings::mp4(out.join("archive.mkv"))
        },
        ExportSettings {
            quality: Quality::Small,
            software: true,
            size: Some((1280, 720)),
            ..ExportSettings::mp4(out.join("small.mp4"))
        },
    ];
    for settings in presets {
        let name = settings.path.clone();
        let export = Export::start(project.clone(), seq, settings, Some(devices.clone()), || {});
        let done = wait(&export, Duration::from_secs(3600));
        eprintln!(
            "{}: {:?} with {} in {:.1?}",
            name.display(),
            done.size,
            done.encoder,
            done.took
        );
    }
}
