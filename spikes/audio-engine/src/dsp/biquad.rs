//! Second-order filters (the usual cookbook shapes), transposed direct
//! form II, one state per channel.

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coefs {
    pub const THROUGH: Coefs = Coefs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn norm(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Coefs {
        Coefs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn w(rate: f32, f: f32) -> (f32, f32) {
        let w = 2.0 * PI * (f / rate).clamp(1e-5, 0.49);
        (w.cos(), w.sin())
    }

    pub fn highpass(rate: f32, f: f32, q: f32) -> Coefs {
        let (c, s) = Coefs::w(rate, f);
        let a = s / (2.0 * q);
        Coefs::norm(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + a,
            -2.0 * c,
            1.0 - a,
        )
    }

    pub fn bandpass(rate: f32, f: f32, q: f32) -> Coefs {
        let (c, s) = Coefs::w(rate, f);
        let a = s / (2.0 * q);
        Coefs::norm(a, 0.0, -a, 1.0 + a, -2.0 * c, 1.0 - a)
    }

    pub fn peaking(rate: f32, f: f32, q: f32, gain_db: f32) -> Coefs {
        if gain_db == 0.0 {
            return Coefs::THROUGH;
        }
        let (c, s) = Coefs::w(rate, f);
        let a = s / (2.0 * q);
        let g = 10f32.powf(gain_db / 40.0);
        Coefs::norm(
            1.0 + a * g,
            -2.0 * c,
            1.0 - a * g,
            1.0 + a / g,
            -2.0 * c,
            1.0 - a / g,
        )
    }

    fn shelf(rate: f32, f: f32, gain_db: f32, high: bool) -> Coefs {
        if gain_db == 0.0 {
            return Coefs::THROUGH;
        }
        let (c, s) = Coefs::w(rate, f);
        let g = 10f32.powf(gain_db / 40.0);
        let a = s / 2.0 * 2f32.sqrt();
        let sq = 2.0 * g.sqrt() * a;
        if high {
            Coefs::norm(
                g * ((g + 1.0) + (g - 1.0) * c + sq),
                -2.0 * g * ((g - 1.0) + (g + 1.0) * c),
                g * ((g + 1.0) + (g - 1.0) * c - sq),
                (g + 1.0) - (g - 1.0) * c + sq,
                2.0 * ((g - 1.0) - (g + 1.0) * c),
                (g + 1.0) - (g - 1.0) * c - sq,
            )
        } else {
            Coefs::norm(
                g * ((g + 1.0) - (g - 1.0) * c + sq),
                2.0 * g * ((g - 1.0) - (g + 1.0) * c),
                g * ((g + 1.0) - (g - 1.0) * c - sq),
                (g + 1.0) + (g - 1.0) * c + sq,
                -2.0 * ((g - 1.0) + (g + 1.0) * c),
                (g + 1.0) + (g - 1.0) * c - sq,
            )
        }
    }

    pub fn low_shelf(rate: f32, f: f32, gain_db: f32) -> Coefs {
        Coefs::shelf(rate, f, gain_db, false)
    }

    pub fn high_shelf(rate: f32, f: f32, gain_db: f32) -> Coefs {
        Coefs::shelf(rate, f, gain_db, true)
    }

    /// The loudness standard's K-weighting, worked out for any rate:
    /// the high shelf, then the high pass.
    pub fn k_weighting(rate: f32) -> (Coefs, Coefs) {
        let rate = f64::from(rate);
        let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Coefs {
            b0: ((vh + vb * k / q + k * k) / a0) as f32,
            b1: (2.0 * (k * k - vh) / a0) as f32,
            b2: ((vh - vb * k / q + k * k) / a0) as f32,
            a1: (2.0 * (k * k - 1.0) / a0) as f32,
            a2: ((1.0 - k / q + k * k) / a0) as f32,
        };
        let (f0, q) = (38.13547087602444, 0.5003270373238773);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high = Coefs {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: (2.0 * (k * k - 1.0) / a0) as f32,
            a2: ((1.0 - k / q + k * k) / a0) as f32,
        };
        (shelf, high)
    }
}

/// One filter for both channels.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub c: Coefs,
    z: [[f32; 2]; 2],
}

impl Default for Biquad {
    fn default() -> Biquad {
        Biquad::new(Coefs::THROUGH)
    }
}

impl Biquad {
    pub fn new(c: Coefs) -> Biquad {
        Biquad { c, z: [[0.0; 2]; 2] }
    }

    pub fn is_through(&self) -> bool {
        self.c == Coefs::THROUGH
    }

    #[inline]
    pub fn process(&mut self, ch: usize, x: &mut [f32]) {
        let Coefs { b0, b1, b2, a1, a2 } = self.c;
        let [mut z1, mut z2] = self.z[ch];
        for s in x.iter_mut() {
            let i = *s;
            let o = b0 * i + z1;
            z1 = b1 * i - a1 * o + z2;
            z2 = b2 * i - a2 * o;
            *s = o;
        }
        self.z[ch] = [z1, z2];
    }

    /// One sample, for detectors that run sample by sample.
    #[inline]
    pub fn tick(&mut self, ch: usize, i: f32) -> f32 {
        let Coefs { b0, b1, b2, a1, a2 } = self.c;
        let [z1, z2] = self.z[ch];
        let o = b0 * i + z1;
        self.z[ch] = [b1 * i - a1 * o + z2, b2 * i - a2 * o];
        o
    }
}
