//! Shared interface parts.
//!
//! Everything that must look and behave the same on every page lives here:
//! theme colours, fonts, shared sizes, the top bar, dialogs, the shortcut
//! table, the viewer, the transport, the timeline and the way errors are shown. Pages use these instead of drawing their own.

pub mod dialog;
pub mod drawer;
pub mod errors;
pub mod fonts;
pub mod icon;
pub mod layout;
pub mod shortcuts;
pub mod theme;
pub mod time_text;
pub mod timeline;
pub mod top_bar;
pub mod transport;
pub mod viewer;

pub use dialog::dialog;
pub use errors::{ErrorCenter, ErrorResponse, ShownError, accent_button, status_dot};
pub use layout::Page;
pub use shortcuts::{Action, Shortcut, ShortcutTable};
pub use theme::{Theme, ThemeName, status};
pub use timeline::{
    ClipEdge, ClipWaveform, TimelineClip, TimelineData, TimelineEvent, TimelineMarker, TimelineTrack,
    TimelineView, Tool,
};
pub use top_bar::{TitleBar, TopBar, TopBarClicks, logo_mark};
pub use transport::TransportClick;
pub use viewer::PreviewQuality;

/// Sets fonts and the theme. Call once at start-up and again when the theme changes.
pub fn setup(ctx: &egui::Context, theme: ThemeName) {
    fonts::install(ctx);
    theme.theme().apply(ctx);
}
