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
