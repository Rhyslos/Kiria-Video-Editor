//! The compressor (per track and on the master) and the downward
//! expander (the simple way of noise clean-up).

const DB: f32 = 8.685_889_6; // 20 / ln(10)

#[inline]
pub fn to_db(x: f32) -> f32 {
    (x + 1e-9).ln() * DB
}

#[inline]
pub fn from_db(db: f32) -> f32 {
    (db / DB).exp()
}

/// The coefficient of a one-pole smoother reaching 63 % in `ms`.
pub fn coef(rate: f32, ms: f32) -> f32 {
    (-1.0 / (rate * ms.max(0.01) * 0.001)).exp()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompSettings {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
}

/// Feed-forward, stereo-linked, peak detection, soft knee, smoothing in dB.
#[derive(Debug, Clone)]
pub struct Compressor {
    pub s: CompSettings,
    rate: f32,
    att: f32,
    rel: f32,
    /// Gain reduction now, in dB (≤ 0).
    gr: f32,
    /// The deepest gain reduction in the last block, for the meter.
    pub gr_meter: f32,
}

impl Compressor {
    pub fn new(rate: f32, s: CompSettings) -> Compressor {
        Compressor {
            s,
            rate,
            att: coef(rate, s.attack_ms),
            rel: coef(rate, s.release_ms),
            gr: 0.0,
            gr_meter: 0.0,
        }
    }

    pub fn set(&mut self, s: CompSettings) {
        if s != self.s {
            self.s = s;
            self.att = coef(self.rate, s.attack_ms);
            self.rel = coef(self.rate, s.release_ms);
        }
    }

    /// The static curve: gain change in dB for a level in dB.
    #[inline]
    fn curve(&self, x: f32) -> f32 {
        let CompSettings {
            threshold_db: t,
            ratio,
            knee_db: w,
            ..
        } = self.s;
        let over = x - t;
        let slope = 1.0 / ratio - 1.0;
        if 2.0 * over <= -w {
            0.0
        } else if 2.0 * over >= w {
            slope * over
        } else {
            let k = over + w / 2.0;
            slope * k * k / (2.0 * w)
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let makeup = self.s.makeup_db;
        let mut deepest = 0f32;
        let (att, rel) = (self.att, self.rel);
        let mut gr = self.gr;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let x = to_db(a.abs().max(b.abs()));
            let want = self.curve(x);
            let c = if want < gr { att } else { rel };
            gr = want + c * (gr - want);
            deepest = deepest.min(gr);
            let g = from_db(gr + makeup);
            *a *= g;
            *b *= g;
        }
        self.gr = gr;
        self.gr_meter = deepest;
    }

    pub fn take_state(&mut self, old: &Compressor) {
        self.gr = old.gr;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpandSettings {
    pub threshold_db: f32,
    pub ratio: f32,
    /// How far down it may go (dB, ≤ 0).
    pub floor_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

/// Turns the sound down further below a threshold: quiet hiss between
/// words falls away; words are left as they are.
#[derive(Debug, Clone)]
pub struct Expander {
    pub s: ExpandSettings,
    rate: f32,
    att: f32,
    rel: f32,
    env: f32,
    gain: f32,
}

impl Expander {
    pub fn new(rate: f32, s: ExpandSettings) -> Expander {
        Expander {
            s,
            rate,
            att: coef(rate, s.attack_ms),
            rel: coef(rate, s.release_ms),
            env: 0.0,
            gain: 0.0,
        }
    }

    pub fn set(&mut self, s: ExpandSettings) {
        if s != self.s {
            self.s = s;
            self.att = coef(self.rate, s.attack_ms);
            self.rel = coef(self.rate, s.release_ms);
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let ExpandSettings {
            threshold_db: t,
            ratio,
            floor_db,
            ..
        } = self.s;
        let env_c = coef(self.rate, 20.0);
        let (mut env, mut gain) = (self.env, self.gain);
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let x = a.abs().max(b.abs());
            env = x.max(env * env_c);
            let lvl = to_db(env);
            let want = if lvl < t {
                ((lvl - t) * (ratio - 1.0)).max(floor_db)
            } else {
                0.0
            };
            // Opening is the attack, closing the release.
            let c = if want > gain { self.att } else { self.rel };
            gain = want + c * (gain - want);
            let g = from_db(gain);
            *a *= g;
            *b *= g;
        }
        self.env = env;
        self.gain = gain;
    }

    pub fn take_state(&mut self, old: &Expander) {
        self.env = old.env;
        self.gain = old.gain;
    }
}
