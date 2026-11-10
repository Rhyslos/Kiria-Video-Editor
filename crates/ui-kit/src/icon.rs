//! The app icon: the mark in the top bar and the window icon.
//!
//! Both come from `assets/icon/icon-64.png`, drawn by `cargo xtask icons`
//! from `packaging/icons/icon.svg`. Decoding it takes well under a
//! millisecond, and the top bar's copy is decoded once and kept.

use egui::{Context, Id, TextureHandle, TextureOptions};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon/icon-64.png");

fn decode() -> Option<([usize; 2], Vec<u8>)> {
    let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (w, h) = image.dimensions();
    Some(([w as usize, h as usize], image.into_raw()))
}

/// The icon for a window's title bar and the taskbar. `None` only if the
/// built-in picture can't be read, in which case the system's default shows.
pub fn window_icon() -> Option<egui::IconData> {
    let ([w, h], rgba) = decode()?;
    Some(egui::IconData {
        rgba,
        width: w as u32,
        height: h as u32,
    })
}

/// Adds the app icon to a window being opened (no change if it can't be read).
pub fn with_window_icon(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    match window_icon() {
        Some(icon) => viewport.with_icon(std::sync::Arc::new(icon)),
        None => viewport,
    }
}

/// The icon as a texture, made the first time it is asked for.
pub fn texture(ctx: &Context) -> Option<TextureHandle> {
    let id = Id::unique("app-icon-texture");
    if let Some(texture) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(texture);
    }
    let (size, rgba) = decode()?;
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    let texture = ctx.load_texture("app-icon", image, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_icon_reads() {
        let icon = window_icon().expect("the icon decodes");
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
    }

    #[test]
    fn the_texture_is_made_once() {
        let ctx = Context::default();
        let first = texture(&ctx).unwrap();
        let second = texture(&ctx).unwrap();
        assert_eq!(first.id(), second.id());
    }
}
