//! The track's high-pass filter and parametric EQ.

use super::biquad::{Biquad, Coefs};

/// 24 dB per octave: two second-order sections (Butterworth Qs).
#[derive(Debug, Clone)]
pub struct HighPass {
    pub freq: f32,
    stages: [Biquad; 2],
}

impl HighPass {
    pub fn new(rate: f32, freq: f32) -> HighPass {
        let mut h = HighPass {
            freq: 0.0,
            stages: [Biquad::default(); 2],
        };
        h.set(rate, freq);
        h
    }

    pub fn set(&mut self, rate: f32, freq: f32) {
        if freq == self.freq {
            return;
        }
        self.freq = freq;
        self.stages[0].c = Coefs::highpass(rate, freq, 0.541_196_1);
        self.stages[1].c = Coefs::highpass(rate, freq, 1.306_563);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for s in &mut self.stages {
            s.process(0, l);
            s.process(1, r);
        }
    }

    pub fn take_state(&mut self, old: &HighPass) {
        for (n, o) in self.stages.iter_mut().zip(&old.stages) {
            let c = n.c;
            *n = *o;
            n.c = c;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    LowShelf,
    Peak,
    HighShelf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub shape: Shape,
    pub freq: f32,
    pub q: f32,
    pub gain_db: f32,
}

pub const BANDS: usize = 6;

/// Six bands: a low shelf, four peaks, a high shelf.
#[derive(Debug, Clone)]
pub struct Eq {
    pub bands: [Band; BANDS],
    filters: [Biquad; BANDS],
}

impl Eq {
    pub fn new(rate: f32, bands: [Band; BANDS]) -> Eq {
        let mut e = Eq {
            bands,
            filters: [Biquad::default(); BANDS],
        };
        for i in 0..BANDS {
            e.filters[i].c = Eq::coefs(rate, &bands[i]);
        }
        e
    }

    fn coefs(rate: f32, b: &Band) -> Coefs {
        match b.shape {
            Shape::LowShelf => Coefs::low_shelf(rate, b.freq, b.gain_db),
            Shape::Peak => Coefs::peaking(rate, b.freq, b.q, b.gain_db),
            Shape::HighShelf => Coefs::high_shelf(rate, b.freq, b.gain_db),
        }
    }

    /// Recomputes only the bands that changed.
    pub fn set(&mut self, rate: f32, bands: &[Band; BANDS]) {
        for i in 0..BANDS {
            if bands[i] != self.bands[i] {
                self.bands[i] = bands[i];
                self.filters[i].c = Eq::coefs(rate, &bands[i]);
            }
        }
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
        for (n, o) in self.filters.iter_mut().zip(&old.filters) {
            let c = n.c;
            *n = *o;
            n.c = c;
        }
    }
}
