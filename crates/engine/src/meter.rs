//! The master output's meter while playing: the real-time mixer's
//! [`audio::MasterReadout`] (peaks and true peaks per channel, the highest
//! true peak, momentary, short-term and integrated loudness, and how far
//! the master's compressor and limiter turn the mix down), read by the
//! interface through atomics without locking.

pub use audio::MasterReadout;

/// One look at the meter. Silence reads as `-inf`.
pub type Levels = crate::loudness::Measured;
