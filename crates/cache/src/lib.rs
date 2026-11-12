//! Every cache, each with a budget.
//!
//! - [`waveform`]: the peak level of a sound stream, many times a second, for
//!   drawing waveforms on the timeline.
//!
//! - [`thumbs`]: small pictures along a video file, for drawing in clips.
//!
//! - [`proxy`]: proxies, small copies of recordings that are cheap to jump
//!   around in, kept in the app's cache folder by the recording's
//!   fingerprint, within the disk budget.
//!
//! Still to come: the RAM frame cache and the disk render cache. (Frames
//! kept on the graphics card are `engine::frames`.)

pub mod proxy;
pub mod thumbs;
pub mod waveform;

pub use waveform::Peaks;
