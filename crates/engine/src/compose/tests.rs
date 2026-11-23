//! The composer with picture effects, on whatever graphics device this
//! machine has (a software one will do; without any, nothing is checked).

use super::*;
use project::{ClipId, MediaId, Time};

fn composer() -> Option<Composer> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Composer::new(Composing {
        device,
        queue,
        wrap: None,
    }))
}

fn white(size: (u32, u32)) -> Decoded {
    Decoded::Rgba(media::Frame {
        width: size.0,
        height: size.1,
        rgba: vec![255; (size.0 * size.1 * 4) as usize],
        time: 0.0,
    })
}

fn layer(area: Area, effects: Vec<render::Effect>) -> Layer {
    Layer {
        clip: ClipId(1),
        media: MediaId(1),
        stream: 0,
        source_time: Time::ZERO,
        area,
        part: Part::WHOLE,
        opacity: 1.0,
        colour: Colour::NEUTRAL,
        effects,
        mode: render::Mode::Normal,
        blend: None,
        title: None,
    }
}

/// The light in each pixel of a composed frame (16-bit floats), red only.
fn light(composer: &Composer, composed: &Composed) -> Vec<f32> {
    let (device, queue) = (&composer.composing.device, &composer.composing.queue);
    let (w, h) = composed.size();
    let row = (w * 8).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        composed.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    mapped
        .chunks(row as usize)
        .flat_map(|r| {
            r[..w as usize * 8]
                .chunks(8)
                .map(|p| half(u16::from_le_bytes([p[0], p[1]])))
        })
        .collect()
}

/// A 16-bit float as an f32.
fn half(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff) / 1024.0;
    match exponent {
        0 => sign * fraction * 2f32.powi(-14),
        _ => sign * (1.0 + fraction) * 2f32.powi(exponent - 15),
    }
}

const HARD: render::Effect = render::Effect::Vignette {
    amount: 1.0,
    size: 0.5,
    softness: 0.0,
};

#[test]
fn a_clips_effects_change_its_own_picture_before_it_is_placed() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: effects aren't checked");
        return;
    };
    let size = (64, 64);
    // Filling the frame: its corners are the frame's.
    let whole = FrameDescription {
        size,
        layers: vec![layer(Area::fitted(size, size), vec![HARD])],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
    let shown = light(&composer, &composed);
    let at = |x: u32, y: u32| shown[(y * size.0 + x) as usize];
    assert!(at(32, 32) > 0.99, "the middle is as it was: {}", at(32, 32));
    assert!(at(0, 0) < 0.01, "the corners are dark: {}", at(0, 0));
    // A quarter of the frame in its middle: darkened at its own corners,
    // not the frame's, and nothing drawn outside it.
    let inside = Area::fitted(size, size).scaled(0.5, 0.5);
    let moved = Area {
        origin: (16.0, 16.0),
        ..inside
    };
    let small = FrameDescription {
        size,
        layers: vec![layer(moved, vec![HARD])],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&small, vec![(0, white(size), None)]).unwrap();
    let shown = light(&composer, &composed);
    let at = |x: u32, y: u32| shown[(y * size.0 + x) as usize];
    assert!(at(32, 32) > 0.99, "its middle: {}", at(32, 32));
    assert!(at(17, 17) < 0.05, "its corner: {}", at(17, 17));
    assert!(at(2, 2) < 0.01, "black around it: {}", at(2, 2));
    // Its own middle and edges, in its own shape.
    assert!(at(28, 32) > 0.99, "near its middle: {}", at(28, 32));
    assert!(at(17, 32) < 0.01, "at its left edge: {}", at(17, 32));
}

#[test]
fn an_outline_is_drawn_past_the_pictures_place_in_the_frame() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: effects aren't checked");
        return;
    };
    let size = (64, 64);
    // A white picture a quarter of the frame, in its middle, with a red
    // line 4 pixels of its 32 lines around it.
    let quarter = Area {
        origin: (16.0, 16.0),
        ..Area::fitted(size, size).scaled(0.5, 0.5)
    };
    let outline = render::Effect::Outline {
        width: 4.0 * 1080.0 / 32.0,
        colour: [1.0, 0.0, 0.0],
    };
    let framed = FrameDescription {
        size,
        layers: vec![layer(quarter, vec![outline])],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&framed, vec![(0, white(size), None)]).unwrap();
    let shown = light(&composer, &composed);
    let at = |x: u32, y: u32| shown[(y * size.0 + x) as usize];
    assert!(at(32, 32) > 0.99, "the picture: {}", at(32, 32));
    assert!(at(14, 32) > 0.9, "the line beside it, in red: {}", at(14, 32));
    assert!(at(4, 32) < 0.01, "nothing further out: {}", at(4, 32));
}

#[test]
fn a_grade_changes_the_light_of_the_picture() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: grades aren't checked");
        return;
    };
    let size = (16, 16);
    let shown_with = |composer: &mut Composer, grade: render::GradeNumbers| {
        let whole = FrameDescription {
            size,
            layers: vec![layer(
                Area::fitted(size, size),
                vec![render::Effect::Grade(grade)],
            )],
            steps: vec![Step::Picture(0)],
        };
        let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
        light(composer, &composed)[8 * 16 + 8]
    };
    let plain = shown_with(&mut composer, render::GradeNumbers::PLAIN);
    assert!((plain - 1.0).abs() < 0.01, "a plain grade leaves it: {plain}");
    let darker = render::GradeNumbers {
        exposure: -1.0,
        ..render::GradeNumbers::PLAIN
    };
    let half = shown_with(&mut composer, darker);
    assert!((half - 0.5).abs() < 0.01, "a stop down halves the light: {half}");
    let lifted = render::GradeNumbers {
        exposure: -3.0,
        shadows_wheel: [0.0, 0.0, 0.0, 0.5],
        ..render::GradeNumbers::PLAIN
    };
    let dark = shown_with(
        &mut composer,
        render::GradeNumbers {
            exposure: -3.0,
            ..render::GradeNumbers::PLAIN
        },
    );
    assert!(
        shown_with(&mut composer, lifted) > dark,
        "the shadows wheel lifts the dark"
    );
}

#[test]
fn a_small_copy_for_the_scopes_reads_the_picture_back() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: the scopes' copy isn't checked");
        return;
    };
    let size = (64, 36);
    let whole = FrameDescription {
        size,
        layers: vec![layer(Area::fitted(size, size), Vec::new())],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
    let sampled = composer.sample(&composed, 32).expect("read back");
    assert_eq!((sampled.width, sampled.height), (32, 18));
    assert!(sampled.rgb.iter().all(|p| p.iter().all(|v| *v > 0.98)), "white");
    assert_eq!(sampled.reading(), "100 % of the picture is clipped at white");
}

#[test]
fn a_grade_curve_bends_the_light() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: curves aren't checked");
        return;
    };
    let size = (16, 16);
    let mut layer_grade = project::GradeLayer::new("Base");
    // White comes down to half on the display-like scale.
    layer_grade.curves.master = project::Curve(vec![(1_000_000, 500_000)]);
    let numbers = crate::frame::grade_numbers(&layer_grade);
    assert!(numbers.curves.is_some());
    let whole = FrameDescription {
        size,
        layers: vec![layer(
            Area::fitted(size, size),
            vec![render::Effect::Grade(numbers)],
        )],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
    let shown = light(&composer, &composed)[8 * 16 + 8];
    let expected = 0.5f32.powf(2.4);
    assert!((shown - expected).abs() < 0.02, "{shown} against {expected}");
}

#[test]
fn a_grade_keeps_to_its_mask_and_its_colour_range() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: masks and ranges aren't checked");
        return;
    };
    let size = (16, 16);
    let mut shown_with = |grade: &project::GradeLayer| {
        let whole = FrameDescription {
            size,
            layers: vec![layer(
                Area::fitted(size, size),
                vec![render::Effect::Grade(crate::frame::grade_numbers(grade))],
            )],
            steps: vec![Step::Picture(0)],
        };
        let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
        light(&composer, &composed)
    };
    let mut darker = project::GradeLayer::new("Left");
    darker.primary.exposure = -1_000_000;
    darker.mask = Some(project::Mask {
        shape: project::MaskShape::Rectangle,
        x: 250_000,
        y: 500_000,
        width: 500_000,
        height: 1_000_000,
        turn: 0,
        feather: 10_000,
        invert: false,
    });
    let masked = shown_with(&darker);
    let (left, right) = (masked[8 * 16 + 2], masked[8 * 16 + 13]);
    assert!((left - 0.5).abs() < 0.02, "inside, a stop down: {left}");
    assert!((right - 1.0).abs() < 0.02, "outside, left alone: {right}");
    if let Some(mask) = darker.mask.as_mut() {
        mask.invert = true;
    }
    let inverted = shown_with(&darker);
    assert!(inverted[8 * 16 + 2] > 0.98 && inverted[8 * 16 + 13] < 0.52);
    // White has no colour, so a range on skin leaves it.
    darker.mask = None;
    darker.range = Some(project::ColorRange::SKIN);
    let ranged = shown_with(&darker)[8 * 16 + 8];
    assert!((ranged - 1.0).abs() < 0.02, "white isn't skin: {ranged}");
}

#[test]
fn a_graphics_error_is_told_for_the_frame_and_the_next_frame_is_drawn() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: graphics errors aren't checked");
        return;
    };
    // Wider than the card can draw: the library refuses the frame, which
    // used to end the preview's thread.
    let too_wide = (composer.composing.device.limits().max_texture_dimension_2d + 1, 8);
    let refused = FrameDescription {
        size: too_wide,
        layers: vec![layer(Area::fitted((16, 8), too_wide), Vec::new())],
        steps: vec![Step::Picture(0)],
    };
    match composer.compose(&refused, vec![(0, white((16, 8)), None)]) {
        Err(NotComposed::Graphics(err)) => {
            assert!(
                err.why_text().is_some_and(|why| !why.is_empty()),
                "{}",
                err.report()
            );
        }
        other => panic!("expected a graphics error, got {other:?}"),
    }
    let size = (16, 16);
    let whole = FrameDescription {
        size,
        layers: vec![layer(Area::fitted(size, size), Vec::new())],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
    assert!(light(&composer, &composed)[8 * 16 + 8] > 0.98, "white");
    assert!(composer.sample(&composed, 8).is_some());
}

#[test]
fn the_scopes_copy_is_kept_while_its_size_stays_the_same() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: the scopes' copy isn't checked");
        return;
    };
    let size = (64, 36);
    let whole = FrameDescription {
        size,
        layers: vec![layer(Area::fitted(size, size), Vec::new())],
        steps: vec![Step::Picture(0)],
    };
    let composed = composer.compose(&whole, vec![(0, white(size), None)]).unwrap();
    let buffer = |composer: &Composer| composer.scope_copy.as_ref().map(|copy| copy.buffer.clone());
    assert!(composer.sample(&composed, 32).is_some());
    let first = buffer(&composer).expect("kept");
    assert!(composer.sample(&composed, 32).is_some());
    assert_eq!(buffer(&composer).as_ref(), Some(&first), "the same buffer");
    let smaller = composer.sample(&composed, 16).expect("read back");
    assert_eq!((smaller.width, smaller.height), (16, 9));
    assert_ne!(
        buffer(&composer).as_ref(),
        Some(&first),
        "a new one for a new size"
    );
}

#[test]
fn upload_slots_for_layers_no_longer_used_are_let_go() {
    let Some(mut composer) = composer() else {
        eprintln!("no graphics device here: upload slots aren't checked");
        return;
    };
    let size = (16, 16);
    let with = |layers: usize| FrameDescription {
        size,
        layers: (0..layers)
            .map(|_| layer(Area::fitted(size, size), Vec::new()))
            .collect(),
        steps: (0..layers).map(Step::Picture).collect(),
    };
    let pictures = |layers: usize| (0..layers).map(|i| (i, white(size), None)).collect();
    composer.compose(&with(5), pictures(5)).unwrap();
    assert_eq!(composer.uploads.len(), 10);
    // A moment with fewer layers keeps them for a while...
    composer.compose(&with(1), pictures(1)).unwrap();
    assert_eq!(composer.uploads.len(), 10);
    // ...and lets them go when frames keep needing fewer.
    for _ in 0..UPLOADS_IDLE_FOR {
        composer.compose(&with(1), pictures(1)).unwrap();
    }
    assert_eq!(composer.uploads.len(), 2);
    composer.compose(&with(2), pictures(2)).unwrap();
    assert_eq!(composer.uploads.len(), 4);
}
