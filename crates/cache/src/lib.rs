//! Every cache, each with a budget.
//!
//! - [`waveform`]: the peak level of a sound stream, many times a second, for
//!   drawing waveforms on the timeline.
//!
//! Still to come: the RAM frame cache, GPU textures, the disk render cache,
//! proxies and thumbnails.

pub mod waveform;

pub use waveform::Peaks;
