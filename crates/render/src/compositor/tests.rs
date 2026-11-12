//! The compositor against known colours, drawn on whatever graphics device
//! this machine has (a software one will do). Without any device there is
//! nothing to draw with: the checks say so and pass.

use super::*;

type Gpu = (wgpu::Device, wgpu::Queue);

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
    let Ok(adapter) = adapter else {
        eprintln!("no graphics device here: the compositor isn't checked");
        return None;
    };
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(pair) => Some(pair),
        Err(err) => {
            eprintln!("the graphics device didn't open ({err}): the compositor isn't checked");
            None
        }
    }
}

/// BT.709, limited range: what most recordings are.
const TO_RGB: [[f32; 4]; 3] = {
    let (y, black, none) = (255.0 / 219.0, 16.0 / 255.0, 128.0 / 255.0);
    let (rv, gu, gv, bu) = (1.792_74, -0.213_25, -0.532_91, 2.112_40);
    [
        [y, 0.0, rv, -y * black - rv * none],
        [y, gu, gv, -y * black - (gu + gv) * none],
        [y, bu, 0.0, -y * black - bu * none],
    ]
};

const GREY: [u8; 3] = [126, 128, 128];
const WHITE: [u8; 3] = [235, 128, 128];

fn texture(
    (device, queue): &Gpu,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes.len() as u32 / height),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A picture's planes: brightness from `luma` (by position), one colour.
struct Planes {
    luma: wgpu::TextureView,
    chroma: wgpu::TextureView,
    u: wgpu::TextureView,
    v: wgpu::TextureView,
}

impl Planes {
    fn new(gpu: &Gpu, (width, height): (u32, u32), luma: impl Fn(u32, u32) -> u8, [u, v]: [u8; 2]) -> Planes {
        let bytes: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| luma(x, y))
            .collect();
        let half = (width.div_ceil(2), height.div_ceil(2));
        let count = (half.0 * half.1) as usize;
        Planes {
            luma: texture(gpu, (width, height), wgpu::TextureFormat::R8Unorm, &bytes),
            chroma: texture(gpu, half, wgpu::TextureFormat::Rg8Unorm, &[u, v].repeat(count)),
            u: texture(gpu, half, wgpu::TextureFormat::R8Unorm, &vec![u; count]),
            v: texture(gpu, half, wgpu::TextureFormat::R8Unorm, &vec![v; count]),
        }
    }

    fn flat(gpu: &Gpu, size: (u32, u32), yuv: [u8; 3]) -> Planes {
        Planes::new(gpu, size, |_, _| yuv[0], [yuv[1], yuv[2]])
    }

    fn two(&self) -> Pixels<'_> {
        Pixels::TwoPlanes {
            luma: &self.luma,
            chroma: &self.chroma,
        }
    }

    fn three(&self) -> Pixels<'_> {
        Pixels::ThreePlanes {
            luma: &self.luma,
            u: &self.u,
            v: &self.v,
        }
    }
}

fn whole(pixels: Pixels<'_>, frame: (u32, u32)) -> Drawn<'_> {
    Drawn {
        pixels,
        to_rgb: TO_RGB,
        rotation: 0,
        area: Area::fitted(frame, frame),
        part: Part::WHOLE,
        opacity: 1.0,
        colour: Colour::NEUTRAL,
        blend: None,
    }
}

/// A finished picture as rows of RGBA bytes.
struct Shown {
    width: u32,
    bytes: Vec<u8>,
}

impl Shown {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.bytes[i],
            self.bytes[i + 1],
            self.bytes[i + 2],
            self.bytes[i + 3],
        ]
    }
}

/// Puts `layers` together in a frame of `size`, shows it in an 8-bit target
/// of `target` format and reads that back.
fn compose(
    gpu: &Gpu,
    working: Working,
    target: wgpu::TextureFormat,
    size: (u32, u32),
    layers: &[Drawn<'_>],
) -> Shown {
    let (device, queue) = gpu;
    let compositor = Compositor::new(device, working);
    let shower = Shower::new(device, target);
    let mut frame = compositor.frame(device, size);
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
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
    let window_view = window.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    compositor.draw(device, queue, &mut encoder, &mut frame, layers);
    let showing = shower.showing(device, queue, frame.view(), frame.working());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &window_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        shower.draw(&mut pass, &showing);
    }
    // Rows in a buffer start at multiples of 256 bytes.
    let row = (size.0 * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * size.1),
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
                rows_per_image: Some(size.1),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    Shown {
        width: size.0,
        bytes: mapped
            .chunks(row as usize)
            .flat_map(|r| r[..size.0 as usize * 4].to_vec())
            .collect(),
    }
}

const EIGHT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BOTH: [Working; 2] = [Working::Linear, Working::Display];

fn near(got: [u8; 4], want: [u8; 3], slack: u8) -> bool {
    (0..3).all(|i| got[i].abs_diff(want[i]) <= slack)
}

#[test]
fn a_picture_that_fills_the_frame_comes_out_as_it_went_in() {
    let Some(gpu) = gpu() else { return };
    let size = (64, 36);
    let grey = Planes::flat(&gpu, size, GREY);
    for working in BOTH {
        for pixels in [grey.two(), grey.three()] {
            let shown = compose(&gpu, working, EIGHT, size, &[whole(pixels, size)]);
            // (126 - 16) / 219 of full white.
            for at in [(0, 0), (32, 18), (63, 35)] {
                let got = shown.at(at.0, at.1);
                assert!(near(got, [128; 3], 1), "{working:?} at {at:?}: {got:?}");
                assert_eq!(got[3], 255);
            }
        }
    }
}

#[test]
fn a_target_that_stores_srgb_shows_the_same_picture() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let grey = Planes::flat(&gpu, size, GREY);
    for working in BOTH {
        let shown = compose(
            &gpu,
            working,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            size,
            &[whole(grey.two(), size)],
        );
        assert!(
            near(shown.at(8, 8), [128; 3], 1),
            "{working:?}: {:?}",
            shown.at(8, 8)
        );
    }
}

#[test]
fn a_wider_picture_has_black_above_and_below() {
    let Some(gpu) = gpu() else { return };
    let (frame, picture) = ((64, 64), (64, 32));
    let white = Planes::flat(&gpu, picture, WHITE);
    let layer = Drawn {
        area: Area::fitted(picture, frame),
        ..whole(white.two(), frame)
    };
    for working in BOTH {
        let shown = compose(&gpu, working, EIGHT, frame, &[layer]);
        assert!(near(shown.at(32, 32), [255; 3], 1), "{working:?} middle");
        assert!(
            near(shown.at(32, 20), [255; 3], 1),
            "{working:?} inside the top edge"
        );
        assert!(near(shown.at(32, 10), [0; 3], 0), "{working:?} above");
        assert!(near(shown.at(32, 54), [0; 3], 0), "{working:?} below");
    }
}

#[test]
fn half_strength_white_mixes_as_light_in_linear_and_as_numbers_in_display() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let (grey, white) = (Planes::flat(&gpu, size, GREY), Planes::flat(&gpu, size, WHITE));
    let layers = [
        whole(grey.two(), size),
        Drawn {
            opacity: 0.5,
            ..whole(white.two(), size)
        },
    ];
    // Halfway between 128 and 255 as numbers.
    let display = compose(&gpu, Working::Display, EIGHT, size, &layers).at(8, 8);
    assert!(near(display, [192; 3], 1), "{display:?}");
    // Halfway between their light: ((0.502^2.4 + 1) / 2)^(1/2.4) = 0.805.
    let linear = compose(&gpu, Working::Linear, EIGHT, size, &layers).at(8, 8);
    assert!(near(linear, [205; 3], 1), "{linear:?}");
}

#[test]
fn only_the_chosen_part_of_the_picture_is_shown() {
    let Some(gpu) = gpu() else { return };
    let size = (64, 32);
    // Dark on the left half, bright on the right.
    let halves = Planes::new(&gpu, size, |x, _| if x < 32 { 16 } else { 235 }, [128, 128]);
    let right = Drawn {
        part: Part {
            left: 0.5,
            top: 0.0,
            width: 0.5,
            height: 1.0,
        },
        ..whole(halves.two(), size)
    };
    let shown = compose(&gpu, Working::Linear, EIGHT, size, &[right]);
    assert!(near(shown.at(4, 16), [255; 3], 1), "{:?}", shown.at(4, 16));
    assert!(near(shown.at(60, 16), [255; 3], 1));
    let all = compose(&gpu, Working::Linear, EIGHT, size, &[whole(halves.two(), size)]);
    assert!(near(all.at(4, 16), [0; 3], 1));
    assert!(near(all.at(60, 16), [255; 3], 1));
}

#[test]
fn a_picture_stored_lying_down_is_shown_upright() {
    let Some(gpu) = gpu() else { return };
    // Stored 64 wide: dark on its left half. Shown a quarter turn clockwise,
    // the stored left edge is the top.
    let stored = Planes::new(&gpu, (64, 32), |x, _| if x < 32 { 16 } else { 235 }, [128, 128]);
    let frame = (32, 64);
    let layer = Drawn {
        rotation: 90,
        ..whole(stored.two(), frame)
    };
    let shown = compose(&gpu, Working::Display, EIGHT, frame, &[layer]);
    assert!(near(shown.at(16, 8), [0; 3], 1), "top: {:?}", shown.at(16, 8));
    assert!(
        near(shown.at(16, 56), [255; 3], 1),
        "bottom: {:?}",
        shown.at(16, 56)
    );
}

#[test]
fn a_see_through_rgba_picture_lets_the_layer_below_show() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let grey = Planes::flat(&gpu, size, GREY);
    // Solid red on the left half, nothing at all on the right.
    let bytes: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            if i % 16 < 8 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    let rgba = texture(&gpu, size, EIGHT, &bytes);
    let layers = [whole(grey.two(), size), whole(Pixels::Rgba(&rgba), size)];
    for working in BOTH {
        let shown = compose(&gpu, working, EIGHT, size, &layers);
        assert!(
            near(shown.at(3, 8), [255, 0, 0], 1),
            "{working:?}: {:?}",
            shown.at(3, 8)
        );
        assert!(
            near(shown.at(12, 8), [128; 3], 1),
            "{working:?}: {:?}",
            shown.at(12, 8)
        );
    }
}

#[test]
fn a_smaller_turned_layer_sits_where_its_area_says() {
    let Some(gpu) = gpu() else { return };
    let size = (64, 64);
    let (grey, white) = (Planes::flat(&gpu, size, GREY), Planes::flat(&gpu, size, WHITE));
    // A square of 20 turned 45 degrees around the middle of the frame: its
    // corners point up, right, down and left.
    let reach = 20.0 / std::f32::consts::SQRT_2;
    let diamond = Drawn {
        area: Area {
            origin: (32.0, 32.0 - reach),
            across: (reach, reach),
            down: (-reach, reach),
        },
        ..whole(white.two(), size)
    };
    let shown = compose(
        &gpu,
        Working::Linear,
        EIGHT,
        size,
        &[whole(grey.two(), size), diamond],
    );
    assert!(near(shown.at(32, 32), [255; 3], 1), "the middle is the layer");
    assert!(near(shown.at(32, 22), [255; 3], 1), "towards its top corner too");
    assert!(
        near(shown.at(22, 22), [128; 3], 1),
        "but not where a square's corner would be"
    );
    assert!(near(shown.at(4, 4), [128; 3], 1));
}

#[test]
fn quick_colour_works_on_light_the_same_in_both_kinds_of_frame() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let grey = Planes::flat(&gpu, size, GREY);
    let with = |colour: Colour, working| {
        let layer = Drawn {
            colour,
            ..whole(grey.two(), size)
        };
        compose(&gpu, working, EIGHT, size, &[layer]).at(8, 8)
    };
    for working in BOTH {
        // A stop brighter is twice the light: (2 x 0.502^2.4)^(1/2.4) = 0.67.
        let brighter = with(Colour::from_sliders(1.0, 0.0, 0.0, 0.0, 0.0), working);
        assert!(near(brighter, [171; 3], 2), "{working:?}: {brighter:?}");
        // Warmer: more red, less blue, about as bright.
        let warm = with(Colour::from_sliders(0.0, 0.0, 0.0, 0.5, 0.0), working);
        assert!(
            warm[0] > 135 && warm[2] < 120 && warm[1].abs_diff(128) < 12,
            "{working:?}: {warm:?}"
        );
        // More magenta: less green.
        let magenta = with(Colour::from_sliders(0.0, 0.0, 0.0, 0.0, 0.5), working);
        assert!(
            magenta[1] < magenta[0] && magenta[1] < magenta[2],
            "{working:?}: {magenta:?}"
        );
        // Harder contrast pushes a value above middle grey up; no colour
        // at all leaves a grey as it is.
        let harder = with(Colour::from_sliders(0.0, 0.5, 0.0, 0.0, 0.0), working);
        assert!(harder[0] > 128 && harder[0] < 140, "{working:?}: {harder:?}");
        let no_colour = with(Colour::from_sliders(0.0, 0.0, -1.0, 0.0, 0.0), working);
        assert!(near(no_colour, [128; 3], 1), "{working:?}: {no_colour:?}");
    }
    // Without colour, red becomes the grey that is as bright: 0.2126 of the
    // light, 0.2126^(1/2.4) = 0.52 on screen.
    let red = texture(&gpu, size, EIGHT, &[255, 0, 0, 255].repeat(16 * 16));
    let layer = Drawn {
        colour: Colour::from_sliders(0.0, 0.0, -1.0, 0.0, 0.0),
        ..whole(Pixels::Rgba(&red), size)
    };
    let shown = compose(&gpu, Working::Linear, EIGHT, size, &[layer]).at(8, 8);
    assert!(near(shown, [134; 3], 2), "{shown:?}");
}

/// Puts `layers` together and reads the frame back as an encoder would get it.
fn read_back(gpu: &Gpu, working: Working, size: (u32, u32), layers: &[Drawn<'_>]) -> Vec<u8> {
    let (device, queue) = gpu;
    let compositor = Compositor::new(device, working);
    let mut frame = compositor.frame(device, size);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    compositor.draw(device, queue, &mut encoder, &mut frame, layers);
    queue.submit([encoder.finish()]);
    let mut reader = crate::Reader::new(device, size);
    let mut out = Vec::new();
    reader
        .read(device, queue, frame.view(), working, &mut out)
        .unwrap();
    assert_eq!(out.len(), reader.bytes());
    out
}

#[test]
fn a_frame_read_back_for_an_encoder_has_the_planes_it_was_made_from() {
    let Some(gpu) = gpu() else { return };
    let size = (64, 36);
    // A saturated red in BT.709 limited range, and the usual grey.
    for yuv in [[63, 102, 240], GREY, [16, 128, 128], WHITE] {
        let picture = Planes::flat(&gpu, size, yuv);
        for working in BOTH {
            let nv12 = read_back(&gpu, working, size, &[whole(picture.two(), size)]);
            let (luma, chroma) = nv12.split_at(64 * 36);
            assert!(
                luma.iter().all(|&y| y.abs_diff(yuv[0]) <= 1),
                "{working:?} {yuv:?}: {}",
                luma[0]
            );
            assert!(
                chroma
                    .chunks(2)
                    .all(|uv| uv[0].abs_diff(yuv[1]) <= 1 && uv[1].abs_diff(yuv[2]) <= 1),
                "{working:?} {yuv:?}: {:?}",
                &chroma[..2]
            );
        }
    }
    // Reading again and again uses the same buffers.
    let grey = Planes::flat(&gpu, size, GREY);
    let (device, queue) = &gpu;
    let compositor = Compositor::new(device, Working::Linear);
    let mut frame = compositor.frame(device, size);
    let mut reader = crate::Reader::new(device, size);
    let mut out = Vec::new();
    for _ in 0..3 {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        compositor.draw(
            device,
            queue,
            &mut encoder,
            &mut frame,
            &[whole(grey.two(), size)],
        );
        queue.submit([encoder.finish()]);
        reader
            .read(device, queue, frame.view(), Working::Linear, &mut out)
            .unwrap();
        assert_eq!(out[100], 126);
    }
}

#[test]
fn rows_come_back_without_the_padding_the_card_adds() {
    let Some(gpu) = gpu() else { return };
    // 70 wide: rows of 70 and 70 bytes, padded to 256 on the way.
    let size = (70, 10);
    let halves = Planes::new(&gpu, size, |x, _| if x < 35 { 16 } else { 235 }, [128, 128]);
    let nv12 = read_back(&gpu, Working::Display, size, &[whole(halves.two(), size)]);
    assert_eq!(nv12.len(), 70 * 10 * 3 / 2);
    for row in nv12[..700].chunks(70) {
        assert!(row[2] <= 17 && row[67] >= 234, "{} {}", row[2], row[67]);
    }
    assert_eq!(
        crate::reader::black((4, 2)),
        [16, 16, 16, 16, 16, 16, 16, 16, 128, 128, 128, 128]
    );
    assert_eq!(
        crate::Reader::new(&gpu.0, (71, 11)).size(),
        (70, 10),
        "odd sides lose a pixel"
    );
}

#[test]
fn a_frame_is_reused_for_the_next_picture() {
    let Some(gpu) = gpu() else { return };
    let (device, queue) = &gpu;
    let size = (16, 16);
    let grey = Planes::flat(&gpu, size, GREY);
    let compositor = Compositor::new(device, Working::Linear);
    let mut frame = compositor.frame(device, size);
    assert_eq!(frame.size(), size);
    for layers in [2, 1, 3] {
        let drawn = vec![whole(grey.two(), size); layers];
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        compositor.draw(device, queue, &mut encoder, &mut frame, &drawn);
        queue.submit([encoder.finish()]);
    }
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(frame.uniforms.len(), 3, "one buffer a layer, kept");
}

#[test]
fn a_part_of_a_turned_picture_is_a_part_of_what_is_seen() {
    let Some(gpu) = gpu() else { return };
    // Stored 64 wide, brighter in three steps from left to right. Shown a
    // quarter turn clockwise, the steps run from top to bottom.
    let stored = Planes::new(
        &gpu,
        (64, 32),
        |x, _| [16, 126, 235][(x * 3 / 64) as usize],
        [128, 128],
    );
    let frame = (32, 32);
    let lower_half = Drawn {
        rotation: 90,
        part: Part {
            left: 0.0,
            top: 0.5,
            width: 1.0,
            height: 0.5,
        },
        ..whole(stored.two(), frame)
    };
    let shown = compose(&gpu, Working::Display, EIGHT, frame, &[lower_half]);
    // The lower half holds the end of the middle step and the bright one.
    assert!(near(shown.at(16, 3), [128; 3], 1), "top: {:?}", shown.at(16, 3));
    assert!(
        near(shown.at(16, 28), [255; 3], 1),
        "bottom: {:?}",
        shown.at(16, 28)
    );
    assert!(near(shown.at(2, 28), [255; 3], 1), "the same all the way across");
}

#[test]
fn two_frames_blend_as_light_mixes_in_both_working_formats() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let black = Planes::flat(&gpu, size, [16, 128, 128]);
    let white = Planes::flat(&gpu, size, WHITE);
    let blended = Drawn {
        blend: Some((white.two(), 0.5)),
        ..whole(black.two(), size)
    };
    // Half the light of white: 0.5^(1/2.4) = 0.749 of the way up.
    for working in BOTH {
        let shown = compose(&gpu, working, EIGHT, size, &[blended]).at(8, 8);
        assert!(near(shown, [191; 3], 1), "{working:?}: {shown:?}");
    }
    // A share of nothing leaves the first picture as it is.
    let none = Drawn {
        blend: Some((black.two(), 0.0)),
        ..whole(white.two(), size)
    };
    let shown = compose(&gpu, Working::Linear, EIGHT, size, &[none]).at(8, 8);
    assert!(near(shown, [255; 3], 1), "{shown:?}");
    // A second picture stored another way is left out.
    let mismatched = Drawn {
        blend: Some((white.three(), 1.0)),
        ..whole(black.two(), size)
    };
    let shown = compose(&gpu, Working::Linear, EIGHT, size, &[mismatched]).at(8, 8);
    assert!(near(shown, [0; 3], 1), "{shown:?}");
}

#[test]
fn a_picture_the_compositor_made_is_drawn_again_as_it_was_and_coloured() {
    let Some(gpu) = gpu() else { return };
    let size = (16, 16);
    let grey = Planes::flat(&gpu, size, GREY);
    for working in BOTH {
        // Grey over the left half of a see-through picture (a nested
        // timeline's, where nothing covers the right half).
        let (device, queue) = &gpu;
        let compositor = Compositor::new(device, working);
        let mut made = compositor.frame(device, size);
        let left = Drawn {
            area: Area::fitted((8, 16), (8, 16)),
            ..whole(grey.two(), size)
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        compositor.draw_over(device, queue, &mut encoder, &mut made, &[left], Background::Clear);
        queue.submit([encoder.finish()]);
        let again = |colour: Colour| Drawn {
            pixels: Pixels::Made(made.view()),
            to_rgb: [[0.0; 4]; 3],
            colour,
            ..whole(grey.two(), size)
        };
        let shown = compose(&gpu, working, EIGHT, size, &[again(Colour::NEUTRAL)]);
        assert!(
            near(shown.at(4, 8), [128; 3], 1),
            "{working:?}: {:?}",
            shown.at(4, 8)
        );
        assert!(
            near(shown.at(12, 8), [0; 3], 0),
            "see-through shows black: {:?}",
            shown.at(12, 8)
        );
        // A stop brighter, worked on light as a clip's own colour is.
        let brighter = compose(
            &gpu,
            working,
            EIGHT,
            size,
            &[again(Colour::from_sliders(1.0, 0.0, 0.0, 0.0, 0.0))],
        );
        assert!(
            near(brighter.at(4, 8), [171; 3], 2),
            "{working:?}: {:?}",
            brighter.at(4, 8)
        );
    }
}
