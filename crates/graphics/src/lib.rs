//! Which graphics system draws the windows, for the launcher and the editor alike.
//!
//! On Windows a window prefers Vulkan, so that video can be decoded straight
//! into the window's own device (`docs/research/decoding.md`), and falls back
//! to Direct3D 12. Both programs go through the same four steps:
//!
//! 1. [`prepare`] reads the shared Graphics setting ([`GraphicsSettings`]),
//!    looks for the program's start marker and decides what to try ([`plan`]).
//! 2. [`open_window`] tries each graphics system on its own, in that order.
//!    One that reports an error is followed by the next in the same start.
//! 3. A driver can also fail without reporting anything: a crash or a freeze
//!    while the window opens. [`StartMarker`] catches that. It is written
//!    before a start with Vulkan and removed once frames have been drawn, so a
//!    marker found at the next start means Vulkan never got that far. That
//!    start uses Direct3D 12, which never loads the Vulkan driver.
//! 4. [`WindowGraphics`] is the window's side: it removes the marker after the
//!    first frames, asks whether to keep Direct3D 12 or try Vulkan again
//!    after a failed start, draws the Graphics row in Settings, and tells
//!    Diagnostics, the log and crash reports what is in use.

mod marker;
mod plan;
mod settings;
mod start;
mod window;

pub use marker::{MarkerState, Program, StartMarker};
pub use plan::{Backend, Plan, Platform, RunKind, plan};
pub use settings::{GraphicsChoice, GraphicsSettings};
pub use start::{FailedTry, Opened, Startup, open_window, prepare};
pub use window::{WarningAnswer, WindowGraphics};

/// Frames drawn with Vulkan before the start counts as working.
pub const FRAMES_BEFORE_TRUSTED: u64 = 3;
