//! Ties the parts together: what is on the timeline at a moment, and the
//! workers that turn it into pictures (and later sound and exported files).
//!
//! - [`timeline`]: which clip is showing at a time, and where in its file.
//! - [`frame`]: every layer that is seen at a time, and where in the frame.
//! - [`compose`]: those layers' pictures put together on the window's device.
//! - [`export`]: the timeline written as a video file, on a worker.
//! - [`preview`]: a worker thread that decodes the picture at the playhead.
//! - [`mix`]: the timeline's sound at any moment, all audible tracks summed.
//! - [`playback`]: sound out of the speakers and the clock the picture follows.
//!
//! The interface only sends requests and reads results back, so slow media
//! can never freeze a window.

pub mod compose;
pub mod export;
pub mod frame;
pub mod frames;
pub mod loudness;
pub mod meter;
pub mod mix;
pub mod playback;
pub mod preview;
pub mod silence;
pub mod source;
pub mod thumbnail;
pub mod timeline;

pub use compose::{Composed, Composing};
pub use export::{Export, ExportSettings};
pub use playback::Playback;
pub use preview::{Convert, DeviceWatch, OnDevice, Picture, Preview};
