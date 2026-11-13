//! Loudness and true peak, measured as the international broadcast
//! loudness recommendation (ITU-R BS.1770-4) and the European broadcast
//! loudness rule built on it, with its meter test signals, say:
//!
//! - **K-weighting:** each channel goes through two filters first, a high
//!   shelf (about +4 dB above 2 kHz, the head's effect) and a high pass
//!   (below about 40 Hz counts for little), so loudness follows what is
//!   heard rather than the signal's size.
//! - **Blocks:** the weighted signal's mean square, summed over the
//!   channels (left and right count the same), is kept per 100 ms. A
//!   400 ms block (four of them, a new one every 100 ms) is the
//!   *momentary* loudness, 3 s (thirty) the *short-term*. Loudness is
//!   −0.691 + 10·log10 of a block's mean square, in LUFS.
//! - **Integrated** loudness (the whole programme) is gated twice: 400 ms
//!   blocks under −70 LUFS are left out (silence), then those more than
//!   10 LU under the loudness of what is left; what remains is averaged
//!   as energy. The blocks are counted in a histogram of 0.01 LU steps,
//!   so a programme of any length takes the same memory and nothing is
//!   allocated while measuring; the gate then lands within 0.01 LU.
//! - **True peak:** the highest level of the signal *between* samples as
//!   well, which a sample peak misses and a converter or encoder can
//!   produce: each channel is oversampled four times (a windowed-sinc
//!   interpolator in four phases, the first of which is the samples
//!   themselves) and the highest absolute value taken, in dBTP.
//!
//! Everything here is plain arithmetic on what is fed in: the master's
//! meter and an export both measure with it. (This is `engine::loudness`
//! moved here; the engine's copy becomes a re-export of it when the
//! engine mixes through this crate, so there is one meter.)

/// A two-pole filter section.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for `rate` samples per second: a high shelf
/// (the head's effect) and a high pass. The recommendation gives the
/// coefficients for 48 kHz; these are the same filters worked out for any
/// rate from their frequency, gain and Q.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    [shelf, pass]
}

/// 100 ms blocks kept: enough for the 3 s short-term window.
const BLOCKS: usize = 30;
/// 100 ms blocks in a momentary (gating) block.
const MOMENTARY: usize = 4;

/// Loudness in LUFS of a mean square (summed over channels).
fn lufs(mean_square: f64) -> f32 {
    if mean_square <= 0.0 {
        f32::NEG_INFINITY
    } else {
        (-0.691 + 10.0 * mean_square.log10()) as f32
    }
}

/// Decibels of a linear level; silence is `-inf`.
pub fn db(level: f32) -> f32 {
    if level > 0.0 {
        20.0 * level.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// The two gates of integrated loudness over every 400 ms block so far.
#[derive(Debug, Clone)]
struct Gate {
    /// Per 0.01 LU from [`Gate::FLOOR`]: how many blocks, and their
    /// summed mean squares.
    counts: Vec<u64>,
    energy: Vec<f64>,
}

impl Gate {
    /// The absolute gate and the histogram's floor, in LUFS.
    const FLOOR: f64 = -70.0;
    /// Its top: louder blocks go in the last step (they are counted
    /// exactly all the same, by their energy).
    const CEILING: f64 = 10.0;
    const STEP: f64 = 0.01;
    /// Below the absolute-gated loudness, the relative gate, in LU.
    const RELATIVE: f64 = 10.0;

    fn new() -> Gate {
        let steps = ((Gate::CEILING - Gate::FLOOR) / Gate::STEP) as usize + 1;
        Gate {
            counts: vec![0; steps],
            energy: vec![0.0; steps],
        }
    }

    fn clear(&mut self) {
        self.counts.fill(0);
        self.energy.fill(0.0);
    }

    fn step_of(&self, lufs: f64) -> usize {
        (((lufs - Gate::FLOOR) / Gate::STEP).floor().max(0.0) as usize).min(self.counts.len() - 1)
    }

    /// Counts a 400 ms block's mean square; silence (the absolute gate) is
    /// left out.
    fn add(&mut self, mean_square: f64) {
        let loudness = f64::from(lufs(mean_square));
        if loudness <= Gate::FLOOR {
            return;
        }
        let i = self.step_of(loudness);
        self.counts[i] += 1;
        self.energy[i] += mean_square;
    }

    /// The integrated loudness of the blocks counted, `-inf` before any.
    fn integrated(&self) -> f32 {
        let mean = |from: usize| {
            let n: u64 = self.counts[from..].iter().sum();
            (n > 0).then(|| self.energy[from..].iter().sum::<f64>() / n as f64)
        };
        let Some(above_silence) = mean(0) else {
            return f32::NEG_INFINITY;
        };
        let relative = f64::from(lufs(above_silence)) - Gate::RELATIVE;
        // Blocks strictly above the relative gate: from the step it is in,
        // that step only when the gate is at its bottom.
        let from = self.step_of(relative);
        let exact = Gate::FLOOR + from as f64 * Gate::STEP;
        let from = if relative > exact + 1e-9 { from + 1 } else { from };
        mean(from.min(self.counts.len())).map_or(f32::NEG_INFINITY, lufs)
    }
}

/// The loudness range (LRA, as the European broadcast rule's loudness
/// range supplement says): how far apart the quieter and the louder
/// stretches of a programme are, from its 3 s (short-term) loudness taken
/// every 100 ms. Readings under -70 LUFS are left out (silence), then
/// those more than 20 LU under the loudness of what is left (energy
/// mean); the range is the 95th percentile less the 10th of what remains,
/// in LU. Counted in a histogram of 0.01 LU steps, like [`Gate`], so it
/// takes the same memory however long it runs.
#[derive(Debug, Clone)]
struct Range {
    counts: Vec<u64>,
    energy: Vec<f64>,
}

impl Range {
    /// Below the absolute-gated loudness, the relative gate, in LU.
    const RELATIVE: f64 = 20.0;
    /// The percentiles whose distance is the range.
    const LOW: f64 = 0.10;
    const HIGH: f64 = 0.95;

    fn new() -> Range {
        let gate = Gate::new();
        Range {
            counts: gate.counts,
            energy: gate.energy,
        }
    }

    fn clear(&mut self) {
        self.counts.fill(0);
        self.energy.fill(0.0);
    }

    fn step_of(&self, lufs: f64) -> usize {
        (((lufs - Gate::FLOOR) / Gate::STEP).floor().max(0.0) as usize).min(self.counts.len() - 1)
    }

    /// Counts one short-term reading's mean square.
    fn add(&mut self, mean_square: f64) {
        let loudness = f64::from(lufs(mean_square));
        if loudness <= Gate::FLOOR {
            return;
        }
        let i = self.step_of(loudness);
        self.counts[i] += 1;
        self.energy[i] += mean_square;
    }

    /// The range in LU, `-inf` before anything was counted.
    fn range(&self) -> f32 {
        let n: u64 = self.counts.iter().sum();
        if n == 0 {
            return f32::NEG_INFINITY;
        }
        let mean = self.energy.iter().sum::<f64>() / n as f64;
        let relative = f64::from(lufs(mean)) - Range::RELATIVE;
        let from = if relative <= Gate::FLOOR {
            0
        } else {
            self.step_of(relative) + 1
        };
        let kept = &self.counts[from.min(self.counts.len())..];
        let total: u64 = kept.iter().sum();
        if total == 0 {
            return 0.0;
        }
        // The step holding the reading at a share of those kept (nearest rank).
        let at = |share: f64| {
            let rank = ((share * total as f64).ceil() as u64).clamp(1, total);
            let mut seen = 0;
            for (i, count) in kept.iter().enumerate() {
                seen += count;
                if seen >= rank {
                    return Gate::FLOOR + (from + i) as f64 * Gate::STEP;
                }
            }
            Gate::FLOOR + (from + kept.len()) as f64 * Gate::STEP
        };
        (at(Range::HIGH) - at(Range::LOW)).max(0.0) as f32
    }
}

/// What a meter counts over a while from readings taken every 100 ms of
/// sound (its momentary and short-term loudness, its highest true or
/// sample peak): integrated loudness, loudness range and the highest
/// peak. For counting across several plays, or only inside a region,
/// where the meter itself starts again each time. A reading's momentary
/// loudness is the 400 ms block the integrated loudness gates, so taken
/// every 100 ms it counts as the meter does. Allocates only when made.
#[derive(Debug, Clone)]
pub struct Tally {
    gate: Gate,
    range: Range,
    peak: f32,
    readings: u64,
}

impl Default for Tally {
    fn default() -> Tally {
        Tally::new()
    }
}

/// The mean square of a loudness, the inverse of [`lufs`].
fn mean_square_of(loudness: f32) -> f64 {
    if loudness.is_finite() {
        10f64.powf((f64::from(loudness) + 0.691) / 10.0)
    } else {
        0.0
    }
}

impl Tally {
    pub fn new() -> Tally {
        Tally {
            gate: Gate::new(),
            range: Range::new(),
            peak: f32::NEG_INFINITY,
            readings: 0,
        }
    }

    /// Forgets everything counted.
    pub fn clear(&mut self) {
        self.gate.clear();
        self.range.clear();
        self.peak = f32::NEG_INFINITY;
        self.readings = 0;
    }

    /// Counts one reading: the momentary and short-term loudness (LUFS)
    /// and the highest peak since the last reading (dB). Silence is `-inf`.
    pub fn add(&mut self, momentary: f32, short_term: f32, peak_db: f32) {
        self.readings += 1;
        if momentary.is_finite() {
            self.gate.add(mean_square_of(momentary));
        }
        if short_term.is_finite() {
            self.range.add(mean_square_of(short_term));
        }
        self.peak = self.peak.max(peak_db);
    }

    /// How many readings were counted (each stands for 100 ms).
    pub fn readings(&self) -> u64 {
        self.readings
    }

    /// The integrated loudness of what was counted, LUFS (`-inf`: none).
    pub fn integrated(&self) -> f32 {
        self.gate.integrated()
    }

    /// The loudness range of what was counted, LU (`-inf`: none).
    pub fn range(&self) -> f32 {
        self.range.range()
    }

    /// The highest peak counted, dB (`-inf`: none).
    pub fn peak(&self) -> f32 {
        self.peak
    }
}

/// Taps of each of the interpolator's phases.
pub const TAPS: usize = 16;
/// How many times over a channel is sampled for its true peak.
pub const OVERSAMPLE: usize = 4;

/// The interpolator for the true peak: a windowed sinc cut off at the
/// original rate's half, `OVERSAMPLE` phases of `TAPS` taps. Phase 0 is
/// the samples themselves (its sinc is zero at every other sample).
pub fn interpolator() -> [[f32; TAPS]; OVERSAMPLE] {
    // The whole filter has OVERSAMPLE * TAPS + 1 taps around its centre.
    let len = OVERSAMPLE * TAPS + 1;
    let centre = (len / 2) as f64;
    // A Kaiser window: little ripple in the pass band, so a full-scale
    // tone reads full scale.
    let beta = 7.0;
    let bessel = |x: f64| {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..30 {
            term *= (x / 2.0) / k as f64;
            sum += term * term;
        }
        sum
    };
    let mut phases = [[0f32; TAPS]; OVERSAMPLE];
    for (p, phase) in phases.iter_mut().enumerate() {
        let mut taps = [0f64; TAPS];
        for (j, tap) in taps.iter_mut().enumerate() {
            let k = j * OVERSAMPLE + p;
            let t = (k as f64 - centre) / OVERSAMPLE as f64;
            let sinc = if t == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
            };
            let r = (k as f64 - centre) / centre;
            let window = bessel(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel(beta);
            *tap = sinc * window;
        }
        // Each phase passes a steady level as it is.
        let sum: f64 = taps.iter().sum();
        for (out, tap) in phase.iter_mut().zip(taps) {
            *out = (tap / sum) as f32;
        }
    }
    phases
}

/// The true peak of one channel: its last samples and the highest level
/// found between and on them.
#[derive(Debug, Clone)]
pub struct TruePeak {
    history: [f32; TAPS],
    /// Where the next sample goes in `history`.
    next: usize,
}

impl Default for TruePeak {
    fn default() -> TruePeak {
        TruePeak::new()
    }
}

impl TruePeak {
    pub fn new() -> TruePeak {
        TruePeak {
            history: [0.0; TAPS],
            next: 0,
        }
    }

    /// Takes in a sample; returns the highest absolute level of the
    /// oversampled signal it ends.
    pub fn feed(&mut self, sample: f32, phases: &[[f32; TAPS]; OVERSAMPLE]) -> f32 {
        self.history[self.next] = sample;
        self.next = (self.next + 1) % TAPS;
        let mut peak = 0f32;
        for phase in phases {
            // Tap j meets the sample j steps back, newest first.
            let mut sum = 0f32;
            for (j, tap) in phase.iter().enumerate() {
                let i = (self.next + TAPS - 1 - j) % TAPS;
                sum += tap * self.history[i];
            }
            peak = peak.max(sum.abs());
        }
        peak
    }
}

/// What has been measured, as the meter shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// The highest sample of each channel in the last block fed, dBFS.
    pub peak_db: [f32; 2],
    /// The highest true peak of each channel in the last block fed, dBTP.
    pub true_peak_db: [f32; 2],
    /// The highest true peak of either channel since the start, dBTP.
    pub true_peak_max: f32,
    /// Loudness over the last 0.4 s and 3 s, LUFS.
    pub momentary: f32,
    pub short_term: f32,
    /// Gated loudness of everything since the start, LUFS.
    pub integrated: f32,
}

impl Measured {
    pub const SILENT: Measured = Measured {
        peak_db: [f32::NEG_INFINITY; 2],
        true_peak_db: [f32::NEG_INFINITY; 2],
        true_peak_max: f32::NEG_INFINITY,
        momentary: f32::NEG_INFINITY,
        short_term: f32::NEG_INFINITY,
        integrated: f32::NEG_INFINITY,
    };
}

/// Measures interleaved stereo as it is fed in. Allocates only when made.
#[derive(Debug, Clone)]
pub struct Loudness {
    filters: [[Biquad; 2]; 2],
    block_len: usize,
    /// Weighted energy and samples of the 100 ms block being filled.
    filling: (f64, usize),
    /// Mean squares of the finished 100 ms blocks, a ring of the last
    /// [`BLOCKS`], `done` of them so far.
    blocks: [f64; BLOCKS],
    done: usize,
    /// `done` when the integrated loudness last started: only 400 ms
    /// blocks wholly after it count.
    gated_from: usize,
    gate: Gate,
    /// The loudness range, from the short-term loudness of every full 3 s.
    range: Range,
    phases: [[f32; TAPS]; OVERSAMPLE],
    true_peaks: [TruePeak; 2],
    true_peak_max: f32,
    measured: Measured,
}

impl Loudness {
    pub fn new(rate: u32) -> Loudness {
        let k = k_weighting(f64::from(rate));
        Loudness {
            filters: [k, k],
            block_len: (rate / 10).max(1) as usize,
            filling: (0.0, 0),
            blocks: [0.0; BLOCKS],
            done: 0,
            gated_from: 0,
            gate: Gate::new(),
            range: Range::new(),
            phases: interpolator(),
            true_peaks: [TruePeak::new(), TruePeak::new()],
            true_peak_max: 0.0,
            measured: Measured::SILENT,
        }
    }

    /// Starts the integrated loudness and the highest true peak again
    /// (momentary and short-term go on: they only look back a while).
    pub fn reset(&mut self) {
        self.gate.clear();
        self.range.clear();
        self.gated_from = self.done;
        // What was before plays no part in the peaks from here.
        self.true_peaks = [TruePeak::new(), TruePeak::new()];
        self.true_peak_max = 0.0;
        self.measured.integrated = f32::NEG_INFINITY;
        self.measured.true_peak_max = f32::NEG_INFINITY;
    }

    /// Mean square of the last `n` finished 100 ms blocks, once there are
    /// that many.
    fn last(&self, n: usize) -> Option<f64> {
        if self.done < n {
            return None;
        }
        let sum: f64 = (0..n)
            .map(|back| self.blocks[(self.done - 1 - back) % BLOCKS])
            .sum();
        Some(sum / n as f64)
    }

    /// Takes in one block (interleaved left, right) and works out the new
    /// values.
    pub fn feed(&mut self, samples: &[f32]) -> Measured {
        let mut peak = [0f32; 2];
        let mut true_peak = [0f32; 2];
        for frame in samples.chunks_exact(2) {
            let mut energy = 0.0;
            for (ch, s) in frame.iter().enumerate() {
                peak[ch] = peak[ch].max(s.abs());
                true_peak[ch] = true_peak[ch].max(self.true_peaks[ch].feed(*s, &self.phases));
                let [a, b] = &mut self.filters[ch];
                let y = b.run(a.run(f64::from(*s)));
                energy += y * y;
            }
            self.filling.0 += energy;
            self.filling.1 += 1;
            if self.filling.1 == self.block_len {
                self.blocks[self.done % BLOCKS] = self.filling.0 / self.block_len as f64;
                self.done += 1;
                self.filling = (0.0, 0);
                if self.done - self.gated_from >= MOMENTARY
                    && let Some(block) = self.last(MOMENTARY)
                {
                    self.gate.add(block);
                    self.measured.integrated = self.gate.integrated();
                }
                if self.done - self.gated_from >= BLOCKS
                    && let Some(short) = self.last(BLOCKS)
                {
                    self.range.add(short);
                }
            }
        }
        self.true_peak_max = self.true_peak_max.max(true_peak[0]).max(true_peak[1]);
        // Short-term: 3 s, or what there is of it once there is 0.4 s.
        let short = (self.done >= MOMENTARY)
            .then(|| self.last(self.done.min(BLOCKS)))
            .flatten();
        self.measured = Measured {
            peak_db: [db(peak[0]), db(peak[1])],
            true_peak_db: [db(true_peak[0]), db(true_peak[1])],
            true_peak_max: db(self.true_peak_max),
            momentary: self.last(MOMENTARY).map_or(f32::NEG_INFINITY, lufs),
            short_term: short.map_or(f32::NEG_INFINITY, lufs),
            integrated: self.measured.integrated,
        };
        self.measured
    }

    /// What was measured last.
    pub fn measured(&self) -> Measured {
        self.measured
    }

    /// The loudness range since the start (or the last reset), LU, from
    /// the short-term loudness of every full 3 s; `-inf` before the first.
    /// Worked out when asked (a walk through its histogram), not per block.
    pub fn range(&self) -> f32 {
        self.range.range()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a sine at `freq` with peak `amplitude` (linear) and
    /// starting `phase` (radians), in the channels `on` (left, right).
    fn sine(rate: u32, freq: f64, amplitude: f64, phase: f64, seconds: f64, on: [bool; 2]) -> Vec<f32> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (amplitude
                    * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate) + phase).sin())
                    as f32;
                [if on[0] { v } else { 0.0 }, if on[1] { v } else { 0.0 }]
            })
            .collect()
    }

    fn of_db(db: f64) -> f64 {
        10f64.powf(db / 20.0)
    }

    fn measure(rate: u32, samples: &[f32]) -> Measured {
        let mut meter = Loudness::new(rate);
        for block in samples.chunks(1024) {
            meter.feed(block);
        }
        meter.measured()
    }

    fn near(value: f32, wanted: f32, within: f32) -> bool {
        (value - wanted).abs() <= within
    }

    #[test]
    fn the_reference_tones_read_as_the_standard_says() {
        for rate in [44_100, 48_000, 96_000] {
            // 1 kHz at -23 dBFS in both channels reads -23 LUFS: momentary,
            // short-term and integrated (the meter test's first case,
            // within 0.1 LU).
            let both = measure(rate, &sine(rate, 1_000.0, of_db(-23.0), 0.0, 20.0, [true; 2]));
            assert!(near(both.momentary, -23.0, 0.1), "{rate}: {both:?}");
            assert!(near(both.short_term, -23.0, 0.1), "{rate}: {both:?}");
            assert!(near(both.integrated, -23.0, 0.1), "{rate}: {both:?}");
            // At 0 dBFS in one channel alone: -3.01 LUFS (the calibration).
            let one = measure(rate, &sine(rate, 1_000.0, 1.0, 0.0, 5.0, [true, false]));
            assert!(near(one.integrated, -3.01, 0.1), "{rate}: {one:?}");
            // And -20 dBFS in one channel: about -23 LUFS.
            let quiet = measure(rate, &sine(rate, 1_000.0, 0.1, 0.0, 5.0, [true, false]));
            assert!(near(quiet.integrated, -23.0, 0.1), "{rate}: {quiet:?}");
        }
    }

    #[test]
    fn the_integrated_loudness_is_gated() {
        let rate = 48_000;
        // -36 dBFS, -23 dBFS, -36 dBFS for 10, 60 and 10 s: the quiet
        // parts fall under the relative gate, so -23 LUFS (the meter
        // test's third case, within 0.1 LU).
        let mut tones = sine(rate, 1_000.0, of_db(-36.0), 0.0, 10.0, [true; 2]);
        tones.extend(sine(rate, 1_000.0, of_db(-23.0), 0.0, 60.0, [true; 2]));
        tones.extend(sine(rate, 1_000.0, of_db(-36.0), 0.0, 10.0, [true; 2]));
        assert!(near(measure(rate, &tones).integrated, -23.0, 0.1));
        // Silence around a tone changes nothing (the absolute gate).
        let mut quiet = vec![0f32; 2 * rate as usize * 20];
        quiet.extend(sine(rate, 1_000.0, of_db(-23.0), 0.0, 20.0, [true; 2]));
        quiet.extend(vec![0f32; 2 * rate as usize * 20]);
        let m = measure(rate, &quiet);
        assert!(near(m.integrated, -23.0, 0.1), "{m:?}");
        assert_eq!(m.momentary, f32::NEG_INFINITY, "the last 0.4 s were silent");
        // Silence alone has none.
        assert_eq!(measure(rate, &vec![0.0; 96_000]).integrated, f32::NEG_INFINITY);
    }

    #[test]
    fn the_true_peak_finds_what_is_between_the_samples() {
        // A quarter of the sampling rate, peak -6 dBFS: in phase every
        // other sample is a peak; at 45° no sample is (each is 3 dB
        // under), yet the signal still reaches -6 dBTP. Likewise a sixth
        // at 60° and an eighth at 67.5°. The meter test allows +0.2 and
        // -0.4 dB. (Read once the tone is steady: its sudden start rings,
        // as it would through any converter.)
        let rate = 48_000;
        for (freq, phase_deg) in [
            (12_000.0, 0.0),
            (12_000.0, 45.0),
            (8_000.0, 60.0),
            (6_000.0, 67.5),
        ] {
            let tone = sine(
                rate,
                freq,
                of_db(-6.0),
                f64::to_radians(phase_deg),
                2.0,
                [true; 2],
            );
            let m = measure(rate, &tone);
            assert!(
                (-6.4..=-5.8).contains(&m.true_peak_db[0]),
                "{freq} Hz at {phase_deg}°: {m:?}"
            );
            assert!(
                m.true_peak_db[0] >= m.peak_db[0] - 0.01,
                "never under the sample peak"
            );
            assert!(m.true_peak_max >= m.true_peak_db[0]);
        }
        let between = measure(
            rate,
            &sine(rate, 12_000.0, of_db(-6.0), f64::to_radians(45.0), 1.0, [true; 2]),
        );
        assert!(
            near(between.peak_db[0], -9.01, 0.05),
            "the samples alone miss 3 dB: {between:?}"
        );
    }

    /// Two tones of 20 s each at `a` and `b` dBFS in both channels.
    fn two_tones(rate: u32, a: f64, b: f64) -> Vec<f32> {
        let mut tones = sine(rate, 1_000.0, of_db(a), 0.0, 20.0, [true; 2]);
        tones.extend(sine(rate, 1_000.0, of_db(b), 0.0, 20.0, [true; 2]));
        tones
    }

    #[test]
    fn the_loudness_range_reads_as_its_test_signals_say() {
        // The range supplement's first three test signals: 20 s of 1 kHz at
        // -20 then -30, -15 or -40 dBFS read 10, 5 and 20 LU (within 1 LU).
        let rate = 48_000;
        for (a, b, range) in [(-20.0, -30.0, 10.0), (-20.0, -15.0, 5.0), (-40.0, -20.0, 20.0)] {
            let mut meter = Loudness::new(rate);
            for block in two_tones(rate, a, b).chunks(1024) {
                meter.feed(block);
            }
            assert!(near(meter.range(), range, 1.0), "{a} then {b}: {}", meter.range());
        }
        // One steady tone has no range; silence has none to tell.
        let mut steady = Loudness::new(rate);
        steady.feed(&sine(rate, 1_000.0, of_db(-23.0), 0.0, 10.0, [true; 2]));
        assert!(near(steady.range(), 0.0, 0.05), "{}", steady.range());
        let mut silent = Loudness::new(rate);
        silent.feed(&vec![0.0; 2 * 48_000 * 5]);
        assert_eq!(silent.range(), f32::NEG_INFINITY);
        steady.reset();
        assert_eq!(steady.range(), f32::NEG_INFINITY, "a reset starts it again");
    }

    #[test]
    fn a_tally_of_readings_every_100_ms_counts_as_the_meter_does() {
        // Readings taken off the meter every 100 ms of sound, as the
        // Audio page takes them while playing, give the meter's own
        // integrated loudness and range, and the highest peak.
        let rate = 48_000;
        let tones = two_tones(rate, -20.0, -30.0);
        let mut meter = Loudness::new(rate);
        let mut tally = Tally::new();
        for block in tones.chunks(2 * 4_800) {
            let m = meter.feed(block);
            tally.add(
                m.momentary,
                m.short_term,
                m.true_peak_db[0].max(m.true_peak_db[1]),
            );
        }
        let m = meter.measured();
        assert_eq!(tally.readings(), 400);
        assert!(
            near(tally.integrated(), m.integrated, 0.05),
            "{} {}",
            tally.integrated(),
            m.integrated
        );
        assert!(
            near(tally.range(), meter.range(), 0.3),
            "{} {}",
            tally.range(),
            meter.range()
        );
        assert!(near(tally.peak(), m.true_peak_max, 0.01));
        // Split over two plays, it adds them up; cleared, it has nothing.
        let mut twice = Tally::new();
        for _ in 0..2 {
            let mut meter = Loudness::new(rate);
            for block in tones[..tones.len() / 2].chunks(2 * 4_800) {
                let m = meter.feed(block);
                twice.add(m.momentary, m.short_term, m.true_peak_max);
            }
        }
        assert!(near(twice.integrated(), -20.0, 0.1), "{}", twice.integrated());
        twice.clear();
        assert_eq!(
            (twice.integrated(), twice.range(), twice.peak(), twice.readings()),
            (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY, 0)
        );
    }

    #[test]
    fn a_reset_starts_the_integrated_loudness_and_the_peak_again() {
        let rate = 48_000;
        let mut meter = Loudness::new(rate);
        meter.feed(&sine(rate, 1_000.0, of_db(-10.0), 0.0, 5.0, [true; 2]));
        assert!(near(meter.measured().integrated, -10.0, 0.1));
        meter.reset();
        assert_eq!(meter.measured().integrated, f32::NEG_INFINITY);
        let m = meter.feed(&sine(rate, 1_000.0, of_db(-30.0), 0.0, 5.0, [true; 2]));
        assert!(near(m.integrated, -30.0, 0.1), "{m:?}");
        assert!(near(m.true_peak_max, -30.0, 0.2), "{m:?}");
    }
}
