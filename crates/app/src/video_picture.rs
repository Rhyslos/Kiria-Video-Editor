//! Decoded YUV planes on their way to the GPU: whether this window can
//! convert colour while drawing, and a decoded frame as the picture the
//! shared viewer draws.

use engine::Convert;
use ui_kit::{ChromaLayout, YuvImage};

/// Where the viewer's colour gets converted in this window: by the GPU
/// while drawing when the window is drawn with wgpu, else (or when
/// `on_cpu` asks for it) on the CPU.
pub fn conversion(render_state: Option<&eframe::egui_wgpu::RenderState>, on_cpu: bool) -> Convert {
    if on_cpu {
        tracing::info!("the viewer's colour is converted on the CPU, as asked");
        return Convert::OnCpu;
    }
    match ui_kit::video_picture::install(render_state) {
        Some(largest_side) => Convert::WhileDrawing { largest_side },
        None => {
            tracing::info!("no GPU drawing in this window: the viewer's colour is converted on the CPU");
            Convert::OnCpu
        }
    }
}

/// A decoded frame's planes as the viewer's picture. Nothing is copied: the
/// planes' memory moves along and goes back to its decoder once uploaded.
pub fn image(frame: media::YuvFrame) -> YuvImage {
    YuvImage {
        luma_size: (frame.width, frame.height),
        layout: match frame.layout {
            media::Layout::Interleaved => ChromaLayout::Interleaved,
            media::Layout::Planar => ChromaLayout::Planar,
        },
        to_rgb: frame.colour.rgb_rows(),
        rotation: frame.rotation,
        planes: Box::new(frame.into_planes()),
    }
}

#[cfg(test)]
mod tests {
    //! The shader against known colours and against the CPU's conversion of
    //! real frames, drawn into a texture on whatever graphics device this
    //! machine has (a software one will do). Without any device there is
    //! nothing to draw with: the checks say so and pass.

    use super::*;
    use eframe::wgpu;
    use media::{Colour, Decoded, Decoding, Matrix, Range, VideoDecoder};
    use std::path::{Path, PathBuf};
    use ui_kit::yuv_renderer::YuvRenderer;

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
        let Ok(adapter) = adapter else {
            eprintln!("no graphics device here: the shader isn't checked");
            return None;
        };
        // The least any device offers, with textures as large as this one takes.
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let descriptor = wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        };
        match pollster::block_on(adapter.request_device(&descriptor)) {
            Ok(pair) => Some(pair),
            Err(err) => {
                eprintln!("the graphics device didn't open ({err}): the shader isn't checked");
                None
            }
        }
    }

    /// Draws `image` into a `width × height` target of `format` and reads
    /// the target back as RGBA bytes.
    fn draw(
        (device, queue): &(wgpu::Device, wgpu::Queue),
        image: &YuvImage,
        format: wgpu::TextureFormat,
        (width, height): (u32, u32),
    ) -> Vec<u8> {
        let mut renderer = YuvRenderer::new(device, format);
        assert!(renderer.upload(device, queue, image), "{image:?} uploads");
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        // Rows in a buffer start at multiples of 256 bytes.
        let row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.draw(&mut pass);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped
            .chunks(row as usize)
            .flat_map(|r| r[..width as usize * 4].to_vec())
            .collect()
    }

    /// A picture of one colour, `width × height`, in either layout.
    fn flat(yuv: [u8; 3], colour: Colour, layout: ChromaLayout, (width, height): (u32, u32)) -> YuvImage {
        let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        let mut planes = vec![yuv[0]; (width * height) as usize];
        match layout {
            ChromaLayout::Interleaved => planes.extend([yuv[1], yuv[2]].repeat(chroma)),
            ChromaLayout::Planar => {
                planes.extend(vec![yuv[1]; chroma]);
                planes.extend(vec![yuv[2]; chroma]);
            }
        }
        YuvImage {
            luma_size: (width, height),
            layout,
            planes: Box::new(planes),
            to_rgb: colour.rgb_rows(),
            rotation: 0,
        }
    }

    #[test]
    fn known_colours_come_out_right_on_both_kinds_of_target() {
        let Some(gpu) = device() else { return };
        let colour = |matrix, range| Colour { matrix, range };
        // Stored values and the colour they mean.
        let cases = [
            (colour(Matrix::Bt709, Range::Limited), [16, 128, 128], [0, 0, 0]),
            (
                colour(Matrix::Bt709, Range::Limited),
                [235, 128, 128],
                [255, 255, 255],
            ),
            (
                colour(Matrix::Bt709, Range::Limited),
                [126, 128, 128],
                [128, 128, 128],
            ),
            (colour(Matrix::Bt709, Range::Limited), [63, 102, 240], [255, 0, 0]),
            (colour(Matrix::Bt709, Range::Limited), [173, 42, 26], [0, 255, 0]),
            (colour(Matrix::Bt709, Range::Limited), [32, 240, 118], [0, 0, 255]),
            (colour(Matrix::Bt601, Range::Limited), [81, 90, 240], [255, 0, 0]),
            (colour(Matrix::Bt601, Range::Limited), [145, 54, 34], [0, 255, 0]),
            (colour(Matrix::Bt601, Range::Limited), [41, 240, 110], [0, 0, 255]),
            (colour(Matrix::Bt709, Range::Full), [16, 128, 128], [16, 16, 16]),
            (
                colour(Matrix::Bt709, Range::Full),
                [255, 128, 128],
                [255, 255, 255],
            ),
            (colour(Matrix::Bt709, Range::Full), [182, 30, 12], [0, 255, 0]),
            (colour(Matrix::Bt601, Range::Full), [29, 255, 107], [0, 0, 255]),
            // Read with the wrong matrix, red would turn orange.
            (colour(Matrix::Bt709, Range::Limited), [81, 90, 240], [255, 24, 0]),
        ];
        // The window's target is one or the other, by system: one stores the
        // values as given, the other applies the screen's curve itself.
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            for layout in [ChromaLayout::Interleaved, ChromaLayout::Planar] {
                for (colour, yuv, want) in cases {
                    // An odd size: the colour planes round up.
                    let image = flat(yuv, colour, layout, (5, 3));
                    let rgba = draw(&gpu, &image, format, (5, 3));
                    for pixel in rgba.chunks_exact(4) {
                        let near = pixel[..3].iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 2);
                        assert!(
                            near && pixel[3] == 255,
                            "{yuv:?} {colour:?} {layout:?} on {format:?}: got {pixel:?}, wanted {want:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_picture_is_turned_upright_while_drawing() {
        let Some(gpu) = device() else { return };
        // Two grey pixels side by side: dark on the left, light on the right.
        let image = |rotation| YuvImage {
            luma_size: (2, 1),
            layout: ChromaLayout::Planar,
            planes: Box::new(vec![50u8, 200, 128, 128]),
            to_rgb: Colour {
                matrix: Matrix::Bt709,
                range: Range::Full,
            }
            .rgb_rows(),
            rotation,
        };
        let greys = |rotation, size| -> Vec<u8> {
            draw(&gpu, &image(rotation), wgpu::TextureFormat::Rgba8Unorm, size)
                .chunks_exact(4)
                .map(|p| p[0])
                .collect()
        };
        assert_eq!(greys(0, (2, 1)), [50, 200]);
        assert_eq!(
            greys(90, (1, 2)),
            [50, 200],
            "a clockwise quarter turn: the left ends on top"
        );
        assert_eq!(greys(180, (2, 1)), [200, 50]);
        assert_eq!(
            greys(270, (1, 2)),
            [200, 50],
            "the other way: the right ends on top"
        );
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    /// The mean and the largest difference between two RGBA pictures' colours.
    fn difference(a: &[u8], b: &[u8]) -> (f64, u8) {
        assert_eq!(a.len(), b.len());
        let (mut total, mut worst) = (0u64, 0u8);
        for (x, y) in a.iter().zip(b) {
            total += u64::from(x.abs_diff(*y));
            worst = worst.max(x.abs_diff(*y));
        }
        (total as f64 / a.len() as f64, worst)
    }

    /// What the shader should give for planes drawn at their own size, not
    /// turned: every pixel's luma, and the colour planes read smoothly
    /// between samples as the GPU reads a half-size texture.
    fn expected(planes: &media::YuvFrame) -> Vec<u8> {
        let (w, h) = (planes.width as usize, planes.height as usize);
        let (cw, ch) = planes.chroma_size();
        let (cw, ch) = (cw as usize, ch as usize);
        let sample = |channel: usize, x: usize, y: usize| -> f32 {
            let at = y * cw + x;
            f32::from(match planes.layout {
                media::Layout::Interleaved => planes.chroma()[at * 2 + channel],
                media::Layout::Planar => planes.chroma()[channel * cw * ch + at],
            })
        };
        // A position in a plane and its neighbour, with the share of each.
        let between = |centre: f32, size: usize| {
            let low = (centre - 0.5).floor();
            let clamp = |i: f32| (i.max(0.0) as usize).min(size - 1);
            (clamp(low), clamp(low + 1.0), centre - 0.5 - low)
        };
        let rows = planes.colour.rgb_rows();
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let (y0, y1, fy) = between((y as f32 + 0.5) * ch as f32 / h as f32, ch);
            for x in 0..w {
                let (x0, x1, fx) = between((x as f32 + 0.5) * cw as f32 / w as f32, cw);
                let smooth = |channel| {
                    let top = sample(channel, x0, y0) * (1.0 - fx) + sample(channel, x1, y0) * fx;
                    let bottom = sample(channel, x0, y1) * (1.0 - fx) + sample(channel, x1, y1) * fx;
                    (top * (1.0 - fy) + bottom * fy) / 255.0
                };
                let yuv = [f32::from(planes.luma()[y * w + x]) / 255.0, smooth(0), smooth(1)];
                for row in rows {
                    let c = row[0] * yuv[0] + row[1] * yuv[1] + row[2] * yuv[2] + row[3];
                    rgba.push((c * 255.0).round().clamp(0.0, 255.0) as u8);
                }
                rgba.push(255);
            }
        }
        rgba
    }

    fn planes_of(name: &str, rotation: u16) -> (media::YuvFrame, media::Frame) {
        let mut decoder = VideoDecoder::open(&fixture(name), 0, rotation, Decoding::Software).unwrap();
        let rgba = decoder.frame_at(0.05, 4000, 4000).unwrap();
        let Decoded::Yuv(planes) = decoder.picture_at(0.05, 4000, 4000).unwrap() else {
            panic!("{name} comes as planes");
        };
        (planes, rgba)
    }

    #[test]
    fn real_frames_match_the_same_maths_on_the_cpu() {
        let Some(gpu) = device() else { return };
        for name in [
            "game-3-audio.mkv",
            "phone-rotated.mp4",
            "vfr-60.mkv",
            "colour-709-full.mp4",
        ] {
            // As stored, at their own size: every value can be worked out.
            let (planes, _) = planes_of(name, 0);
            let want = expected(&planes);
            let size = (planes.width, planes.height);
            let drawn = draw(&gpu, &image(planes), wgpu::TextureFormat::Rgba8Unorm, size);
            let (mean, worst) = difference(&drawn, &want);
            assert!(worst <= 3, "{name}: mean {mean:.3}, worst {worst}");
        }
    }

    #[test]
    fn real_frames_come_out_like_the_rgba_path_turned_and_all() {
        let Some(gpu) = device() else { return };
        for (name, rotation) in [
            ("game-3-audio.mkv", 0),
            ("phone-rotated.mp4", 270),
            ("vfr-60.mkv", 0),
            ("colour-709-full.mp4", 90),
        ] {
            let (planes, rgba) = planes_of(name, rotation);
            let size = planes.upright_size();
            assert_eq!(size, (rgba.width, rgba.height));
            let drawn = draw(&gpu, &image(planes), wgpu::TextureFormat::Rgba8Unorm, size);
            // The scaler takes each pixel's colour from the nearest sample and
            // the GPU reads smoothly between samples, so values differ where
            // colours change sharply (these test patterns are full of such
            // edges) and nowhere else. A wrong turn or matrix would differ
            // nearly everywhere.
            let close = drawn
                .iter()
                .zip(&rgba.rgba)
                .filter(|(a, b)| a.abs_diff(**b) <= 3)
                .count();
            let share = close as f64 / drawn.len() as f64;
            let (mean, _) = difference(&drawn, &rgba.rgba);
            assert!(
                share > 0.6 && mean < 8.0,
                "{name}: {share:.2} of values within 3, mean {mean:.2}"
            );
        }
    }

    #[test]
    fn a_picture_the_device_cannot_take_is_refused_not_drawn_wrong() {
        let Some((device, queue)) = device() else { return };
        let mut renderer = YuvRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let colour = Colour {
            matrix: Matrix::Bt709,
            range: Range::Limited,
        };
        let good = flat([128, 128, 128], colour, ChromaLayout::Planar, (4, 4));
        assert!(renderer.upload(&device, &queue, &good));
        assert!(renderer.has_picture());
        // Planes shorter than the size says.
        let short = YuvImage {
            planes: Box::new(vec![0u8; 5]),
            ..flat([0, 0, 0], colour, ChromaLayout::Planar, (4, 4))
        };
        assert!(!renderer.upload(&device, &queue, &short));
        assert!(
            !renderer.has_picture(),
            "the picture before it isn't left standing in"
        );
        // Wider than any texture.
        let side = device.limits().max_texture_dimension_2d + 2;
        let wide = flat([0, 0, 0], colour, ChromaLayout::Interleaved, (side, 2));
        assert!(!renderer.upload(&device, &queue, &wide));
        // And back to a good one, in the other layout.
        let other = flat([128, 128, 128], colour, ChromaLayout::Interleaved, (4, 4));
        assert!(renderer.upload(&device, &queue, &other));
    }
}
