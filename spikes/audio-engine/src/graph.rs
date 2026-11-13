//! The three ways of holding a track's effects, and the graph that runs
//! them all: dialogue tracks first (they are the ducking key), then the
//! key detector, then every other track, then the master.
//!
//! - A `Vec<Effect>`: an enum per effect, matched on each block;
//! - B `Vec<Box<dyn Effect>>`: a trait object per effect;
//! - C `Strip`: one fixed struct per track holding every effect, each
//!   switched on or off.
//!
//! All three run the same effect code in the same order: high pass,
//! clean-up, EQ, compressor, ducking, fader (volume and pan lines), meter,
//! analyser tap.

use crate::dsp::Tap;
use crate::dsp::duck::{Ducker, KeyDetector, KeySettings};
use crate::dsp::dynamics::{CompSettings, Compressor, Expander};
use crate::dsp::fader::{Fader, Line};
use crate::dsp::filters::{Eq, HighPass};
use crate::dsp::gate::SpectralGate;
use crate::dsp::limiter::Limiter;
use crate::dsp::meter::{TrackMeter, TruePeak};
use crate::settings::{Clean, EXPAND, GATE, TrackSettings};
use crate::signal::{RATE, Role, Signal};
use std::sync::Arc;

pub const MAX_BLOCK: usize = 1024;

/// What every effect may look at besides its own sound.
pub struct Ctx<'a> {
    /// The block's first sample, from the start of playback.
    pub pos: u64,
    /// How much speech the key has, per sample of this block.
    pub activity: &'a [f32],
}

pub trait Effect: Send {
    fn process(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]);
}

impl Effect for HighPass {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        HighPass::process(self, l, r)
    }
}
impl Effect for Eq {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Eq::process(self, l, r)
    }
}
impl Effect for Compressor {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Compressor::process(self, l, r)
    }
}
impl Effect for SpectralGate {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        SpectralGate::process(self, l, r)
    }
}
impl Effect for Expander {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Expander::process(self, l, r)
    }
}
impl Effect for Ducker {
    fn process(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Ducker::process(self, ctx.activity, l, r)
    }
}
impl Effect for Fader {
    fn process(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Fader::process(self, ctx.pos, l, r)
    }
}
impl Effect for TrackMeter {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        TrackMeter::process(self, l, r)
    }
}
impl Effect for Tap {
    fn process(&mut self, _: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Tap::process(self, l, r)
    }
}

/// Design A's effect: one enum, matched.
pub enum Fx {
    HighPass(HighPass),
    Gate(Box<SpectralGate>),
    Expand(Expander),
    Eq(Eq),
    Comp(Compressor),
    Duck(Ducker),
    Fader(Fader),
    Meter(TrackMeter),
    Tap(Tap),
}

impl Fx {
    #[inline]
    fn process(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        match self {
            Fx::HighPass(e) => e.process(l, r),
            Fx::Gate(e) => e.process(l, r),
            Fx::Expand(e) => e.process(l, r),
            Fx::Eq(e) => e.process(l, r),
            Fx::Comp(e) => e.process(l, r),
            Fx::Duck(e) => e.process(ctx.activity, l, r),
            Fx::Fader(e) => e.process(ctx.pos, l, r),
            Fx::Meter(e) => e.process(l, r),
            Fx::Tap(e) => e.process(l, r),
        }
    }
}

/// A track's chain, whichever way it is held.
pub trait Chain: Send {
    fn run(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]);
}

impl Chain for Vec<Fx> {
    fn run(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        for fx in self.iter_mut() {
            fx.process(ctx, l, r);
        }
    }
}

impl Chain for Vec<Box<dyn Effect>> {
    fn run(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        for fx in self.iter_mut() {
            fx.process(ctx, l, r);
        }
    }
}

/// Design C: every effect, always there, switched on or off.
pub struct Strip {
    rate: f32,
    pub settings: TrackSettings,
    pub hp: HighPass,
    pub gate: SpectralGate,
    pub expand: Expander,
    pub eq: Eq,
    pub comp: Compressor,
    pub duck: Ducker,
    pub fader: Fader,
    pub meter: TrackMeter,
    pub tap: Option<Tap>,
}

impl Strip {
    /// Off the real-time thread: allocates.
    pub fn new(rate: f32, s: &TrackSettings, profile: &[f32], fader: Fader, tap: Option<Tap>) -> Strip {
        let comp_s = s.comp.unwrap_or(CompSettings {
            threshold_db: 0.0,
            ratio: 1.0,
            attack_ms: 5.0,
            release_ms: 100.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        });
        let gate_s = if let Clean::Gate(g) = s.clean { g } else { GATE };
        let exp_s = if let Clean::Expand(e) = s.clean { e } else { EXPAND };
        Strip {
            rate,
            settings: *s,
            hp: HighPass::new(rate, s.hp.unwrap_or(30.0)),
            gate: SpectralGate::new(rate, gate_s, profile),
            expand: Expander::new(rate, exp_s),
            eq: Eq::new(rate, s.eq),
            comp: Compressor::new(rate, comp_s),
            duck: Ducker {
                depth_db: s.duck.unwrap_or(0.0),
            },
            fader,
            meter: TrackMeter::new(rate),
            tap,
        }
    }

    /// New settings: only what changed is worked out again. Never
    /// allocates, so it can run on the real-time thread.
    pub fn set(&mut self, s: &TrackSettings) {
        if *s == self.settings {
            return;
        }
        if let Some(f) = s.hp {
            self.hp.set(self.rate, f);
        }
        self.eq.set(self.rate, &s.eq);
        if let Some(c) = s.comp {
            self.comp.set(c);
        }
        match s.clean {
            Clean::Gate(g) => self.gate.set(g),
            Clean::Expand(e) => self.expand.set(e),
            Clean::Off => {}
        }
        self.duck.depth_db = s.duck.unwrap_or(0.0);
        self.settings = *s;
    }

    /// Carries over everything that runs on from block to block (filter
    /// memories, envelopes, the clean-up's windows, meters) from the strip
    /// this one replaces, so swapping in a new one makes no click.
    pub fn take_state(&mut self, old: &mut Strip) {
        self.hp.take_state(&old.hp);
        self.gate.take_state(&old.gate);
        self.expand.take_state(&old.expand);
        self.eq.take_state(&old.eq);
        self.comp.take_state(&old.comp);
        self.fader.take_state(&old.fader);
        self.meter.take_state(&mut old.meter);
        if self.tap.is_none() {
            self.tap = old.tap.take();
        }
    }
}

impl Chain for Strip {
    fn run(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        let s = &self.settings;
        if s.hp.is_some() {
            self.hp.process(l, r);
        }
        match s.clean {
            Clean::Gate(_) => self.gate.process(l, r),
            Clean::Expand(_) => self.expand.process(l, r),
            Clean::Off => {}
        }
        self.eq.process(l, r);
        if s.comp.is_some() {
            self.comp.process(l, r);
        }
        if s.duck.is_some() {
            self.duck.process(ctx.activity, l, r);
        }
        self.fader.process(ctx.pos, l, r);
        self.meter.process(l, r);
        if let Some(t) = &mut self.tap {
            t.process(l, r);
        }
    }
}

impl Chain for Box<Strip> {
    fn run(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        Strip::run(self, ctx, l, r)
    }
}

/// Everything a chain needs that isn't settings: the noise profile, the
/// automation lines, the tap. Built once per track.
pub struct Parts {
    pub profile: Vec<f32>,
    pub fader: Fader,
    pub tap: Option<Tap>,
}

/// Design A's chain from settings.
pub fn chain_a(s: &TrackSettings, p: Parts) -> Vec<Fx> {
    let mut v = Vec::new();
    if let Some(f) = s.hp {
        v.push(Fx::HighPass(HighPass::new(RATE, f)));
    }
    match s.clean {
        Clean::Gate(g) => v.push(Fx::Gate(Box::new(SpectralGate::new(RATE, g, &p.profile)))),
        Clean::Expand(e) => v.push(Fx::Expand(Expander::new(RATE, e))),
        Clean::Off => {}
    }
    v.push(Fx::Eq(Eq::new(RATE, s.eq)));
    if let Some(c) = s.comp {
        v.push(Fx::Comp(Compressor::new(RATE, c)));
    }
    if let Some(d) = s.duck {
        v.push(Fx::Duck(Ducker { depth_db: d }));
    }
    v.push(Fx::Fader(p.fader));
    v.push(Fx::Meter(TrackMeter::new(RATE)));
    if let Some(t) = p.tap {
        v.push(Fx::Tap(t));
    }
    v
}

/// Design B's chain from settings.
pub fn chain_b(s: &TrackSettings, p: Parts) -> Vec<Box<dyn Effect>> {
    let mut v: Vec<Box<dyn Effect>> = Vec::new();
    if let Some(f) = s.hp {
        v.push(Box::new(HighPass::new(RATE, f)));
    }
    match s.clean {
        Clean::Gate(g) => v.push(Box::new(SpectralGate::new(RATE, g, &p.profile))),
        Clean::Expand(e) => v.push(Box::new(Expander::new(RATE, e))),
        Clean::Off => {}
    }
    v.push(Box::new(Eq::new(RATE, s.eq)));
    if let Some(c) = s.comp {
        v.push(Box::new(Compressor::new(RATE, c)));
    }
    if let Some(d) = s.duck {
        v.push(Box::new(Ducker { depth_db: d }));
    }
    v.push(Box::new(p.fader));
    v.push(Box::new(TrackMeter::new(RATE)));
    if let Some(t) = p.tap {
        v.push(Box::new(t));
    }
    v
}

/// Design C's chain from settings.
pub fn chain_c(s: &TrackSettings, p: Parts) -> Strip {
    Strip::new(RATE, s, &p.profile, p.fader, p.tap)
}

/// The master: compressor, true-peak limiter at -1 dBTP, meters.
pub struct Master {
    pub comp: Compressor,
    pub limiter: Limiter,
    pub meter: TrackMeter,
    tp: TruePeak,
    /// Highest true peak of the output so far (linear).
    pub tp_max: f32,
    pub on: bool,
}

impl Master {
    pub fn new(on: bool) -> Master {
        Master {
            comp: Compressor::new(
                RATE,
                CompSettings {
                    threshold_db: -12.0,
                    ratio: 2.0,
                    attack_ms: 10.0,
                    release_ms: 200.0,
                    knee_db: 6.0,
                    makeup_db: 0.0,
                },
            ),
            limiter: Limiter::new(RATE, -1.0, 80.0),
            meter: TrackMeter::new(RATE),
            tp: TruePeak::new(),
            tp_max: 0.0,
            on,
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if self.on {
            self.comp.process(l, r);
            self.limiter.process(l, r);
        }
        self.meter.process(l, r);
        for (&a, &b) in l.iter().zip(r.iter()) {
            self.tp_max = self.tp_max.max(self.tp.push(a, b));
        }
    }
}

/// One track: its chain, where its sound comes from, its readout.
pub struct Track<C> {
    pub chain: C,
    pub role: Role,
    pub signal: usize,
    pub offset: usize,
}

pub struct Graph<C> {
    pub tracks: Vec<Track<C>>,
    key: KeyDetector,
    signals: Arc<Vec<Signal>>,
    bufs: Vec<[Vec<f32>; 2]>,
    key_buf: [Vec<f32>; 2],
    activity: Vec<f32>,
    pub master: Master,
    pub pos: u64,
}

impl<C: Chain> Graph<C> {
    pub fn new(tracks: Vec<Track<C>>, signals: Arc<Vec<Signal>>, master_on: bool) -> Graph<C> {
        let n = tracks.len();
        Graph {
            tracks,
            key: KeyDetector::new(
                RATE,
                KeySettings {
                    threshold_db: -40.0,
                    hold_ms: 300.0,
                    down_ms: 40.0,
                    up_ms: 400.0,
                },
            ),
            signals,
            bufs: (0..n)
                .map(|_| [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]])
                .collect(),
            key_buf: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            activity: vec![0.0; MAX_BLOCK],
            master: Master::new(master_on),
            pos: 0,
        }
    }

    /// Stands in for taking each track's decoded sound from its ring.
    fn read(&mut self, t: usize, n: usize) {
        let tr = &self.tracks[t];
        let sig = &self.signals[tr.signal];
        let len = sig.l.len();
        let mut at = (self.pos as usize + tr.offset) % len;
        let [l, r] = &mut self.bufs[t];
        let mut done = 0;
        while done < n {
            let take = (n - done).min(len - at);
            l[done..done + take].copy_from_slice(&sig.l[at..at + take]);
            r[done..done + take].copy_from_slice(&sig.r[at..at + take]);
            done += take;
            at = 0;
        }
    }

    /// Mixes the next `out_l.len()` samples into `out_l`, `out_r`.
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        out_l.fill(0.0);
        out_r.fill(0.0);
        self.key_buf[0][..n].fill(0.0);
        self.key_buf[1][..n].fill(0.0);
        let pos = self.pos;
        for pass in [true, false] {
            if !pass {
                let [kl, kr] = &self.key_buf;
                self.key.detect(&kl[..n], &kr[..n], &mut self.activity[..n]);
            }
            for t in 0..self.tracks.len() {
                if (self.tracks[t].role == Role::Dialogue) != pass {
                    continue;
                }
                self.read(t, n);
                let ctx = Ctx {
                    pos,
                    activity: &self.activity[..n],
                };
                let [l, r] = &mut self.bufs[t];
                self.tracks[t].chain.run(&ctx, &mut l[..n], &mut r[..n]);
                for i in 0..n {
                    out_l[i] += l[i];
                    out_r[i] += r[i];
                }
                if pass {
                    for i in 0..n {
                        self.key_buf[0][i] += l[i];
                        self.key_buf[1][i] += r[i];
                    }
                }
            }
        }
        self.master.process(out_l, out_r);
        self.pos += n as u64;
    }
}

/// The volume and pan lines of track `t`: a point every half second.
pub fn automation(t: usize, seconds: f32) -> Fader {
    let mut rng = crate::signal::Rng::new(t as u64 + 999);
    let step = (RATE * 0.5) as u64;
    let points = (seconds * 2.0) as u64 + 2;
    let vol = (0..points).map(|i| (i * step, -6.0 + 4.0 * rng.s())).collect();
    let pan = (0..points).map(|i| (i * step, 0.5 * rng.s())).collect();
    Fader {
        volume_db: Line::new(vol),
        pan: Line::new(pan),
    }
}
