//! `cargo xtask icons`: draws the app icon from `packaging/icons/icon.svg`.
//!
//! Every size is drawn straight from the drawing (never scaled down from a big
//! picture), so small sizes stay sharp. Writes:
//! - `packaging/icons/icon.png` (256 px: window, launcher and Linux icon),
//! - `packaging/icons/icon.ico` (16 to 256 px: Windows shortcuts and installer),
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

    let file =
        std::fs::File::create(out.join("icon.ico")).map_err(|e| format!("Can't write icon.ico: {e}"))?;
    let frames = ICO_SIZES
        .iter()
        .map(|&size| {
            let img = draw(&tree, size)?;
            image::codecs::ico::IcoFrame::as_png(img.as_raw(), size, size, image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("Can't encode the {size} px icon: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    image::codecs::ico::IcoEncoder::new(file)
        .encode_images(&frames)
        .map_err(|e| format!("Can't write icon.ico: {e}"))?;
    println!("Wrote {PACKAGING}/icon.png, {PACKAGING}/icon.ico and {INTERFACE}/icon-64.png.");
    Ok(())
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
}
