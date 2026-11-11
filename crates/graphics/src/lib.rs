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
//!    With Vulkan, the device is first opened here instead of by the window
//!    toolkit, with its video decode queues switched on (`vulkan_device`), so
//!    that video can be decoded on it; when that can't be done, the toolkit
//!    opens its own device as before and [`VideoDecoding`] says why.
//! 3. A driver can also fail without reporting anything: a crash or a freeze
//!    while the window opens. [`StartMarker`] catches that. It is written
//!    before a start with Vulkan and removed once frames have been drawn, so a
//!    marker found at the next start means Vulkan never got that far. That
//!    start uses Direct3D 12, which never loads the Vulkan driver.
//! 4. [`WindowGraphics`] is the window's side: it removes the marker after the
//!    first frames, asks whether to keep Direct3D 12 or try Vulkan again
//!    after a failed start, draws the Graphics row in Settings, and tells
//!    Diagnostics, the log and crash reports what is in use and which kinds
//!    of video the window's device decodes.
//! 5. Decoding video on the window's device has the same safety net, apart
//!    from the window's: a switch in the same settings file, and a second
//!    marker held by [`DecodeGuard`] from the first decode of a session until
//!    pictures decoded there have been drawn. A marker found at the next start
//!    switches that way of decoding off before the window opens, and
//!    [`WindowGraphics`] asks whether to keep it off or to try it again.
//!    [`plane_textures`] hands a picture decoded there to the window's renderer.

mod decode_guard;
mod marker;
mod plan;
mod settings;
mod start;
mod video;
mod vulkan_device;
mod window;

pub use decode_guard::{DecodeGuard, PICTURES_BEFORE_TRUSTED, STUCK_AFTER};
pub use marker::{MarkerState, Program, StartMarker};
pub use plan::{Backend, DecodingFound, Plan, Platform, RunKind, plan};
pub use settings::{GraphicsChoice, GraphicsSettings};
pub use start::{FailedTry, Opened, Startup, open_window, prepare};
pub use video::VideoDecoding;
pub use vulkan_device::plane_textures;
pub use window::{DECODING_SWITCH, DecodingAnswer, DeviceDecoding, WarningAnswer, WindowGraphics};

/// Frames drawn with Vulkan before the start counts as working.
pub const FRAMES_BEFORE_TRUSTED: u64 = 3;
