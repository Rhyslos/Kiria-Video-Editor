//! Noise clean-up by spectral gating: the sound is cut into overlapping
//! windows (1024 samples, a new one every 256), each turned into its
//! frequencies; every frequency that is not clearly above the learned
//! noise there is turned down, smoothly over time and across neighbouring
//! frequencies, and the windows are put back together. Both channels get
//! the same gains so the stereo picture doesn't wander.
//!
//! Latency: `N - HOP` samples (768, 16 ms at 48 kHz).

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const N: usize = 1024;
pub const HOP: usize = 256;
const BINS: usize = N / 2 + 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSettings {
    /// How far above the noise a frequency must be to pass (dB).
    pub threshold_db: f32,
    /// How far down noise goes (dB, ≤ 0).
    pub reduction_db: f32,
    /// How fast a frequency closes again (ms).
    pub release_ms: f32,
}

pub struct SpectralGate {
    pub s: GateSettings,
    rate: f32,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// The learned noise: root mean power per frequency.
    profile: Vec<f32>,
    // Per channel: the last N input samples, the output being added up,
    // the output ready to go out.
    input: [Vec<f32>; 2],
    acc: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    rover: usize,
    // Working space, made once.
    frame: Vec<f32>,
    spec: [Vec<Complex<f32>>; 2],
    scratch_f: Vec<Complex<f32>>,
    scratch_i: Vec<Complex<f32>>,
    gains: Vec<f32>,
    smooth: Vec<f32>,
    floor: f32,
    thr2: f32,
    rel: f32,
    att: f32,
}

impl std::fmt::Debug for SpectralGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpectralGate({:?})", self.s)
    }
}

impl SpectralGate {
    /// Made off the real-time thread: it allocates.
    pub fn new(rate: f32, s: GateSettings, profile: &[f32]) -> SpectralGate {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(N);
        let inv = planner.plan_fft_inverse(N);
        let window = (0..N)
            .map(|i| (std::f32::consts::PI * i as f32 / N as f32).sin()) // sqrt of a periodic Hann
            .collect();
        let scratch_f = fwd.make_scratch_vec();
        let scratch_i = inv.make_scratch_vec();
        let mut g = SpectralGate {
            s,
            rate,
            fwd,
            inv,
            window,
            profile: profile.to_vec(),
            input: [vec![0.0; N], vec![0.0; N]],
            acc: [vec![0.0; N], vec![0.0; N]],
            out: [vec![0.0; N], vec![0.0; N]],
            rover: N - HOP,
            frame: vec![0.0; N],
            spec: [vec![Complex::default(); BINS], vec![Complex::default(); BINS]],
            scratch_f,
            scratch_i,
            gains: vec![1.0; BINS],
            smooth: vec![1.0; BINS],
            floor: 0.0,
            thr2: 0.0,
            rel: 0.0,
            att: 0.0,
        };
        g.derive();
        g
    }

    fn derive(&mut self) {
        self.floor = 10f32.powf(self.s.reduction_db / 20.0);
        self.thr2 = 10f32.powf(self.s.threshold_db / 10.0);
        let hops_per_s = self.rate / HOP as f32;
        self.rel = (-1.0 / (hops_per_s * self.s.release_ms * 0.001)).exp();
        // Opening takes about two windows, so a lone noisy moment doesn't.
        self.att = (-1.0 / (hops_per_s * 0.010)).exp();
    }

    pub fn set(&mut self, s: GateSettings) {
        if s != self.s {
            self.s = s;
            self.derive();
        }
    }

    /// The noise profile from a stretch that is only noise (off the
    /// real-time thread; in the app a worker does this from a chosen
    /// stretch, or from the quietest stretches of the clip).
    pub fn learn(noise: &[f32]) -> Vec<f32> {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(N);
        let mut spec = fwd.make_output_vec();
        let mut sum = vec![0f32; BINS];
        let mut frames = 0;
        let mut frame = vec![0f32; N];
        for start in (0..noise.len().saturating_sub(N)).step_by(HOP) {
            for (i, f) in frame.iter_mut().enumerate() {
                *f = noise[start + i] * (std::f32::consts::PI * i as f32 / N as f32).sin();
            }
            let _ = fwd.process(&mut frame, &mut spec);
            for (s, c) in sum.iter_mut().zip(&spec) {
                *s += c.norm_sqr();
            }
            frames += 1;
        }
        // The root of the mean power: noise goes over its mean magnitude
        // far more often than over its mean power.
        sum.iter_mut()
            .for_each(|s| *s = (*s / frames.max(1) as f32).sqrt());
        sum
    }

    pub fn latency() -> usize {
        N - HOP
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for i in 0..l.len() {
            let at = self.rover;
            self.input[0][at] = l[i];
            self.input[1][at] = r[i];
            l[i] = self.out[0][at - (N - HOP)];
            r[i] = self.out[1][at - (N - HOP)];
            self.rover += 1;
            if self.rover == N {
                self.rover = N - HOP;
                self.hop();
            }
        }
    }

    fn hop(&mut self) {
        for ch in 0..2 {
            for ((f, x), w) in self.frame.iter_mut().zip(&self.input[ch]).zip(&self.window) {
                *f = x * w;
            }
            let _ = self
                .fwd
                .process_with_scratch(&mut self.frame, &mut self.spec[ch], &mut self.scratch_f);
        }
        // Gains: how much of each frequency is above the noise there.
        for k in 0..BINS {
            let m2 = self.spec[0][k].norm_sqr().max(self.spec[1][k].norm_sqr());
            let n2 = self.profile[k] * self.profile[k] * self.thr2;
            let want = if m2 > n2 {
                ((m2 - n2) / m2).sqrt().max(self.floor)
            } else {
                self.floor
            };
            let prev = self.gains[k];
            let c = if want > prev { self.att } else { self.rel };
            self.gains[k] = want + c * (prev - want);
        }
        // Across neighbouring frequencies, so single bins don't twitter.
        self.smooth[0] = self.gains[0];
        self.smooth[BINS - 1] = self.gains[BINS - 1];
        for k in 1..BINS - 1 {
            self.smooth[k] = 0.25 * self.gains[k - 1] + 0.5 * self.gains[k] + 0.25 * self.gains[k + 1];
        }
        let scale = 0.5 / N as f32;
        for ch in 0..2 {
            for (c, g) in self.spec[ch].iter_mut().zip(&self.smooth) {
                *c *= *g;
            }
            self.spec[ch][0].im = 0.0;
            self.spec[ch][BINS - 1].im = 0.0;
            let _ = self
                .inv
                .process_with_scratch(&mut self.spec[ch], &mut self.frame, &mut self.scratch_i);
            let acc = &mut self.acc[ch];
            for ((a, f), w) in acc.iter_mut().zip(&self.frame).zip(&self.window) {
                *a += f * w * scale;
            }
            self.out[ch][..HOP].copy_from_slice(&acc[..HOP]);
            acc.copy_within(HOP.., 0);
            acc[N - HOP..].fill(0.0);
            self.input[ch].copy_within(HOP.., 0);
        }
    }

    /// Carries the running state over from the strip this one replaces.
    pub fn take_state(&mut self, old: &SpectralGate) {
        for ch in 0..2 {
            self.input[ch].copy_from_slice(&old.input[ch]);
            self.acc[ch].copy_from_slice(&old.acc[ch]);
            self.out[ch].copy_from_slice(&old.out[ch]);
        }
        self.gains.copy_from_slice(&old.gains);
        self.rover = old.rover;
    }
}
