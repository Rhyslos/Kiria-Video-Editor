//! The real-time audio engine: effects on every track, the master, and the
//! mixer that runs them in the sound card's callback without allocating,
//! locking or waiting. Pure Rust: no sound card, no files, no FFmpeg, so
//! everything here runs the same live and in an export, and is tested
//! without a device.
//!
//! - [`dsp`]: the effects themselves.
//! - [`settings`]: settings as plain values, built from the project.
//! - [`strip`]: a track's effects in the Audio page's order.
//! - [`master`]: the mix's volume, compressor, limiter and meter.
//! - [`mixer`]: the real-time end; [`control`]: the interface's end
//!   (commands, swaps, the return of what was replaced).
//! - [`feed`]: each track's input ring, filled by a feeder thread.
//! - [`block`]: any device buffer size in, fixed blocks through the mixer.
//! - [`readout`], [`meter`]: what is measured, read through atomics.
//! - [`analyser`]: the EQ's spectrum, worked out on a worker.
//!
//! The design and how it was chosen: `docs/research/audio-engine.md`.

pub mod analyser;
pub mod block;
pub mod control;
pub mod dsp;
pub mod feed;
pub mod master;
pub mod meter;
pub mod mixer;
pub mod readout;
pub mod settings;
pub mod strip;
pub mod test_signal;

pub use analyser::{Analyser, Spectrum};
pub use block::BlockAdapter;
pub use control::{Command, Controller};
pub use feed::{FeedReader, FeedWriter};
pub use mixer::{Mixer, mixer};
pub use readout::{MasterReadout, TrackLevels, TrackReadout};
pub use settings::{MasterChange, MasterSettings, TrackChange, TrackSettings};

/// Frames in every block the mixer makes: 5.3 ms at 48 kHz. A setting
/// changed is heard within one block; the clean-up makes one window per
/// block, so every block costs the same.
pub const BLOCK: usize = 256;
