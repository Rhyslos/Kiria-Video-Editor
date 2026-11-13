//! The master's true-peak limiter: one copy, in `audio::dsp::limiter`
//! (where the real-time mixer's master uses it), re-exported here for
//! export and anything that limits the timeline's sound.

pub use audio::dsp::limiter::*;
