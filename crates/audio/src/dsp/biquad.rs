//! Second-order filters: the usual cookbook shapes (high pass, band pass,
//! peak, low and high shelf), worked out in double precision and run in
//! single, as a transposed direct form II with one state per channel.
//!
//! [`Coefs::response_db`] is the filter's gain at a frequency, worked out
//! from the coefficients: the tests check the filters against it and the
//! formulas, and the Audio page draws the EQ's curve with it.

use std::f64::consts::PI;

/// A filter's coefficients, normalised so `a0` is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coefs {
    /// Lets everything through as it is.
    pub const THROUGH: Coefs = Coefs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Coefs {
        Coefs {
            b0: (b[0] / a[0]) as f32,
            b1: (b[1] / a[0]) as f32,
            b2: (b[2] / a[0]) as f32,
            a1: (a[1] / a[0]) as f32,
            a2: (a[2] / a[0]) as f32,
        }
    }

    /// Cosine and sine of the frequency's angle, kept just inside the
    /// range a filter can have (above zero, below half the rate).
    fn angle(rate: f32, freq: f32) -> (f64, f64) {
        let w = 2.0 * PI * (f64::from(freq) / f64::from(rate)).clamp(1e-5, 0.49);
        (w.cos(), w.sin())
    }

    /// Takes out what is below `freq` (12 dB an octave beyond it).
    pub fn high_pass(rate: f32, freq: f32, q: f32) -> Coefs {
        let (c, s) = Coefs::angle(rate, freq);
        let alpha = s / (2.0 * f64::from(q));
        Coefs::normalised(
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        )
    }

    /// Keeps a band around `freq`, 0 dB at its centre.
    pub fn band_pass(rate: f32, freq: f32, q: f32) -> Coefs {
        let (c, s) = Coefs::angle(rate, freq);
        let alpha = s / (2.0 * f64::from(q));
        Coefs::normalised([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    /// Raises or lowers a band around `freq` by `gain_db` at its centre.
    pub fn peak(rate: f32, freq: f32, q: f32, gain_db: f32) -> Coefs {
        if gain_db == 0.0 {
            return Coefs::THROUGH;
        }
        let (c, s) = Coefs::angle(rate, freq);
        let alpha = s / (2.0 * f64::from(q));
        let a = 10f64.powf(f64::from(gain_db) / 40.0);
        Coefs::normalised(
            [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
        )
    }

    /// Raises or lowers everything below `freq` by `gain_db` (half of it
    /// at `freq`).
    pub fn low_shelf(rate: f32, freq: f32, gain_db: f32) -> Coefs {
        Coefs::shelf(rate, freq, gain_db, false)
    }

    /// Raises or lowers everything above `freq` by `gain_db` (half of it
    /// at `freq`).
    pub fn high_shelf(rate: f32, freq: f32, gain_db: f32) -> Coefs {
        Coefs::shelf(rate, freq, gain_db, true)
    }

    /// The cookbook shelves with a slope of 1 (the steepest without a bump).
    fn shelf(rate: f32, freq: f32, gain_db: f32, high: bool) -> Coefs {
        if gain_db == 0.0 {
            return Coefs::THROUGH;
        }
        let (c, s) = Coefs::angle(rate, freq);
        let a = 10f64.powf(f64::from(gain_db) / 40.0);
        let alpha = s / 2.0 * 2f64.sqrt();
        let root = 2.0 * a.sqrt() * alpha;
        // The high shelf is the low one with the cosine's sign turned.
        let k = if high { -1.0 } else { 1.0 };
        Coefs::normalised(
            [
                a * ((a + 1.0) - k * (a - 1.0) * c + root),
                k * 2.0 * a * ((a - 1.0) - k * (a + 1.0) * c),
                a * ((a + 1.0) - k * (a - 1.0) * c - root),
            ],
            [
                (a + 1.0) + k * (a - 1.0) * c + root,
                -k * 2.0 * ((a - 1.0) + k * (a + 1.0) * c),
                (a + 1.0) + k * (a - 1.0) * c - root,
            ],
        )
    }

    /// The loudness standard's K-weighting at any rate: the high shelf (the
    /// head's effect), then the high pass. The same filters as
    /// [`super::loudness`] measures with, run in single precision.
    pub fn k_weighting(rate: f32) -> (Coefs, Coefs) {
        let rate = f64::from(rate);
        let (f0, gain_db, q) = (
            1_681.974_450_955_533,
            3.999_843_853_973_347,
            0.707_175_236_955_419_6,
        );
        let k = (PI * f0 / rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let shelf = Coefs::normalised(
            [
                vh + vb * k / q + k * k,
                2.0 * (k * k - vh),
                vh - vb * k / q + k * k,
            ],
            [1.0 + k / q + k * k, 2.0 * (k * k - 1.0), 1.0 - k / q + k * k],
        );
        let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
        let k = (PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Coefs::normalised(
            [a0, -2.0 * a0, a0],
            [a0, 2.0 * (k * k - 1.0), 1.0 - k / q + k * k],
        );
        (shelf, high_pass)
    }

    /// The filter's gain at `freq`, in dB.
    pub fn response_db(&self, rate: f32, freq: f32) -> f32 {
        let w = 2.0 * PI * f64::from(freq) / f64::from(rate);
        // |B(e^jw)|² / |A(e^jw)|², with z^-1 = e^-jw.
        let power = |c0: f64, c1: f64, c2: f64| {
            let re = c0 + c1 * w.cos() + c2 * (2.0 * w).cos();
            let im = -(c1 * w.sin() + c2 * (2.0 * w).sin());
            re * re + im * im
        };
        let b = power(f64::from(self.b0), f64::from(self.b1), f64::from(self.b2));
        let a = power(1.0, f64::from(self.a1), f64::from(self.a2));
        (10.0 * (b / a).max(1e-30).log10()) as f32
    }
}

/// One filter for both channels.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub coefs: Coefs,
    state: [[f32; 2]; 2],
}

impl Default for Biquad {
    fn default() -> Biquad {
        Biquad::new(Coefs::THROUGH)
    }
}

impl Biquad {
    pub fn new(coefs: Coefs) -> Biquad {
        Biquad {
            coefs,
            state: [[0.0; 2]; 2],
        }
    }

    /// Whether it lets everything through, so it can be skipped.
    pub fn is_through(&self) -> bool {
        self.coefs == Coefs::THROUGH
    }

    /// Filters channel `ch`'s block in place.
    #[inline]
    pub fn process(&mut self, ch: usize, block: &mut [f32]) {
        let Coefs { b0, b1, b2, a1, a2 } = self.coefs;
        let [mut z1, mut z2] = self.state[ch];
        for s in block.iter_mut() {
            let x = *s;
            let y = b0 * x + z1;
            z1 = b1 * x - a1 * y + z2;
            z2 = b2 * x - a2 * y;
            *s = y;
        }
        self.state[ch] = [z1, z2];
    }

    /// One sample of channel `ch`, for detectors that run sample by sample.
    #[inline]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let Coefs { b0, b1, b2, a1, a2 } = self.coefs;
        let [z1, z2] = self.state[ch];
        let y = b0 * x + z1;
        self.state[ch] = [b1 * x - a1 * y + z2, b2 * x - a2 * y];
        y
    }

    /// Carries the running state over from the filter this one replaces.
    pub fn take_state(&mut self, old: &Biquad) {
        self.state = old.state;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const RATE: f32 = 48_000.0;

    /// The gain a filter gives a steady sine at `freq`, measured by running
    /// it (dB), after it has settled.
    pub fn measured_db(mut run: impl FnMut(&mut [f32]), freq: f32) -> f32 {
        let n = RATE as usize;
        let mut x: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / RATE).sin())
            .collect();
        for block in x.chunks_mut(256) {
            run(block);
        }
        let tail = &x[n / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        20.0 * (rms * 2f32.sqrt()).log10()
    }

    #[test]
    fn a_peak_has_its_gain_at_its_centre_and_none_far_away() {
        let c = Coefs::peak(RATE, 1_000.0, 1.0, 6.0);
        assert!((c.response_db(RATE, 1_000.0) - 6.0).abs() < 0.01);
        assert!(c.response_db(RATE, 50.0).abs() < 0.1);
        assert!(c.response_db(RATE, 20_000.0).abs() < 0.1);
        let mut f = Biquad::new(c);
        let got = measured_db(|b| f.process(0, b), 1_000.0);
        assert!((got - 6.0).abs() < 0.05, "{got}");
        assert_eq!(Coefs::peak(RATE, 1_000.0, 1.0, 0.0), Coefs::THROUGH);
    }

    #[test]
    fn shelves_lift_one_side_and_half_at_their_corner() {
        let low = Coefs::low_shelf(RATE, 200.0, -8.0);
        assert!((low.response_db(RATE, 20.0) + 8.0).abs() < 0.2);
        assert!((low.response_db(RATE, 200.0) + 4.0).abs() < 0.05);
        assert!(low.response_db(RATE, 10_000.0).abs() < 0.05);
        let high = Coefs::high_shelf(RATE, 5_000.0, 6.0);
        assert!((high.response_db(RATE, 20_000.0) - 6.0).abs() < 0.3);
        assert!((high.response_db(RATE, 5_000.0) - 3.0).abs() < 0.05);
        assert!(high.response_db(RATE, 100.0).abs() < 0.05);
    }

    #[test]
    fn a_high_pass_is_three_down_at_its_corner_for_butterworth() {
        let c = Coefs::high_pass(RATE, 100.0, std::f32::consts::FRAC_1_SQRT_2);
        assert!((c.response_db(RATE, 100.0) + 3.01).abs() < 0.02);
        assert!(c.response_db(RATE, 5_000.0).abs() < 0.01);
        // 12 dB an octave well below the corner.
        let drop = c.response_db(RATE, 20.0) - c.response_db(RATE, 10.0);
        assert!((drop - 12.0).abs() < 0.5, "{drop}");
    }

    #[test]
    fn a_band_pass_keeps_its_centre() {
        let c = Coefs::band_pass(RATE, 1_200.0, 0.7);
        assert!(c.response_db(RATE, 1_200.0).abs() < 0.01);
        assert!(c.response_db(RATE, 60.0) < -20.0);
    }
}
