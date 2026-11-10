//! `cargo xtask icons <out-dir>`: draws the app icon.
//!
//! The mark is the accent-coloured rounded square from the interface with a
//! dark play triangle. Writes `icon.png` (256 px) and `icon.ico` (16 to 256 px).

use image::{ImageFormat, Rgba, RgbaImage, imageops::FilterType};
use std::path::Path;

const ACCENT: Rgba<u8> = Rgba([0xED, 0xE6, 0xD6, 0xFF]);
const INK: Rgba<u8> = Rgba([0x1A, 0x1B, 0x1C, 0xFF]);

pub fn write(args: &[String]) -> Result<(), String> {
    let [out] = args else {
        return Err("Usage: cargo xtask icons <out-dir>".into());
    };
    let out = Path::new(out);
    std::fs::create_dir_all(out).map_err(|e| format!("Can't create {}: {e}", out.display()))?;
    let big = draw(1024);
    let png = image::imageops::resize(&big, 256, 256, FilterType::Lanczos3);
    png.save_with_format(out.join("icon.png"), ImageFormat::Png)
        .map_err(|e| format!("Can't write icon.png: {e}"))?;

    let ico = image::codecs::ico::IcoEncoder::new(
        std::fs::File::create(out.join("icon.ico")).map_err(|e| format!("Can't write icon.ico: {e}"))?,
    );
    let frames = [16u32, 24, 32, 48, 64, 128, 256]
        .iter()
        .map(|&size| {
            let img = image::imageops::resize(&big, size, size, FilterType::Lanczos3);
            image::codecs::ico::IcoFrame::as_png(img.as_raw(), size, size, image::ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Can't encode the icon: {e}"))?;
    ico.encode_images(&frames)
        .map_err(|e| format!("Can't write icon.ico: {e}"))?;
    println!("Wrote icon.png and icon.ico to {}.", out.display());
    Ok(())
}

/// Draws the icon at `size`, smoothing edges by sampling 4×4 points per pixel.
fn draw(size: u32) -> RgbaImage {
    let s = size as f32;
    let (margin, radius) = (s * 0.06, s * 0.22);
    // Play triangle, pointing right, optically centred.
    let (tx0, tx1, ty0, ty1) = (s * 0.40, s * 0.70, s * 0.30, s * 0.70);
    RgbaImage::from_fn(size, size, |x, y| {
        let mut square: f32 = 0.0;
        let mut triangle: f32 = 0.0;
        for sy in 0..4 {
            for sx in 0..4 {
                let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                if in_rounded_square(px, py, margin, s - margin, radius) {
                    square += 1.0 / 16.0;
                    let mid = (ty0 + ty1) / 2.0;
                    let half_height = (ty1 - ty0) / 2.0 * (1.0 - (px - tx0) / (tx1 - tx0));
                    if px >= tx0 && px <= tx1 && (py - mid).abs() <= half_height {
                        triangle += 1.0 / 16.0;
                    }
                }
            }
        }
        let ink = triangle / square.max(1e-6);
        let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - ink) + f32::from(b) * ink) as u8;
        Rgba([
            mix(ACCENT[0], INK[0]),
            mix(ACCENT[1], INK[1]),
            mix(ACCENT[2], INK[2]),
            (square * 255.0) as u8,
        ])
    })
}

fn in_rounded_square(x: f32, y: f32, lo: f32, hi: f32, r: f32) -> bool {
    if x < lo || x > hi || y < lo || y > hi {
        return false;
    }
    let cx = x.clamp(lo + r, hi - r);
    let cy = y.clamp(lo + r, hi - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}
