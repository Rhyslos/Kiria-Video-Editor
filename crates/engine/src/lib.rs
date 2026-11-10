//! Ties the parts together: what is on the timeline at a moment, and the
//! workers that turn it into pictures (and later sound and exported files).
//!
//! - [`timeline`]: which clip is showing at a time, and where in its file.
//! - [`preview`]: a worker thread that decodes the picture at the playhead.
//! - [`mix`]: the timeline's sound at any moment, all audible tracks summed.
//! - [`playback`]: sound out of the speakers and the clock the picture follows.
//!
//! The interface only sends requests and reads results back, so slow media
//! can never freeze a window.

pub mod mix;
pub mod playback;
pub mod preview;
pub mod timeline;

pub use playback::Playback;
pub use preview::{Picture, Preview};
