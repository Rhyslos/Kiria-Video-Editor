//! 64 bands by frequency, evenly spread on a musical scale from 20 Hz to
//! 20 kHz (about a sixth of an octave each). Noise profiles are kept in
//! them, so a profile is small and works at any rate, and the analyser
//! shows them.

/// How many bands.
pub const COUNT: usize = 64;
/// The lowest band's lower edge, Hz.
pub const LOWEST: f32 = 20.0;
/// The highest band's upper edge, Hz.
pub const HIGHEST: f32 = 20_000.0;

/// Where `freq` falls on the band scale: band `i` covers `i..i + 1`.
/// Below the lowest band it is negative, above the highest at least `COUNT`.
pub fn position(freq: f32) -> f32 {
    COUNT as f32 * (freq.max(1e-3) / LOWEST).ln() / (HIGHEST / LOWEST).ln()
}

/// The band `freq` falls in, kept to the bands there are.
pub fn band_of(freq: f32) -> usize {
    (position(freq).floor().max(0.0) as usize).min(COUNT - 1)
}

/// Band `i`'s centre (on the musical scale), Hz.
pub fn centre(i: usize) -> f32 {
    LOWEST * (HIGHEST / LOWEST).powf((i as f32 + 0.5) / COUNT as f32)
}

/// The value at `freq` from per-band values, in a straight line between
/// band centres on the musical scale (flat beyond the outer centres).
pub fn between(values: &[f32; COUNT], freq: f32) -> f32 {
    let at = (position(freq) - 0.5).clamp(0.0, (COUNT - 1) as f32);
    let i = (at.floor() as usize).min(COUNT - 2);
    let share = at - i as f32;
    values[i] + (values[i + 1] - values[i]) * share
}

/// Per-band means of per-bin values (bin `k` at `k * bin_hz`): the mean of
/// the bins inside each band, or the bin nearest its centre when the band
/// is narrower than a bin. The bin at 0 Hz is left out.
pub fn from_bins(bins: &[f32], bin_hz: f32, out: &mut [f32; COUNT]) {
    let mut sums = [0f32; COUNT];
    let mut counts = [0u32; COUNT];
    for (k, v) in bins.iter().enumerate().skip(1) {
        let p = position(k as f32 * bin_hz);
        if (0.0..COUNT as f32).contains(&p) {
            let b = p as usize;
            sums[b] += v;
            counts[b] += 1;
        }
    }
    let last = bins.len().saturating_sub(1);
    for (i, o) in out.iter_mut().enumerate() {
        *o = if counts[i] > 0 {
            sums[i] / counts[i] as f32
        } else {
            let k = ((centre(i) / bin_hz).round() as usize).clamp(1.min(last), last);
            bins.get(k).copied().unwrap_or(0.0)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bands_cover_the_range_evenly() {
        assert!(position(LOWEST).abs() < 1e-4);
        assert!((position(HIGHEST) - COUNT as f32).abs() < 1e-3);
        assert_eq!(band_of(10.0), 0);
        assert_eq!(band_of(30_000.0), COUNT - 1);
        // Each band is the same ratio wide.
        let ratio = centre(1) / centre(0);
        assert!((centre(40) / centre(39) - ratio).abs() < 1e-3);
        assert_eq!(band_of(centre(17)), 17);
    }

    #[test]
    fn values_go_straight_between_centres_and_back_from_bins() {
        let mut v = [0f32; COUNT];
        v[10] = 10.0;
        assert!((between(&v, centre(10)) - 10.0).abs() < 1e-3);
        let halfway = (centre(10) * centre(11)).sqrt();
        assert!((between(&v, halfway) - 5.0).abs() < 1e-2);
        // Bins of 1 Hz, value = frequency: each band's mean lands inside it.
        let bins: Vec<f32> = (0..24_000).map(|k| k as f32).collect();
        let mut out = [0f32; COUNT];
        from_bins(&bins, 1.0, &mut out);
        for (i, o) in out.iter().enumerate() {
            assert_eq!(band_of(*o), i, "band {i}: {o}");
        }
        // Wide bins: narrow low bands take the nearest bin.
        let wide: Vec<f32> = (0..513).map(|k| k as f32 * 46.875).collect();
        from_bins(&wide, 46.875, &mut out);
        assert!(out.iter().all(|v| *v > 0.0));
    }
}
