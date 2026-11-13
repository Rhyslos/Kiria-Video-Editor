//! Timing the three ways of holding effects on the same 32 tracks.

use crate::alloc;
use crate::dsp::Tap;
use crate::dsp::gate::SpectralGate;
use crate::graph::{self, Chain, Graph, Parts, Track};
use crate::settings::{Clean, EXPAND, GATE, TrackSettings};
use crate::signal::{self, RATE, Signal};
use std::sync::Arc;
use std::time::Instant;

pub const TRACKS: usize = 32;
pub const SECONDS: f32 = 10.0;

/// The noise profile every clean-up uses: learned from the dialogue's
/// hiss and hum alone.
pub fn profile() -> Vec<f32> {
    let n = (RATE * 2.0) as usize;
    let h = signal::hiss(n, 0.003, 18);
    let noise: Vec<f32> = (0..n)
        .map(|i| h[i] + 0.01 * (2.0 * std::f32::consts::PI * 50.0 * i as f32 / RATE).sin())
        .collect();
    SpectralGate::learn(&noise)
}

pub fn build<C: Chain>(
    make: &dyn Fn(&TrackSettings, Parts) -> C,
    settings: &dyn Fn(usize) -> TrackSettings,
    signals: &Arc<Vec<Signal>>,
    profile: &[f32],
    mut tap: Option<Tap>,
    master: bool,
) -> Graph<C> {
    let tracks = (0..TRACKS)
        .map(|t| {
            let role = signal::role_of(t);
            let parts = Parts {
                profile: profile.to_vec(),
                fader: graph::automation(t, SECONDS),
                tap: if t == 0 { tap.take() } else { None },
            };
            let chain = make(&settings(t), parts);
            Track {
                chain,
                role,
                signal: role as usize * 2 + t % 2,
                offset: t * 7919,
            }
        })
        .collect();
    Graph::new(tracks, Arc::clone(signals), master)
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub mean: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
    pub count: usize,
}

impl Stats {
    pub fn of(us: &mut [f64]) -> Stats {
        if us.is_empty() {
            return Stats::default();
        }
        us.sort_by(|a, b| a.total_cmp(b));
        let at = |q: f64| us[((us.len() - 1) as f64 * q).round() as usize];
        Stats {
            mean: us.iter().sum::<f64>() / us.len() as f64,
            p50: at(0.5),
            p99: at(0.99),
            max: us[us.len() - 1],
            count: us.len(),
        }
    }
}

pub struct Run {
    pub stats: Stats,
    /// The thread's own processor time per block, where the system says.
    pub cpu: Option<Stats>,
    pub allocs: usize,
    /// A fingerprint of the output, to check the designs sound the same.
    pub sum: f64,
    pub tp_max_db: f32,
    pub master_momentary: f32,
}

/// Processes the whole input once, in blocks of `block`, timing each.
pub fn run<C: Chain>(g: &mut Graph<C>, block: usize) -> Run {
    let blocks = (SECONDS * RATE) as usize / block;
    let mut times = Vec::with_capacity(blocks);
    let mut cpu = Vec::with_capacity(blocks);
    let (mut l, mut r) = (vec![0f32; block], vec![0f32; block]);
    let mut sum = 0f64;
    alloc::reset();
    alloc::on_rt(|| {
        for _ in 0..blocks {
            let c0 = crate::os::thread_cpu_us();
            let t0 = Instant::now();
            g.process(&mut l, &mut r);
            times.push(t0.elapsed().as_secs_f64() * 1e6);
            if let (Some(a), Some(b)) = (c0, crate::os::thread_cpu_us()) {
                cpu.push(b - a);
            }
            for (a, b) in l.iter().zip(&r) {
                sum += f64::from(a.abs()) + 2.0 * f64::from(b.abs());
            }
        }
    });
    let allocs = alloc::count();
    let m = crate::dsp::meter::Readout::get(&g.master.meter.readout.momentary);
    Run {
        stats: Stats::of(&mut times),
        cpu: (!cpu.is_empty()).then(|| Stats::of(&mut cpu)),
        allocs,
        sum,
        tp_max_db: 20.0 * g.master.tp_max.log10(),
        master_momentary: m,
    }
}

pub fn full_gate(t: usize) -> TrackSettings {
    TrackSettings::full(signal::role_of(t), Clean::Gate(GATE))
}

pub fn full_expand(t: usize) -> TrackSettings {
    TrackSettings::full(signal::role_of(t), Clean::Expand(EXPAND))
}

/// How well a clean-up method does on speech over steady noise at
/// `noise` (peak, linear): how far the pauses go down and how far the
/// words go down (dB), well inside each, delay taken into account.
pub fn cleanup_quality(noise: f32, gate: bool) -> (f64, f64) {
    use crate::dsp::dynamics::{ExpandSettings, Expander};
    let n = (RATE * 8.0) as usize;
    let (speech, talking) = signal::speech(n, 17);
    let hiss = signal::hiss(n, noise, 99);
    let input: Vec<f32> = speech.iter().zip(&hiss).map(|(a, b)| a + b).collect();
    let (mut l, mut r) = (input.clone(), input.clone());
    let lat = if gate {
        let profile = SpectralGate::learn(&signal::hiss((RATE * 2.0) as usize, noise, 5));
        let mut g = SpectralGate::new(RATE, GATE, &profile);
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            g.process(a, b);
        }
        SpectralGate::latency()
    } else {
        // Threshold 6 dB over the noise's peaks, as a person would set it.
        let s = ExpandSettings {
            threshold_db: 20.0 * noise.log10() + 6.0,
            ..EXPAND
        };
        let mut e = Expander::new(RATE, s);
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            e.process(a, b);
        }
        0
    };
    // Samples at least 0.15 s from any change between words and pauses.
    let far = 7_200usize;
    let mut since = vec![0usize; n];
    let mut until = vec![0usize; n];
    for i in 1..n {
        since[i] = if talking[i] == talking[i - 1] {
            since[i - 1] + 1
        } else {
            0
        };
    }
    for i in (0..n - 1).rev() {
        until[i] = if talking[i] == talking[i + 1] {
            until[i + 1] + 1
        } else {
            0
        };
    }
    let energy = |sig: &[f32], want: bool, shift: usize| {
        let (mut e, mut k) = (0f64, 0usize);
        for i in RATE as usize..n - lat {
            if talking[i] == want && since[i] >= far && until[i] >= far {
                e += f64::from(sig[i + shift]).powi(2);
                k += 1;
            }
        }
        10.0 * (e / k.max(1) as f64).log10()
    };
    (
        energy(&input, false, 0) - energy(&l, false, lat),
        energy(&input, true, 0) - energy(&l, true, lat),
    )
}
