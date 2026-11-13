//! Where one sound fits on another: cross-correlation by FFT, with two ways
//! of weighing what is compared, and how sure the best fit is.
//!
//! - [`Method::Whitened`]: the sound itself, every frequency given the same
//!   weight (the generalised cross-correlation with phase transform), so a
//!   mic with more bass or a recording that is quieter compares the same.
//! - [`Method::Onsets`]: where sound starts, from rises in loudness a
//!   thousand times a second, which ignores the colour of the sound
//!   altogether but only finds the fit to the millisecond.
//!
//! `docs/research/sync-by-sound.md` compares them; the whitened one is used.

use diag::{AppError, Explain};
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// How two sounds are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The sound itself, each frequency weighed the same.
    Whitened,
    /// Where sound starts: rises in loudness, every millisecond.
    Onsets,
}

/// Where `b` fits best on `a`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// `b[n]` is heard at `a[n + shift]` (in samples of the rate compared).
    pub shift: i64,
    /// The best fit's height over the best one away from it (1 when the
    /// two are as good).
    pub ratio: f32,
    /// The best fit's height over the correlation's typical height (its
    /// root mean square): how far it stands out of the noise.
    pub prominence: f32,
}

/// How a whitened comparison weighs frequencies: only those from `low` to
/// `high` (Hz) count, and each one's strength is divided by itself to the
/// power `power` (1 would make them all the same; a little less keeps
/// strong frequencies, which are less likely noise, a little ahead).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Whitening {
    pub low: f32,
    pub high: f32,
    pub power: f32,
}

/// For finding the rough fit: the band a voice carries in (a mic's
/// rumble and hiss, and a game's booms, are mostly outside it).
pub const SPEECH_BAND: Whitening = Whitening {
    low: 150.0,
    high: 3500.0,
    power: 0.8,
};

/// For the exact fit: the voice's band and the treble above it, which
/// sharpens the peak.
pub const FINE_BAND: Whitening = Whitening {
    low: 150.0,
    high: 8000.0,
    power: 0.8,
};

/// Fits nearer the best than this many milliseconds are part of the same
/// peak (a voice repeats itself every few milliseconds), not a rival.
const SAME_PEAK_MS: i64 = 40;

/// Where `b` fits best on `a` (both at `rate` samples per second), among
/// the shifts that leave at least `min_overlap` samples of the two side by
/// side. `None` when no shift does, or either is silent.
pub fn fit(a: &[f32], b: &[f32], rate: u32, method: Method, min_overlap: usize) -> diag::Result<Option<Fit>> {
    match method {
        Method::Whitened => {
            let c = correlate(a, b, rate, Some(SPEECH_BAND))?;
            Ok(best(
                &c,
                a.len(),
                b.len(),
                min_overlap,
                None,
                ms(rate, SAME_PEAK_MS),
            ))
        }
        Method::Onsets => {
            let per_ms = (rate / 1000).max(1) as usize;
            let (fa, fb) = (onsets(a, per_ms), onsets(b, per_ms));
            let c = correlate(&fa, &fb, 1000, None)?;
            let overlap = min_overlap.div_ceil(per_ms);
            Ok(
                best(&c, fa.len(), fb.len(), overlap, None, SAME_PEAK_MS).map(|f| Fit {
                    shift: f.shift * per_ms as i64,
                    ..f
                }),
            )
        }
    }
}

/// The exact fit of `b` on `a` (both at `rate`), whitened, among shifts
/// `around - reach` to `around + reach` only: for lining up to the sample
/// once a rough fit is known.
pub fn fit_near(a: &[f32], b: &[f32], rate: u32, around: i64, reach: i64) -> diag::Result<Option<Fit>> {
    let c = correlate(a, b, rate, Some(FINE_BAND))?;
    // The neighbourhood is the whole search: the ratio says little here.
    Ok(best(
        &c,
        a.len(),
        b.len(),
        1,
        Some((around - reach, around + reach)),
        0,
    ))
}

/// `c[k]` for every shift `k` from `-(b.len() - 1)` to `a.len() - 1`,
/// found at index `k + b.len() - 1`: the sum of `a[n + k] * b[n]` (both
/// at `rate`), after weighing frequencies by `whiten` when given.
pub fn correlate(a: &[f32], b: &[f32], rate: u32, whiten: Option<Whitening>) -> diag::Result<Vec<f32>> {
    if a.is_empty() || b.is_empty() {
        return Ok(Vec::new());
    }
    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let forward = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let failed =
        || AppError::new("The sounds can't be compared").why("Working out their frequencies failed.");
    let spectrum_of = |x: &[f32]| -> diag::Result<Vec<Complex<f32>>> {
        let mut input = vec![0.0; n];
        input[..x.len()].copy_from_slice(x);
        let mut out = forward.make_output_vec();
        forward.process(&mut input, &mut out).or_explain(failed)?;
        Ok(out)
    };
    let mut product = spectrum_of(a)?;
    let sb = spectrum_of(b)?;
    for (p, q) in product.iter_mut().zip(&sb) {
        *p *= q.conj();
    }
    if let Some(w) = whiten {
        let hz_per_bin = rate as f32 / n as f32;
        for (bin, p) in product.iter_mut().enumerate() {
            let hz = bin as f32 * hz_per_bin;
            let size = p.norm();
            *p = if hz < w.low || hz > w.high || size <= f32::MIN_POSITIVE {
                Complex::new(0.0, 0.0)
            } else {
                *p / size.powf(w.power)
            };
        }
    }
    // The inverse wants the first and last bins real.
    if let Some(first) = product.first_mut() {
        first.im = 0.0;
    }
    if let Some(last) = product.last_mut() {
        last.im = 0.0;
    }
    let mut circular = inverse.make_output_vec();
    inverse.process(&mut product, &mut circular).or_explain(failed)?;
    // Negative shifts are at the end of the circle.
    let (la, lb) = (a.len(), b.len());
    let mut c = Vec::with_capacity(la + lb - 1);
    c.extend_from_slice(&circular[n - (lb - 1)..]);
    c.extend_from_slice(&circular[..la]);
    Ok(c)
}

/// The best shift in `c` (from [`correlate`]) with at least `min_overlap`
/// samples side by side, and within `range` when given; rivals nearer it
/// than `same_peak` don't count.
fn best(
    c: &[f32],
    la: usize,
    lb: usize,
    min_overlap: usize,
    range: Option<(i64, i64)>,
    same_peak: i64,
) -> Option<Fit> {
    let (la, lb) = (la as i64, lb as i64);
    let overlap = |k: i64| (lb.min(la - k) - 0.max(-k)).max(0);
    let lowest = -(lb - 1);
    let mut from = lowest;
    let mut to = la - 1;
    // Overlap grows by one per shift towards the middle: the shifts with
    // enough form one stretch.
    while from <= to && overlap(from) < min_overlap as i64 {
        from += 1;
    }
    while to >= from && overlap(to) < min_overlap as i64 {
        to -= 1;
    }
    if let Some((low, high)) = range {
        from = from.max(low);
        to = to.min(high);
    }
    if from > to {
        return None;
    }
    let at = |k: i64| c[(k - lowest) as usize];
    let (mut shift, mut height) = (from, f32::MIN);
    let mut squares = 0.0f64;
    for k in from..=to {
        let v = at(k);
        squares += f64::from(v) * f64::from(v);
        if v > height {
            (shift, height) = (k, v);
        }
    }
    if height <= 0.0 {
        return None;
    }
    let rms = (squares / (to - from + 1) as f64).sqrt() as f32;
    let rival = (from..=to)
        .filter(|k| (k - shift).abs() > same_peak)
        .map(at)
        .fold(0.0f32, f32::max);
    Some(Fit {
        shift,
        ratio: if rival > 0.0 {
            height / rival
        } else {
            f32::INFINITY
        },
        prominence: if rms > 0.0 { height / rms } else { 0.0 },
    })
}

/// Rises in loudness, one value per `per_ms` samples: the loudness (in
/// decibels, over 10 ms) now less that of 3 ms ago, where it rose, with
/// the average taken away.
pub fn onsets(x: &[f32], per_ms: usize) -> Vec<f32> {
    const WINDOW_MS: usize = 10;
    const BACK: usize = 3;
    const FLOOR_DB: f32 = 40.0;
    let window = WINDOW_MS * per_ms;
    let steps = x.len() / per_ms;
    // Running sum of squares, for the energy of any window.
    let mut sums = Vec::with_capacity(x.len() + 1);
    let mut total = 0.0f64;
    sums.push(0.0);
    for v in x {
        total += f64::from(*v) * f64::from(*v);
        sums.push(total);
    }
    let level: Vec<f32> = (0..steps)
        .map(|j| {
            let end = ((j + 1) * per_ms).min(x.len());
            let start = end.saturating_sub(window);
            let energy = (sums[end] - sums[start]) / (end - start).max(1) as f64;
            (10.0 * (energy + 1e-10).log10()) as f32
        })
        .collect();
    // Quieter than this under the loudest is all the same: room noise.
    let loudest = level.iter().copied().fold(f32::MIN, f32::max);
    let level: Vec<f32> = level.iter().map(|l| l.max(loudest - FLOOR_DB)).collect();
    let mut rises: Vec<f32> = (0..steps)
        .map(|j| {
            if j >= BACK {
                (level[j] - level[j - BACK]).max(0.0)
            } else {
                0.0
            }
        })
        .collect();
    let mean = rises.iter().sum::<f32>() / rises.len().max(1) as f32;
    for r in &mut rises {
        *r -= mean;
    }
    rises
}

/// `millis` milliseconds in samples at `rate`.
fn ms(rate: u32, millis: i64) -> i64 {
    i64::from(rate) * millis / 1000
}
