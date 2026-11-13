//! Ties the parts together: what is on the timeline at a moment, and the
//! workers that turn it into pictures (and later sound and exported files).
//!
//! - [`timeline`]: which clip is showing at a time, and where in its file.
//! - [`frame`]: every layer that is seen at a time, and where in the frame.
//! - [`compose`]: those layers' pictures put together on the window's device.
//! - [`export`]: the timeline written as a video or sound file, on a worker.
//! - [`preview`]: a worker thread that decodes the picture at the playhead.
//! - [`mix`]: each track's clips' sound at any moment (decoding, clip volume,
//!   fades, speed, ramps, nested timelines).
//! - [`feeder`]: that sound, per track, into the real-time mixer's rings
//!   (`crates/audio`: track effects, volume, mute, solo, the master).
//! - [`playback`]: sound out of the speakers and the clock the picture
//!   follows; [`output`]: the sound card's callback.
//! - [`offline`]: the same mix without a sound card, for export.
//! - [`loudness`], [`limiter`], [`meter`]: `crates/audio`'s meter and
//!   limiter, re-exported.
//! - [`sync`]: where a separately recorded sound lines up with another, by
//!   what they hear.
//!
//! The interface only sends requests and reads results back, so slow media
//! can never freeze a window.

pub mod compose;
pub mod export;
pub mod feeder;
pub mod frame;
pub mod frames;
pub mod limiter;
pub mod loudness;
pub mod meter;
pub mod mix;
pub mod offline;
pub mod output;
pub mod playback;
pub mod preview;
pub mod silence;
pub mod source;
pub mod sync;
pub mod thumbnail;
pub mod timeline;

pub use compose::{Composed, Composing};
pub use export::{Export, ExportSettings};
pub use playback::Playback;
/// Counts allocations on threads a test marks: the sound card's callback
/// is checked never to allocate (`playback`'s tests).
#[cfg(test)]
#[global_allocator]
static COUNTING: os::alloc_count::Counting = os::alloc_count::Counting;

pub use preview::{Convert, DeviceWatch, OnDevice, Picture, Preview};
