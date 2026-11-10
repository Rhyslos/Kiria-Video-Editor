//! The bundled fonts: a sans-serif for the interface and a monospace font
//! for every timecode and number. Both are under the SIL Open Font License;
//! the licence texts sit next to the font files in `assets/fonts/`.

use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

const SANS: &[u8] = include_bytes!("../assets/fonts/Inter_400Regular.ttf");
const SANS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter_600SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono_400Regular.ttf");

/// The font family for headings, names and the active tab.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// A timecode or number readout of the given size.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Installs the fonts. The built-in fonts stay behind them as a fallback for
/// characters ours don't cover.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("sans".into(), Arc::new(FontData::from_static(SANS)));
    fonts.font_data.insert(
        "sans-semibold".into(),
        Arc::new(FontData::from_static(SANS_SEMIBOLD)),
    );
    fonts
        .font_data
        .insert("mono".into(), Arc::new(FontData::from_static(MONO)));

    let fallbacks = |family: &FontFamily| fonts.families.get(family).cloned().unwrap_or_default();
    let proportional = fallbacks(&FontFamily::Proportional);
    let monospace = fallbacks(&FontFamily::Monospace);

    let with = |first: &str, rest: &[String]| {
        std::iter::once(first.to_owned())
            .chain(rest.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with("sans", &proportional));
    fonts
        .families
        .insert(semibold(), with("sans-semibold", &proportional));
    fonts
        .families
        .insert(FontFamily::Monospace, with("mono", &monospace));
    ctx.set_fonts(fonts);
}
