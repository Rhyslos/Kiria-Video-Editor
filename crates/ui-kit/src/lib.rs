//! Shared interface parts.
//!
//! Everything that must look and behave the same on every page lives here:
//! theme colours, fonts, shared sizes, the top bar, dialogs, the shortcut
//! table, the viewer, the transport, the timeline and the way errors are shown. Pages use these instead of drawing their own.

pub mod dialog;
pub mod drawer;
pub mod errors;
pub mod file_dialog;
pub mod fonts;
pub mod icon;
pub mod inspector;
pub mod keyboard;
pub mod keyframe;
pub mod layout;
pub mod master;
pub mod mixer;
pub mod picture_handles;
pub mod scrub;
pub mod settings;
pub mod shortcuts;
pub mod splitter;
pub mod tab_strip;
pub mod theme;
pub mod time_text;
pub mod timeline;
pub mod timeline_tabs;
pub mod top_bar;
pub mod transport;
pub mod user_guide;
pub mod video_picture;
pub mod viewer;
pub mod waveform;
pub mod yuv_renderer;

pub use dialog::dialog;
pub use errors::{ErrorCenter, ErrorResponse, ShownError, accent_button, status_dot};
pub use layout::Page;
pub use shortcuts::{Action, Context, Shortcut, ShortcutTable};
pub use theme::{Theme, ThemeName, status};
pub use timeline::{
    ClickMoves, ClipEdge, ClipPictures, ClipShows, ClipSound, ClipWaveform, DraggedMedia, DraggedTimeline,
    KeyframeMark, LineKind, LineOwner, LinesShown, RegionCut, SlipRoom, SourceMap, SpeedCurve, TimelineClip,
    TimelineData, TimelineEvent, TimelineMarker, TimelineSpan, TimelineTrack, TimelineView, Tool, ViewStep,
};
pub use top_bar::{TitleBar, TopBar, TopBarClicks, logo_mark, with_keys};
pub use transport::TransportClick;
pub use video_picture::{ChromaLayout, ComposedPicture, PlaneTextures, Slot, VideoPicture, YuvImage};
pub use viewer::PreviewQuality;

/// Sets fonts and the theme. Call once at start-up and again when the theme changes.
pub fn setup(ctx: &egui::Context, theme: ThemeName) {
    fonts::install(ctx);
    theme.theme().apply(ctx);
    // Ctrl with =, - and 0 belong to the shortcut table (they zoom the
    // viewer's picture), not to the toolkit's zoom of the whole interface.
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_keys_that_zoom_the_interface_are_left_to_the_shortcut_table() {
        let ctx = egui::Context::default();
        assert!(ctx.options(|o| o.zoom_with_keyboard), "on until set up");
        super::setup(&ctx, super::ThemeName::default());
        assert!(!ctx.options(|o| o.zoom_with_keyboard));
    }
}
