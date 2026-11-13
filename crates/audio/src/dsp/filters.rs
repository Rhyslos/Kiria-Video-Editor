//! The track's high pass and its parametric EQ.

use super::biquad::{Biquad, Coefs};

/// Takes out rumble, handling noise and hum below a frequency: 24 dB an
/// octave (two Butterworth sections), so what is just above it is left as
/// it is.
#[derive(Debug, Clone)]
pub struct HighPass {
    rate: f32,
    freq: f32,
    stages: [Biquad; 2],
}

impl HighPass {
    /// The Qs of a fourth-order Butterworth filter's two sections.
    const QS: [f32; 2] = [0.541_196_1, 1.306_563];

    pub fn new(rate: f32, freq: f32) -> HighPass {
        let mut h = HighPass {
            rate,
            freq: f32::NAN,
            stages: [Biquad::default(); 2],
        };
        h.set(freq);
        h
    }

    /// A new corner frequency; nothing is worked out when it is the same.
    pub fn set(&mut self, freq: f32) {
        if freq == self.freq {
            return;
        }
        self.freq = freq;
        for (stage, q) in self.stages.iter_mut().zip(HighPass::QS) {
            stage.coefs = Coefs::high_pass(self.rate, freq, q);
        }
    }

    /// Its gain at `freq`, in dB (for drawing and tests).
    pub fn response_db(&self, freq: f32) -> f32 {
        self.stages
            .iter()
            .map(|s| s.coefs.response_db(self.rate, freq))
            .sum()
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for s in &mut self.stages {
            s.process(0, l);
            s.process(1, r);
        }
    }

    pub fn take_state(&mut self, old: &HighPass) {
        for (new, old) in self.stages.iter_mut().zip(&old.stages) {
            new.take_state(old);
        }
    }
}

/// The shape of an EQ band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Everything below the frequency up or down.
    LowShelf,
    /// A bump or a dip around the frequency, as wide as its Q says.
    Peak,
    /// Everything above the frequency up or down.
    HighShelf,
}

/// One band of the EQ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub on: bool,
    pub shape: Shape,
    pub freq: f32,
    /// How narrow a peak is (higher is narrower); shelves ignore it.
    pub q: f32,
    pub gain_db: f32,
}

impl Band {
    fn coefs(&self, rate: f32) -> Coefs {
        if !self.on {
            return Coefs::THROUGH;
        }
        match self.shape {
            Shape::LowShelf => Coefs::low_shelf(rate, self.freq, self.gain_db),
            Shape::Peak => Coefs::peak(rate, self.freq, self.q, self.gain_db),
            Shape::HighShelf => Coefs::high_shelf(rate, self.freq, self.gain_db),
        }
    }
}

/// How many bands the EQ has.
pub const BANDS: usize = 6;

/// The six bands as they start: a low shelf, four peaks, a high shelf, all
/// flat.
pub const FLAT: [Band; BANDS] = {
    const fn band(shape: Shape, freq: f32) -> Band {
        Band {
            on: true,
            shape,
            freq,
            q: 1.0,
            gain_db: 0.0,
        }
    }
    [
        band(Shape::LowShelf, 100.0),
        band(Shape::Peak, 250.0),
        band(Shape::Peak, 1_000.0),
        band(Shape::Peak, 3_000.0),
        band(Shape::Peak, 6_000.0),
        band(Shape::HighShelf, 10_000.0),
    ]
};

/// The parametric EQ: six bands, each its own filter; flat bands cost
/// nothing.
#[derive(Debug, Clone)]
pub struct Eq {
    rate: f32,
    bands: [Band; BANDS],
    filters: [Biquad; BANDS],
}

impl Eq {
    pub fn new(rate: f32, bands: [Band; BANDS]) -> Eq {
        Eq {
            rate,
            bands,
            filters: bands.map(|b| Biquad::new(b.coefs(rate))),
        }
    }

    pub fn bands(&self) -> &[Band; BANDS] {
        &self.bands
    }

    /// Sets band `i`; only that band is worked out again.
    pub fn set_band(&mut self, i: usize, band: Band) {
        if let (Some(b), Some(f)) = (self.bands.get_mut(i), self.filters.get_mut(i))
            && *b != band
        {
            *b = band;
            f.coefs = band.coefs(self.rate);
        }
    }

    /// The whole EQ's gain at `freq`, in dB: the curve the page draws.
    pub fn response_db(&self, freq: f32) -> f32 {
        self.filters
            .iter()
            .map(|f| f.coefs.response_db(self.rate, freq))
            .sum()
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for f in &mut self.filters {
            if !f.is_through() {
                f.process(0, l);
                f.process(1, r);
            }
        }
    }

    pub fn take_state(&mut self, old: &Eq) {
        for (new, old) in self.filters.iter_mut().zip(&old.filters) {
            new.take_state(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::biquad::tests::{RATE, measured_db};

    #[test]
    fn the_high_pass_falls_24_db_an_octave_below_its_corner() {
        let h = HighPass::new(RATE, 80.0);
        assert!(
            (h.response_db(80.0) + 3.01).abs() < 0.05,
            "Butterworth: -3 dB at the corner"
        );
        assert!(h.response_db(1_000.0).abs() < 0.01);
        let octave = h.response_db(20.0) - h.response_db(10.0);
        assert!((octave - 24.0).abs() < 0.5, "{octave}");
        let mut run = HighPass::new(RATE, 80.0);
        let mut other = vec![0f32; 256];
        let got = measured_db(|b| run.process(b, &mut other), 40.0);
        assert!((got - run.response_db(40.0)).abs() < 0.1, "{got}");
        assert!(got < -20.0);
    }

    #[test]
    fn each_band_does_what_its_settings_say_and_flat_does_nothing() {
        let flat = Eq::new(RATE, FLAT);
        for f in [30.0, 300.0, 3_000.0, 15_000.0] {
            assert_eq!(flat.response_db(f), 0.0);
        }
        let mut eq = Eq::new(RATE, FLAT);
        eq.set_band(
            2,
            Band {
                gain_db: -6.0,
                q: 2.0,
                ..FLAT[2]
            },
        );
        eq.set_band(
            5,
            Band {
                gain_db: 4.0,
                ..FLAT[5]
            },
        );
        assert!((eq.response_db(1_000.0) + 6.0).abs() < 0.05);
        assert!((eq.response_db(19_000.0) - 4.0).abs() < 0.3);
        assert!(eq.response_db(100.0).abs() < 0.1);
        let mut other = vec![0f32; 256];
        let got = measured_db(|b| eq.process(b, &mut other), 1_000.0);
        assert!((got + 6.0).abs() < 0.1, "{got}");
        // A band turned off is flat again.
        eq.set_band(
            2,
            Band {
                on: false,
                ..eq.bands()[2]
            },
        );
        assert!(eq.response_db(1_000.0).abs() < 0.05);
    }
}
