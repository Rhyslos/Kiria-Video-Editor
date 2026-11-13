//! Noise clean-up by spectral gating.
//!
//! The sound is cut into overlapping windows (1024 samples, a new one
//! every 256), each turned into its frequencies; every frequency that is
//! not clearly above the learned noise there is turned down, smoothly over
//! time and across neighbouring frequencies, and the windows are put back
//! together. Both channels get the same gains, so the stereo picture never
//! wanders. Hiss and hum go from under words as well as between them.
//!
//! The noise is a [`NoiseProfile`]: levels in 64 bands by frequency,
//! learned on a worker ([`NoiseProfile::learn`], or
//! [`NoiseProfile::learn_quietest`] from a clip's quietest stretches). The
//! gate takes it as [`BinNoise`], worked out for its rate off the
//! real-time thread and swapped in.
//!
//! The gate always delays the sound by [`LATENCY`] samples, on or off, so
//! every track stays in step; off, it lets the windows through untouched
//! (no FFT) once its gains have opened fully, so turning it on or off never
//! clicks.

use super::bands;
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::f32::consts::PI;
use std::sync::Arc;

/// The window: 1024 samples.
pub const WINDOW: usize = 1024;
/// A new window every 256 samples.
pub const HOP: usize = 256;
/// How many frequencies a window has.
pub const BINS: usize = WINDOW / 2 + 1;
/// How late the sound comes out: 1024 samples (21.3 ms at 48 kHz). A
/// sample is only finished once the last of the four windows over it has
/// been put back, a whole window after it went in.
pub const LATENCY: usize = WINDOW;
/// Where new samples go in a window: its last hop.
const FILL_FROM: usize = WINDOW - HOP;
/// Windows over each sample.
const OVERLAP: usize = WINDOW / HOP;

/// The clean-up's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CleanupSettings {
    pub on: bool,
    /// How far above the noise a frequency must be to pass (dB).
    pub threshold_db: f32,
    /// How far down noise goes (dB, 0 or less).
    pub reduction_db: f32,
    /// How fast a frequency closes again after sound in it stops (ms).
    pub release_ms: f32,
}

impl CleanupSettings {
    /// Off, with settings that suit a voice over steady hiss for when it is
    /// turned on.
    pub const OFF: CleanupSettings = CleanupSettings {
        on: false,
        threshold_db: 6.0,
        reduction_db: -18.0,
        release_ms: 120.0,
    };
}

/// A noise profile: the noise's level in each of the 64 bands, in dB per
/// hertz on the gate's own scale. Small, and the same at any rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseProfile(pub [f32; bands::COUNT]);

/// The noise for each of the gate's frequencies at one rate: the magnitude
/// a window of noise alone has there.
pub type BinNoise = [f32; BINS];

/// The analysis window: the square root of a periodic Hann window, used
/// again when putting the windows back, so the two together make a Hann
/// window, which four overlapping windows add up to twice.
fn window() -> Vec<f32> {
    (0..WINDOW)
        .map(|i| (PI * i as f32 / WINDOW as f32).sin())
        .collect()
}

impl NoiseProfile {
    /// No noise: the gate lets everything through.
    pub const SILENT: NoiseProfile = NoiseProfile([-200.0; bands::COUNT]);

    /// Learns the noise from a stretch of stereo sound that is only noise
    /// (at least one window long). Allocates: for a worker.
    pub fn learn(rate: u32, l: &[f32], r: &[f32]) -> NoiseProfile {
        NoiseProfile::learn_from(rate, l, r, 1.0)
    }

    /// Learns the noise from the quietest quarter of the windows of a longer
    /// stretch (a clip where people talk with pauses between): the pauses
    /// are the noise. Allocates: for a worker.
    pub fn learn_quietest(rate: u32, l: &[f32], r: &[f32]) -> NoiseProfile {
        NoiseProfile::learn_from(rate, l, r, 0.25)
    }

    fn learn_from(rate: u32, l: &[f32], r: &[f32], share: f32) -> NoiseProfile {
        let len = l.len().min(r.len());
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let w = window();
        let mut frame = vec![0f32; WINDOW];
        let mut spec = [fft.make_output_vec(), fft.make_output_vec()];
        // Per window: its power, and per bin the louder channel's power.
        let mut windows: Vec<(f32, Vec<f32>)> = Vec::new();
        let mut start = 0;
        while start + WINDOW <= len {
            for (ch, input) in [l, r].into_iter().enumerate() {
                for ((f, x), w) in frame.iter_mut().zip(&input[start..]).zip(&w) {
                    *f = x * w;
                }
                // Lengths are the plan's own, so it can't fail.
                let _ = fft.process(&mut frame, &mut spec[ch]);
            }
            let power: Vec<f32> = spec[0]
                .iter()
                .zip(&spec[1])
                .map(|(a, b)| a.norm_sqr().max(b.norm_sqr()))
                .collect();
            windows.push((power.iter().sum(), power));
            start += HOP;
        }
        if windows.is_empty() {
            return NoiseProfile::SILENT;
        }
        windows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keep = ((windows.len() as f32 * share).ceil() as usize).clamp(1, windows.len());
        // The mean power, not the mean magnitude: noise goes over its mean
        // magnitude far more often than over its mean power.
        let mut mean = vec![0f32; BINS];
        for (_, power) in &windows[..keep] {
            for (m, p) in mean.iter_mut().zip(power) {
                *m += p / keep as f32;
            }
        }
        let bin_hz = rate as f32 / WINDOW as f32;
        // Per hertz, so the same noise reads the same at any rate.
        mean.iter_mut().for_each(|m| *m /= bin_hz);
        let mut levels = [0f32; bands::COUNT];
        bands::from_bins(&mean, bin_hz, &mut levels);
        NoiseProfile(levels.map(|p| 10.0 * (p + 1e-20).log10()))
    }

    /// How loud the noise is in each band, dBFS: the band's share of the
    /// noise's whole level (its mean square against a full-scale square
    /// wave's), so the bands of the noise add up to the noise and none is
    /// louder than all of them together. For drawing the profile; the
    /// stored levels are per hertz on the gate's own scale (a window's
    /// magnitudes, unscaled), which is what the gate compares with and
    /// says nothing on its own.
    pub fn band_dbfs(&self) -> [f32; bands::COUNT] {
        // A window's bin k of a real signal holds 4·|X(k)|²/N² of its mean
        // square (Parseval, with the two window halves' power N/2 and each
        // frequency's mirror image); the levels are |X(k)|² per hertz.
        let share = 10.0 * (4.0 / (WINDOW as f32 * WINDOW as f32)).log10();
        std::array::from_fn(|i| self.0[i] + share + 10.0 * bands::width(i).log10())
    }

    /// The noise at each of the gate's frequencies at `rate`. Allocates:
    /// worked out off the real-time thread and swapped in.
    pub fn bins(&self, rate: u32) -> Box<BinNoise> {
        let bin_hz = rate as f32 / WINDOW as f32;
        let mut out = Box::new([0f32; BINS]);
        for (k, o) in out.iter_mut().enumerate() {
            let db = bands::between(&self.0, (k as f32 * bin_hz).max(bin_hz / 2.0));
            *o = (10f32.powf(db / 10.0) * bin_hz).sqrt();
        }
        out
    }
}

/// The spectral gate. Made off the real-time thread (it allocates);
/// processing never does.
pub struct SpectralGate {
    settings: CleanupSettings,
    rate: f32,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    noise: Box<BinNoise>,
    // Per channel: the last WINDOW input samples, the output being added
    // up, the output ready to go out.
    input: [Vec<f32>; 2],
    sum: [Vec<f32>; 2],
    ready: [Vec<f32>; 2],
    /// Where the next sample goes in `input` (from FILL_FROM to WINDOW).
    at: usize,
    // Working space, made once.
    frame: Vec<f32>,
    spec: [Vec<Complex<f32>>; 2],
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    gains: Vec<f32>,
    smooth: Vec<f32>,
    /// Every gain is fully open and the gate is off: windows go through
    /// without an FFT.
    open: bool,
    /// Windows in a row that went through open (up to [`OVERLAP`]). When
    /// every window over the samples going out did, they go out exactly as
    /// they came in (a plain delay), not as the windows added back up,
    /// which can differ in the last bit: a track with clean-up off sounds
    /// exactly as it would without the clean-up.
    open_run: usize,
    floor: f32,
    threshold: f32,
    release: f32,
    attack: f32,
    /// The lowest gain of the last window (dB), for the page.
    deepest_db: f32,
}

impl std::fmt::Debug for SpectralGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectralGate")
            .field("settings", &self.settings)
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

impl SpectralGate {
    pub fn new(rate: u32, settings: CleanupSettings, noise: Box<BinNoise>) -> SpectralGate {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(WINDOW);
        let inv = planner.plan_fft_inverse(WINDOW);
        let scratch_fwd = fwd.make_scratch_vec();
        let scratch_inv = inv.make_scratch_vec();
        let spec = [fwd.make_output_vec(), fwd.make_output_vec()];
        let mut gate = SpectralGate {
            settings,
            rate: rate as f32,
            fwd,
            inv,
            window: window(),
            noise,
            input: [vec![0.0; WINDOW], vec![0.0; WINDOW]],
            sum: [vec![0.0; WINDOW], vec![0.0; WINDOW]],
            ready: [vec![0.0; HOP], vec![0.0; HOP]],
            at: FILL_FROM,
            frame: vec![0.0; WINDOW],
            spec,
            scratch_fwd,
            scratch_inv,
            gains: vec![1.0; BINS],
            smooth: vec![1.0; BINS],
            open: !settings.on,
            open_run: if settings.on { 0 } else { OVERLAP },
            floor: 0.0,
            threshold: 0.0,
            release: 0.0,
            attack: 0.0,
            deepest_db: 0.0,
        };
        gate.derive();
        gate
    }

    fn derive(&mut self) {
        let s = self.settings;
        self.floor = 10f32.powf(s.reduction_db.min(0.0) / 20.0);
        self.threshold = 10f32.powf(s.threshold_db / 10.0);
        let windows_per_second = self.rate / HOP as f32;
        self.release = super::smoothing(windows_per_second, s.release_ms);
        // Opening takes about two windows, so a lone noisy moment doesn't
        // open it for a whole release.
        self.attack = super::smoothing(windows_per_second, 10.0);
    }

    pub fn settings(&self) -> &CleanupSettings {
        &self.settings
    }

    pub fn set(&mut self, settings: CleanupSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.derive();
            if settings.on {
                self.open = false;
            }
        }
    }

    /// Puts in new noise and hands back the old, to be freed elsewhere.
    pub fn swap_noise(&mut self, noise: &mut Box<BinNoise>) {
        std::mem::swap(&mut self.noise, noise);
    }

    /// How far down the most-reduced frequency was in the last window (dB).
    pub fn reduction_db(&self) -> f32 {
        self.deepest_db
    }

    /// Takes a block in and gives out the sound [`LATENCY`] samples back.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let at = self.at;
            self.input[0][at] = *a;
            self.input[1][at] = *b;
            *a = self.ready[0][at - FILL_FROM];
            *b = self.ready[1][at - FILL_FROM];
            self.at += 1;
            if self.at == WINDOW {
                self.at = FILL_FROM;
                self.window_done();
            }
        }
    }

    /// A window is complete: work out its gains, put it back, move on.
    fn window_done(&mut self) {
        if self.open {
            self.pass_through();
            self.open_run = (self.open_run + 1).min(OVERLAP);
        } else {
            self.gate();
            self.open_run = 0;
        }
        for ch in 0..2 {
            let sum = &mut self.sum[ch];
            if self.open_run == OVERLAP {
                self.ready[ch].copy_from_slice(&self.input[ch][..HOP]);
            } else {
                self.ready[ch].copy_from_slice(&sum[..HOP]);
            }
            sum.copy_within(HOP.., 0);
            sum[FILL_FROM..].fill(0.0);
            self.input[ch].copy_within(HOP.., 0);
        }
    }

    /// Off and fully open: the window is added back as it is (what the
    /// FFT and its inverse give with every gain at one).
    fn pass_through(&mut self) {
        for ch in 0..2 {
            for ((s, x), w) in self.sum[ch].iter_mut().zip(&self.input[ch]).zip(&self.window) {
                *s += x * w * w * 0.5;
            }
        }
        self.deepest_db = 0.0;
    }

    fn gate(&mut self) {
        for ch in 0..2 {
            for ((f, x), w) in self.frame.iter_mut().zip(&self.input[ch]).zip(&self.window) {
                *f = x * w;
            }
            // Lengths are the plan's own, so it can't fail.
            let _ = self
                .fwd
                .process_with_scratch(&mut self.frame, &mut self.spec[ch], &mut self.scratch_fwd);
        }
        let on = self.settings.on;
        let mut all_open = true;
        for k in 0..BINS {
            let want = if on {
                let power = self.spec[0][k].norm_sqr().max(self.spec[1][k].norm_sqr());
                let noise = self.noise[k] * self.noise[k] * self.threshold;
                if power > noise {
                    ((power - noise) / power).sqrt().max(self.floor)
                } else {
                    self.floor
                }
            } else {
                1.0
            };
            let now = self.gains[k];
            let c = if want > now { self.attack } else { self.release };
            let g = want + c * (now - want);
            self.gains[k] = if !on && g > 0.9999 { 1.0 } else { g };
            all_open &= self.gains[k] == 1.0;
        }
        // Across neighbouring frequencies too, so single ones don't twitter.
        self.smooth[0] = self.gains[0];
        self.smooth[BINS - 1] = self.gains[BINS - 1];
        let mut lowest = 1f32;
        for k in 1..BINS - 1 {
            self.smooth[k] = 0.25 * self.gains[k - 1] + 0.5 * self.gains[k] + 0.25 * self.gains[k + 1];
            lowest = lowest.min(self.smooth[k]);
        }
        self.deepest_db = super::to_db(lowest);
        let scale = 0.5 / WINDOW as f32;
        for ch in 0..2 {
            for (c, g) in self.spec[ch].iter_mut().zip(&self.smooth) {
                *c *= *g;
            }
            self.spec[ch][0].im = 0.0;
            self.spec[ch][BINS - 1].im = 0.0;
            let _ = self
                .inv
                .process_with_scratch(&mut self.spec[ch], &mut self.frame, &mut self.scratch_inv);
            for ((s, f), w) in self.sum[ch].iter_mut().zip(&self.frame).zip(&self.window) {
                *s += f * w * scale;
            }
        }
        if !on && all_open {
            self.open = true;
        }
    }

    /// Carries the running state over from the gate this one replaces.
    pub fn take_state(&mut self, old: &SpectralGate) {
        for ch in 0..2 {
            self.input[ch].copy_from_slice(&old.input[ch]);
            self.sum[ch].copy_from_slice(&old.sum[ch]);
            self.ready[ch].copy_from_slice(&old.ready[ch]);
        }
        self.gains.copy_from_slice(&old.gains);
        self.at = old.at;
        self.open = old.open && !self.settings.on;
        self.open_run = if self.open { old.open_run } else { 0 };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_signal::{Rng, speech};

    const RATE: u32 = 48_000;

    fn hiss(n: usize, level: f32, seed: u64) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        (0..n).map(|_| rng.signed() * level).collect()
    }

    fn on() -> CleanupSettings {
        CleanupSettings {
            on: true,
            ..CleanupSettings::OFF
        }
    }

    fn run(gate: &mut SpectralGate, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            gate.process(a, b);
        }
    }

    #[test]
    fn off_it_only_delays() {
        let mut gate = SpectralGate::new(RATE, CleanupSettings::OFF, NoiseProfile::SILENT.bins(RATE));
        let input = hiss(8_192, 0.3, 1);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut gate, &mut l, &mut r);
        // Exactly: a plain delay, to the bit.
        assert_eq!(l[LATENCY..], input[..input.len() - LATENCY]);
        assert!(l[..LATENCY].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn turned_off_while_running_it_becomes_a_plain_delay_once_the_windows_pass() {
        let profile = NoiseProfile([-20.0; crate::dsp::bands::COUNT]);
        let mut gate = SpectralGate::new(RATE, on(), profile.bins(RATE));
        let input = hiss(4_096 + 2 * RATE as usize, 0.3, 1);
        let (mut l, mut r) = (input[..4_096].to_vec(), input[..4_096].to_vec());
        run(&mut gate, &mut l, &mut r);
        gate.set(CleanupSettings::OFF);
        let (mut l, mut r) = (input[4_096..].to_vec(), input[4_096..].to_vec());
        run(&mut gate, &mut l, &mut r);
        // A release and the windows still holding gated sound, then exact.
        let settled = RATE as usize;
        assert_eq!(
            l[settled..],
            input[4_096 + settled - LATENCY..input.len() - LATENCY]
        );
    }

    /// How far (dB) the gate takes pauses and words down, for speech over
    /// hiss at `noise` (peak, linear).
    pub fn pauses_and_words(noise: f32) -> (f32, f32) {
        let n = RATE as usize * 6;
        let (voice, talking) = speech(RATE as f32, n, 7);
        let (hl, hr) = (hiss(n, noise, 2), hiss(n, noise, 3));
        let mut l: Vec<f32> = voice.iter().zip(&hl).map(|(v, h)| v + h).collect();
        let mut r: Vec<f32> = voice.iter().zip(&hr).map(|(v, h)| v * 0.9 + h).collect();
        let (dry_l, dry_r) = (l.clone(), r.clone());
        // Learned from a stretch of the same hiss alone.
        let profile = NoiseProfile::learn(
            RATE,
            &hiss(RATE as usize, noise, 4),
            &hiss(RATE as usize, noise, 5),
        );
        let mut gate = SpectralGate::new(RATE, on(), profile.bins(RATE));
        run(&mut gate, &mut l, &mut r);
        // Energy before and after, well inside pauses and words (a window
        // away from either edge), after the first second.
        let margin = WINDOW;
        let (mut pause, mut word) = ([0f64; 2], [0f64; 2]);
        for i in RATE as usize..n - LATENCY - margin {
            let inside = (i - margin..i + margin)
                .step_by(64)
                .all(|j| talking[j] == talking[i]);
            if !inside {
                continue;
            }
            let into = if talking[i] { &mut word } else { &mut pause };
            let before = f64::from(dry_l[i]).powi(2) + f64::from(dry_r[i]).powi(2);
            let after = f64::from(l[i + LATENCY]).powi(2) + f64::from(r[i + LATENCY]).powi(2);
            into[0] += before;
            into[1] += after;
        }
        let down = |e: [f64; 2]| (10.0 * (e[0] / e[1]).log10()) as f32;
        (down(pause), down(word))
    }

    #[test]
    fn it_takes_hiss_out_of_pauses_and_leaves_words() {
        for noise in [0.003, 0.02] {
            let (pauses, words) = pauses_and_words(noise);
            println!("clean-up, noise {noise}: pauses down {pauses:.1} dB, words down {words:.2} dB");
            assert!(pauses > 12.0, "pauses down only {pauses:.1} dB");
            assert!(words < 1.0, "words down {words:.2} dB");
        }
    }

    /// White noise of a known level, as one channel (a microphone): the
    /// bands drawn add up to that level over the part of the frequencies
    /// they cover, and none is louder than the whole.
    #[test]
    fn a_learned_profile_reads_in_dbfs_and_its_bands_add_up_to_the_noise() {
        let noise = hiss(2 * RATE as usize, 0.05, 9);
        let rms_db = 10.0 * (noise.iter().map(|s| s * s).sum::<f32>() / noise.len() as f32).log10();
        let profile = NoiseProfile::learn(RATE, &noise, &noise);
        let bands = profile.band_dbfs();
        let total = 10.0 * bands.iter().map(|b| 10f32.powf(b / 10.0)).sum::<f32>().log10();
        // The bands cover 20 Hz to 20 kHz of the 24 kHz there is.
        let covered = rms_db + 10.0 * ((bands::HIGHEST - bands::LOWEST) / (RATE as f32 / 2.0)).log10();
        assert!(
            (total - covered).abs() < 0.5,
            "bands add to {total:.2}, noise {covered:.2} dBFS"
        );
        assert!(bands.iter().all(|b| *b < rms_db), "{bands:?} against {rms_db}");
        // White noise has the same level per hertz: a band twice as wide
        // reads 3 dB louder.
        let (lo, hi) = (40, 52);
        let wider = 10.0 * (bands::width(hi) / bands::width(lo)).log10();
        assert!(
            (bands[hi] - bands[lo] - wider).abs() < 1.5,
            "{} {}",
            bands[lo],
            bands[hi]
        );
    }

    #[test]
    fn a_learned_profile_reads_the_same_at_another_rate() {
        let at48 = NoiseProfile::learn(48_000, &hiss(48_000, 0.1, 1), &hiss(48_000, 0.1, 2));
        let at96 = NoiseProfile::learn(96_000, &hiss(96_000, 0.1, 1), &hiss(96_000, 0.1, 2));
        // White noise at the same sample level spreads over twice the
        // bandwidth at twice the rate: 3 dB less per hertz.
        for b in 30..60 {
            let d = at48.0[b] - at96.0[b];
            assert!((d - 3.0).abs() < 1.5, "band {b}: {d}");
        }
        // And the bins worked out again match what a window really holds.
        let bins = at48.bins(48_000);
        let quiet = NoiseProfile::learn_quietest(48_000, &hiss(48_000, 0.1, 3), &hiss(48_000, 0.1, 4));
        assert!(quiet.0[40] <= at48.0[40] + 0.5);
        assert!(bins[200] > 0.0);
    }

    #[test]
    fn turning_it_off_opens_smoothly_then_lets_through() {
        let profile = NoiseProfile::learn(RATE, &hiss(48_000, 0.05, 1), &hiss(48_000, 0.05, 2));
        let mut gate = SpectralGate::new(RATE, on(), profile.bins(RATE));
        let mut l = hiss(24_000, 0.05, 3);
        let mut r = hiss(24_000, 0.05, 4);
        run(&mut gate, &mut l, &mut r);
        assert!(gate.reduction_db() < -10.0);
        gate.set(CleanupSettings::OFF);
        let mut l = hiss(24_000, 0.05, 5);
        let mut r = l.clone();
        let input = l.clone();
        run(&mut gate, &mut l, &mut r);
        assert!(gate.open, "fully open again");
        let tail = 20_000;
        assert!((l[tail] - input[tail - LATENCY]).abs() < 1e-5);
        // Its output never jumps: the step from block to block stays small.
        let step = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(step < 0.2, "{step}");
    }
}
