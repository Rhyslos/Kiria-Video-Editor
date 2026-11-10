//! Ties the parts together: what is on the timeline at a moment, and the
//! workers that turn it into pictures (and later sound and exported files).
//!
//! - [`timeline`]: which clip is showing at a time, and where in its file.
//! - [`preview`]: a worker thread that decodes the picture at the playhead.
//!
//! The interface only sends requests and reads results back, so slow media
//! can never freeze a window.

pub mod preview;
pub mod timeline;

pub use preview::{Picture, Preview};
