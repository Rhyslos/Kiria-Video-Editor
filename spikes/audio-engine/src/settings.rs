//! A track's settings as plain values, and as a flat list of numbers (for
//! the designs that send one number at a time).

use crate::dsp::dynamics::{CompSettings, ExpandSettings};
use crate::dsp::filters::{BANDS, Band, Shape};
use crate::dsp::gate::GateSettings;
use crate::signal::Role;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clean {
    Off,
    Gate(GateSettings),
    Expand(ExpandSettings),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    pub hp: Option<f32>,
    pub eq: [Band; BANDS],
    pub comp: Option<CompSettings>,
    pub clean: Clean,
    /// Ducked under the dialogue by this depth (dB).
    pub duck: Option<f32>,
}

pub const GATE: GateSettings = GateSettings {
    threshold_db: 6.0,
    reduction_db: -18.0,
    release_ms: 120.0,
};
pub const EXPAND: ExpandSettings = ExpandSettings {
    threshold_db: -45.0,
    ratio: 2.0,
    floor_db: -18.0,
    attack_ms: 2.0,
    release_ms: 80.0,
};

impl TrackSettings {
    /// Every effect on, as in the stress run: clean-up by `clean`, ducking
    /// on every track that isn't dialogue.
    pub fn full(role: Role, clean: Clean) -> TrackSettings {
        let band = |shape, freq, gain_db| Band {
            shape,
            freq,
            q: 1.0,
            gain_db,
        };
        TrackSettings {
            hp: Some(if role == Role::Dialogue { 80.0 } else { 30.0 }),
            eq: [
                band(Shape::LowShelf, 120.0, -2.0),
                band(Shape::Peak, 300.0, -3.0),
                band(Shape::Peak, 1000.0, 1.0),
                band(Shape::Peak, 3000.0, 2.0),
                band(Shape::Peak, 6000.0, -1.5),
                band(Shape::HighShelf, 10000.0, 1.5),
            ],
            comp: Some(CompSettings {
                threshold_db: -24.0,
                ratio: 3.0,
                attack_ms: 5.0,
                release_ms: 120.0,
                knee_db: 6.0,
                makeup_db: 3.0,
            }),
            clean,
            duck: if role == Role::Dialogue { None } else { Some(-12.0) },
        }
    }

    /// Nothing on: the mix alone.
    pub fn none() -> TrackSettings {
        let mut s = TrackSettings::full(Role::Music, Clean::Off);
        s.hp = None;
        s.eq.iter_mut().for_each(|b| b.gain_db = 0.0);
        s.comp = None;
        s.duck = None;
        s
    }
}

/// How many numbers a track's settings are.
pub const P: usize = 2 + BANDS * 3 + 7 + 1 + 3 + 5 + 2;
/// The number the latency runs change: the third EQ band's frequency.
pub const PROBE: usize = 2 + 2 * 3;

impl TrackSettings {
    pub fn to_array(&self) -> [f32; P] {
        let mut a = [0f32; P];
        let mut i = 0;
        let mut put = |v: f32| {
            a[i] = v;
            i += 1;
        };
        put(self.hp.is_some() as u8 as f32);
        put(self.hp.unwrap_or(30.0));
        for b in &self.eq {
            put(b.freq);
            put(b.q);
            put(b.gain_db);
        }
        let c = self.comp.unwrap_or(CompSettings {
            threshold_db: 0.0,
            ratio: 1.0,
            attack_ms: 5.0,
            release_ms: 100.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        });
        put(self.comp.is_some() as u8 as f32);
        for v in [
            c.threshold_db,
            c.ratio,
            c.attack_ms,
            c.release_ms,
            c.knee_db,
            c.makeup_db,
        ] {
            put(v);
        }
        let (mode, g, e) = match self.clean {
            Clean::Off => (0.0, GATE, EXPAND),
            Clean::Gate(g) => (1.0, g, EXPAND),
            Clean::Expand(e) => (2.0, GATE, e),
        };
        put(mode);
        for v in [g.threshold_db, g.reduction_db, g.release_ms] {
            put(v);
        }
        for v in [e.threshold_db, e.ratio, e.floor_db, e.attack_ms, e.release_ms] {
            put(v);
        }
        put(self.duck.is_some() as u8 as f32);
        put(self.duck.unwrap_or(0.0));
        a
    }

    pub fn from_array(a: &[f32; P]) -> TrackSettings {
        let mut i = 0;
        let mut get = || {
            i += 1;
            a[i - 1]
        };
        let hp_on = get() != 0.0;
        let hp = get();
        let shapes = [
            Shape::LowShelf,
            Shape::Peak,
            Shape::Peak,
            Shape::Peak,
            Shape::Peak,
            Shape::HighShelf,
        ];
        let eq = shapes.map(|shape| Band {
            shape,
            freq: get(),
            q: get(),
            gain_db: get(),
        });
        let comp_on = get() != 0.0;
        let comp = CompSettings {
            threshold_db: get(),
            ratio: get(),
            attack_ms: get(),
            release_ms: get(),
            knee_db: get(),
            makeup_db: get(),
        };
        let mode = get();
        let g = GateSettings {
            threshold_db: get(),
            reduction_db: get(),
            release_ms: get(),
        };
        let e = ExpandSettings {
            threshold_db: get(),
            ratio: get(),
            floor_db: get(),
            attack_ms: get(),
            release_ms: get(),
        };
        let duck_on = get() != 0.0;
        let depth = get();
        TrackSettings {
            hp: hp_on.then_some(hp),
            eq,
            comp: comp_on.then_some(comp),
            clean: match mode as u8 {
                1 => Clean::Gate(g),
                2 => Clean::Expand(e),
                _ => Clean::Off,
            },
            duck: duck_on.then_some(depth),
        }
    }
}
