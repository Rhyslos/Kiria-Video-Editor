//! The effects, each a plain struct that processes stereo blocks in place
//! and never allocates after it is made.
//!
//! - [`biquad`]: second-order filters (the cookbook shapes) and their response.
//! - [`filters`]: the track's high pass and six-band EQ.
//! - [`dynamics`]: the compressor and its plain presets.
//! - [`cleanup`]: noise clean-up by spectral gating, and learning a noise profile.
//! - [`duck`]: the ducking key (the dialogue's speech band) and each ducked track's ducker.
//! - [`level`]: the Loudness tool, which keeps a track at a loudness.
//! - [`lines`]: volume and pan lines on a fixed grid, and the fader.
//! - [`loudness`], [`limiter`]: loudness, true peak and the true-peak limiter.
//! - [`bands`]: the 64 bands by frequency shared by noise profiles and the analyser.

pub mod bands;
pub mod biquad;
pub mod cleanup;
pub mod duck;
pub mod dynamics;
pub mod filters;
pub mod level;
pub mod limiter;
pub mod lines;
pub mod loudness;

/// 20 / ln(10): decibels from a natural logarithm.
const DB_PER_NEPER: f32 = 8.685_889;

/// Decibels of a linear level. Silence is about -180 dB rather than
/// `-inf`, so arithmetic on it stays finite.
#[inline]
pub fn to_db(level: f32) -> f32 {
    (level.abs() + 1e-9).ln() * DB_PER_NEPER
}

/// The linear level of decibels.
#[inline]
pub fn from_db(db: f32) -> f32 {
    (db / DB_PER_NEPER).exp()
}

/// The factor a fader or the master's volume puts on samples: worked out
/// exactly as the timeline's track and master volumes always were
/// (`10^(dB/20)` in single precision), so a mix with every effect off is
/// the same to the bit as before the effects came. [`from_db`] is quicker
/// and is what the effects use, sample by sample.
pub fn volume_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The coefficient of a one-pole smoother that goes 63 % of the way in
/// `ms` milliseconds at `rate` steps a second: `next = want + c * (now - want)`.
pub fn smoothing(rate: f32, ms: f32) -> f32 {
    (-1.0 / (rate * ms.max(0.01) * 0.001)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibels_go_both_ways() {
        for db in [-60.0, -6.0, 0.0, 12.0] {
            assert!((to_db(from_db(db)) - db).abs() < 1e-3);
        }
        assert!((to_db(0.5) + 6.0206).abs() < 1e-3);
        assert!(to_db(0.0).is_finite());
    }

    #[test]
    fn a_smoother_goes_most_of_the_way_in_its_time() {
        let c = smoothing(1000.0, 10.0);
        let mut x = 0f32;
        for _ in 0..10 {
            x = 1.0 + c * (x - 1.0);
        }
        assert!((x - 0.632).abs() < 0.01, "{x}");
    }
}
