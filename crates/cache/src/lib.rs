//! Every cache, each with a budget.
//!
//! - [`waveform`]: the peak level of a sound stream, many times a second, for
//!   drawing waveforms on the timeline.
//!
//! - [`thumbs`]: small pictures along a video file, for drawing in clips.
//!
//! Still to come: the RAM frame cache, GPU textures, the disk render cache
//! and proxies.

pub mod thumbs;
pub mod waveform;

pub use waveform::Peaks;
