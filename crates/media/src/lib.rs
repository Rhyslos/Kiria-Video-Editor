//! Reading and writing media files.
//!
//! A small, safe layer over FFmpeg. Everything outside this crate sees plain
//! Rust types and explained errors; the FFmpeg calls stay in here.
//!
//! - [`probe`]: what a file holds (container, streams, codec, frame rate,
//!   length, rotation), without decoding it.
//! - [`timing`]: constant or variable frame rate, and the nearest standard rate.
//! - [`audio`]: sound as stereo float samples at any rate, from any point.
//! - [`decode`]: the picture at any moment of a video, scaled and upright.
//! - [`encode`]: writing a file: MP4 or MKV video (H.264, H.265, AV1 with
//!   AAC, Opus or FLAC sound), or sound alone as WAV, MP3 or OGG.
//! - [`proxy`]: writing a proxy, a small copy of a recording with key frames
//!   close together and exactly the original's frame times.
//! - [`yuv`]: that picture as the decoder's own planes, for the GPU to convert.
//! - [`vulkan`]: decoding on the window's own graphics device, where the
//!   picture never leaves the graphics card.
//! - [`fingerprint`]: recognises a file's contents quickly, so a moved file can
//!   be found again.
//! - [`describe`]: both of the above as a [`project::MediaRef`] ready to import.
//! - [`check`]: [`describe`] plus the checks a file passes before import
//!   (codecs this build decodes, first picture and sound readable, nothing
//!   missing at the end), each refusal explained.
//!
//! Probing opens the file and reads its headers, which can take tens of
//! milliseconds on a slow disk: call it from a worker, never the interface thread.

// This crate is the FFmpeg wrapper the working rules allow `unsafe` in.
#![allow(unsafe_code)]

pub mod audio;
pub mod check;
pub mod decode;
pub mod encode;
mod ffi;
pub mod fingerprint;
pub mod jump;
pub mod probe;
pub mod proxy;
pub mod relink;
pub mod timing;
pub mod vulkan;
pub mod yuv;

pub use audio::{AudioDecoder, AudioStreams};
pub use check::{Refusal, Refused, check, end_missing_words, salvage};
pub use decode::{DecodeCost, Decoded, Decoding, Frame, Level, Newer, Reached, VideoDecoder, Work};
pub use fingerprint::fingerprint;
pub use probe::{Probe, probe};
pub use vulkan::{GpuFrame, WindowDevice};
pub use yuv::{Colour, Layout, Matrix, Planes, Range, YuvFrame};

use project::{MediaId, MediaRef};
use std::path::Path;

/// Everything the project keeps about a file, read from disk. The id is a
/// placeholder; the project gives a real one when the file is imported.
pub fn describe(path: &Path) -> diag::Result<MediaRef> {
    let probe = probe(path)?;
    Ok(MediaRef {
        id: MediaId(0),
        path: path.to_path_buf(),
        fingerprint: fingerprint(path)?,
        kind: probe.kind,
        duration: probe.duration,
        info: probe.info,
    })
}
