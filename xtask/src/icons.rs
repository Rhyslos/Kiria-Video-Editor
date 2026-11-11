//! `cargo xtask icons`: draws the app icon from `packaging/icons/icon.svg`.
//!
//! Every size is drawn straight from the drawing (never scaled down from a big
//! picture), so small sizes stay sharp. Writes:
//! - `packaging/icons/icon.png` (256 px: window, launcher and Linux icon),
//! - `packaging/icons/icon.ico` (16 to 256 px: built into both Windows programs, and used by shortcuts and the installer),
//! - `crates/ui-kit/assets/icon/icon-64.png` (the mark in the interface's top bar).

use image::{ImageFormat, RgbaImage};
use std::path::Path;

/// The drawing every icon comes from, relative to the repository root.
const SOURCE: &str = "packaging/icons/icon.svg";

/// Sizes inside the Windows icon file.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Where the files go, relative to the repository root.
const PACKAGING: &str = "packaging/icons";
const INTERFACE: &str = "crates/ui-kit/assets/icon";

pub fn write(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("Usage: cargo xtask icons".into());
    }
    let out = Path::new(PACKAGING);
    let svg = std::fs::read(SOURCE).map_err(|e| format!("Can't read {SOURCE}: {e}"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .map_err(|e| format!("{SOURCE} is not a valid drawing: {e}"))?;

    for (path, size) in [
        (out.join("icon.png"), 256),
        (Path::new(INTERFACE).join("icon-64.png"), 64),
    ] {
        draw(&tree, size)?
            .save_with_format(&path, ImageFormat::Png)
            .map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    }

    let images = ICO_SIZES
        .iter()
        .map(|&size| draw(&tree, size))
        .collect::<Result<Vec<_>, _>>()?;
    std::fs::write(out.join("icon.ico"), ico(&images)?).map_err(|e| format!("Can't write icon.ico: {e}"))?;
    println!("Wrote {PACKAGING}/icon.png, {PACKAGING}/icon.ico and {INTERFACE}/icon-64.png.");
    Ok(())
}

/// A Windows icon file holding every image. Sizes up to 128 px are stored
/// as plain bitmaps and only the 256 px one as PNG: some parts of Windows
/// (shortcuts, file icons, the embedded program icon) show nothing for
/// small PNG images.
fn ico(images: &[RgbaImage]) -> Result<Vec<u8>, String> {
    let mut entries = Vec::new();
    let mut data: Vec<Vec<u8>> = Vec::new();
    for img in images {
        let (w, h) = img.dimensions();
        let bytes = if w >= 256 {
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
                .map_err(|e| format!("Can't encode the {w} px icon: {e}"))?;
            png
        } else {
            bitmap(img)
        };
        entries.push((w, h, bytes.len() as u32));
        data.push(bytes);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (w, h, len) in &entries {
        // 0 stands for 256 in the one-byte size fields.
        out.push(if *w >= 256 { 0 } else { *w as u8 });
        out.push(if *h >= 256 { 0 } else { *h as u8 });
        out.extend_from_slice(&[0, 0]); // no palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += len;
    }
    for d in data {
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// One icon image as a 32-bit bitmap: the header (height doubled, as icon
/// files expect), the pixels bottom row first in blue-green-red-alpha, then
/// an all-clear mask (alpha already says what shows).
fn bitmap(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // no compression; sizes and colours unused
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_row = (w.div_ceil(32) * 4) as usize;
    out.extend(std::iter::repeat_n(0u8, mask_row * h as usize));
    out
}

/// Draws the icon at `size` × `size` pixels.
fn draw(tree: &resvg::usvg::Tree, size: u32) -> Result<RgbaImage, String> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("The icon size must not be zero.")?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // The drawing library keeps colours premultiplied by transparency; image files don't.
    let pixels: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    RgbaImage::from_raw(size, size, pixels).ok_or_else(|| "The drawn icon has the wrong size.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_drawing_renders_at_every_size() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let svg = std::fs::read(root.join(SOURCE)).unwrap();
        let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).unwrap();
        for size in ICO_SIZES {
            let img = draw(&tree, size).unwrap();
            assert_eq!(img.dimensions(), (size, size));
            let centre = img.get_pixel(size / 2, size / 2);
            assert_eq!(centre[3], 255, "the middle of the icon is opaque");
            let corner = img.get_pixel(0, 0);
            assert_eq!(corner[3], 0, "the rounded corner is transparent");
        }
    }

    #[test]
    fn the_icon_file_has_bitmaps_and_one_png() {
        let small = RgbaImage::from_pixel(16, 16, image::Rgba([10, 20, 30, 255]));
        let big = RgbaImage::from_pixel(256, 256, image::Rgba([10, 20, 30, 255]));
        let file = ico(&[small, big]).unwrap();
        assert_eq!(&file[..6], &[0, 0, 1, 0, 2, 0]);
        // First entry: 16 px, one plane, 32 bits, a bitmap header at its offset.
        assert_eq!((file[6], file[10], file[12]), (16, 1, 32));
        let at = u32::from_le_bytes(file[18..22].try_into().unwrap()) as usize;
        assert_eq!(&file[at..at + 4], &40u32.to_le_bytes());
        // Second entry: 256 px (written as 0), stored as PNG.
        assert_eq!(file[22], 0);
        let at = u32::from_le_bytes(file[34..38].try_into().unwrap()) as usize;
        assert_eq!(&file[at + 1..at + 4], b"PNG");
        // The whole file reads back.
        let back = image::load_from_memory_with_format(&file, ImageFormat::Ico).unwrap();
        assert_eq!(back.width(), 256);
    }
}
