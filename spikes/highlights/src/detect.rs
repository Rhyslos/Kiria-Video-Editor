//! The approaches from docs/research/highlights.md, run on the features:
//!
//! - A, loudness: each audio stream on its own, a moment where the sound
//!   over the last 200 ms is much louder than the stream usually is.
//! - B, flux: sudden new sound (spectral flux) in each stream, measured
//!   against how busy that stream usually is, then the streams combined
//!   with the mic counting most.
//! - C, video: cuts (colours change at once, or the whole picture changes
//!   in one frame only) and sudden motion (the picture changing much more
//!   than it usually does, for a moment).
//!
//! Every threshold has three settings, the sensitivity.

use crate::audio::Features;
use crate::video::Change;

/// What an audio stream carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Game,
    Mic,
    Chat,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Game => "game",
            Role::Mic => "mic",
            Role::Chat => "chat",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text.trim().to_ascii_lowercase().as_str() {
            "game" => Some(Role::Game),
            "mic" => Some(Role::Mic),
            "chat" => Some(Role::Chat),
            _ => None,
        }
    }

    /// How much a burst on this stream counts in approach B.
    fn weight(self) -> f32 {
        match self {
            Role::Mic => 1.0,
            Role::Game => 0.6,
            Role::Chat => 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensitivity {
    Low,
    Medium,
    High,
}

impl Sensitivity {
    pub const ALL: [Sensitivity; 3] = [Sensitivity::Low, Sensitivity::Medium, Sensitivity::High];

    pub fn name(self) -> &'static str {
        match self {
            Sensitivity::Low => "low",
            Sensitivity::Medium => "medium",
            Sensitivity::High => "high",
        }
    }

    /// Picks one of three values: low, medium, high.
    fn pick(self, low: f32, medium: f32, high: f32) -> f32 {
        match self {
            Sensitivity::Low => low,
            Sensitivity::Medium => medium,
            Sensitivity::High => high,
        }
    }
}

/// A suggested label.
#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub time: f64,
    /// How far over its threshold it was; bigger is surer. Units differ by
    /// detector, so only compare within one.
    pub score: f32,
    /// What found it: `mic`, `game`, `chat`, `cut`, `motion`, joined by `+`.
    pub sources: String,
}

fn db(power: f32) -> f32 {
    10.0 * (power + 1e-12).log10()
}

/// The `q` quantile (0..1) of `values`.
pub fn quantile(values: &[f32], q: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    let k = ((v.len() - 1) as f32 * q).round() as usize;
    let (_, x, _) = v.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    *x
}

/// For each value of a series with `step` seconds between values, the `q`
/// quantile of the values within `half` seconds either side. Worked out
/// once a second and held in between, so a long recording stays cheap.
fn local_quantile(values: &[f32], step: f64, half: f64, q: f32) -> Vec<f32> {
    let per_second = (1.0 / step).round().max(1.0) as usize;
    let reach = (half / step).round() as usize;
    let mut out = Vec::with_capacity(values.len());
    let mut at = 0;
    while at < values.len() {
        let centre = (at + per_second / 2).min(values.len() - 1);
        let lo = centre.saturating_sub(reach);
        let hi = (centre + reach + 1).min(values.len());
        let level = quantile(&values[lo..hi], q);
        let end = (at + per_second).min(values.len());
        out.extend(std::iter::repeat_n(level, end - at));
        at = end;
    }
    out
}

/// Turns a series of "how far over" values into moments: a run of at least
/// `min_run` values over zero starts a moment, and runs that start within
/// `quiet` seconds of the last moment join it. `time_of` gives each value's
/// time: video frame times are taken from the frames, never counted from a
/// frame rate (containers round them, and counting drifts).
fn runs(
    over: &[f32],
    time_of: impl Fn(usize) -> f64,
    min_run: usize,
    quiet: f64,
    source: &str,
) -> Vec<Moment> {
    let mut out: Vec<Moment> = Vec::new();
    let mut i = 0;
    while i < over.len() {
        if over[i] <= 0.0 {
            i += 1;
            continue;
        }
        let begin = i;
        let mut peak = 0.0f32;
        while i < over.len() && over[i] > 0.0 {
            peak = peak.max(over[i]);
            i += 1;
        }
        if i - begin < min_run {
            continue;
        }
        let time = time_of(begin);
        match out.last_mut() {
            Some(last) if time - last.time < quiet => last.score = last.score.max(peak),
            _ => out.push(Moment {
                time,
                score: peak,
                sources: source.to_owned(),
            }),
        }
    }
    out
}

/// Values of one stream in blocks of `size` steps: the mean of each block.
fn blocks(values: &[f32], size: usize) -> Vec<f32> {
    values
        .chunks(size)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

/// 50 ms blocks: the audio detectors work on these.
const BLOCK: usize = 5;

/// The level a stream "usually" has, so the mic's pauses don't make every
/// word look loud: the middle loudness of the blocks that have sound in them
/// (well above the quietest tenth). When those are rare they are the
/// bursts themselves, not talking, and the middle of everything is used.
fn active_level(short: &[f32]) -> f32 {
    let floor = quantile(short, 0.1) + 15.0;
    let active: Vec<f32> = short.iter().copied().filter(|&v| v > floor).collect();
    if active.len() * 5 < short.len() {
        quantile(short, 0.5)
    } else {
        quantile(&active, 0.5)
    }
}

/// Approach A on one stream.
pub fn loudness(f: &Features, role: Role, s: Sensitivity) -> Vec<Moment> {
    let step = Features::step() * BLOCK as f64;
    let power = blocks(&f.power, BLOCK);
    // Loudness over the last 200 ms.
    let short: Vec<f32> = (0..power.len())
        .map(|b| {
            let from = b.saturating_sub(3);
            db(power[from..=b].iter().sum::<f32>() / (b - from + 1) as f32)
        })
        .collect();
    let usual = active_level(&short);
    let local = local_quantile(&short, step, 15.0, 0.75);
    let threshold = s.pick(14.0, 10.0, 7.0);
    let over: Vec<f32> = short
        .iter()
        .zip(&local)
        .map(|(&v, &l)| v - l.max(usual) - threshold)
        .collect();
    let mut found = runs(&over, |b| f.start + b as f64 * step, 2, 2.0, role.name());
    for m in &mut found {
        m.score += threshold;
    }
    found
}

/// The onsets of one stream for approach B: peaks of the spectral flux
/// (summed over 100 ms, in dB) that stand out from the stream's usual
/// flux by `z` robust standard deviations.
fn onsets(f: &Features) -> Vec<(f64, f32)> {
    const MIN_Z: f32 = 3.0;
    let step = Features::step() * BLOCK as f64;
    let flux = blocks(&f.flux, BLOCK);
    let novelty: Vec<f32> = (0..flux.len())
        .map(|b| {
            let from = b.saturating_sub(1);
            db((flux[from..=b].iter().sum::<f32>() * BLOCK as f32).powi(2))
        })
        .collect();
    let median = local_quantile(&novelty, step, 15.0, 0.5);
    let spread: Vec<f32> = novelty.iter().zip(&median).map(|(v, m)| (v - m).abs()).collect();
    let mad = local_quantile(&spread, step, 15.0, 0.5);
    let z: Vec<f32> = (0..novelty.len())
        .map(|i| (novelty[i] - median[i]) / (1.4826 * mad[i]).max(2.0))
        .collect();
    let mut out = Vec::new();
    for i in 0..z.len() {
        let lo = i.saturating_sub(5);
        let hi = (i + 6).min(z.len());
        if z[i] >= MIN_Z && z[lo..hi].iter().all(|&o| o <= z[i]) {
            out.push((f.start + i as f64 * step, z[i] - MIN_Z));
        }
    }
    out
}

/// Approach B over all streams together.
pub fn flux(streams: &[(Role, &Features)], s: Sensitivity) -> Vec<Moment> {
    let mut peaks: Vec<(f64, f32, Role)> = Vec::new();
    for (role, f) in streams {
        peaks.extend(onsets(f).into_iter().map(|(t, z)| (t, z, *role)));
    }
    peaks.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Onsets within half a second of each other, on any stream, are one
    // event; each stream adds its strongest onset in it, weighted.
    let mut clusters: Vec<Moment> = Vec::new();
    let mut i = 0;
    while i < peaks.len() {
        let first = peaks[i].0;
        let mut best: Vec<(Role, f32)> = Vec::new();
        while i < peaks.len() && peaks[i].0 - first <= 0.5 {
            let (_, z, role) = peaks[i];
            match best.iter_mut().find(|(r, _)| *r == role) {
                Some(b) => b.1 = b.1.max(z),
                None => best.push((role, z)),
            }
            i += 1;
        }
        let score = best.iter().map(|(r, z)| r.weight() * z).sum();
        let sources = best.iter().map(|(r, _)| r.name()).collect::<Vec<_>>().join("+");
        clusters.push(Moment {
            time: first,
            score,
            sources,
        });
    }
    let threshold = s.pick(7.0, 4.0, 2.0);
    clusters.retain(|c| c.score >= threshold);
    strongest_apart(clusters, 2.0)
}

/// Keeps the strongest moments, dropping any within `apart` seconds of a
/// stronger one kept.
fn strongest_apart(mut moments: Vec<Moment>, apart: f64) -> Vec<Moment> {
    moments.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Moment> = Vec::new();
    for m in moments {
        if kept.iter().all(|k| (k.time - m.time).abs() >= apart) {
            kept.push(m);
        }
    }
    kept.sort_by(|a, b| a.time.total_cmp(&b.time));
    kept
}

/// The median of `values[i]` over the indices within `reach` of `i`,
/// leaving `i` out.
fn around(values: &[f32], i: usize, reach: usize) -> f32 {
    let lo = i.saturating_sub(reach);
    let hi = (i + reach + 1).min(values.len());
    let others: Vec<f32> = (lo..hi).filter(|&j| j != i).map(|j| values[j]).collect();
    quantile(&others, 0.5)
}

fn frame_step(changes: &[Change]) -> f64 {
    let gaps: Vec<f32> = changes
        .windows(2)
        .map(|w| (w[1].time - w[0].time) as f32)
        .collect();
    let step = f64::from(quantile(&gaps, 0.5));
    if step > 0.0 { step } else { 1.0 / 60.0 }
}

/// Which frames start a new shot, from every decoded frame.
fn cut_frames(changes: &[Change], s: Sensitivity) -> Vec<bool> {
    let step = frame_step(changes);
    let half_second = (0.5 / step).round() as usize;
    let hist: Vec<f32> = changes.iter().map(|c| c.histogram).collect();
    let diff: Vec<f32> = changes.iter().map(|c| c.difference).collect();
    let min_hist = s.pick(0.35, 0.25, 0.15);
    let min_diff = s.pick(30.0, 20.0, 15.0);
    (0..changes.len())
        .map(|i| {
            let colours = hist[i] >= min_hist && hist[i] >= 3.0 * around(&hist, i, half_second);
            // A jump of the whole picture in one frame, with the frames
            // around it calm: a cut between two views of one place.
            let calm = |j: Option<usize>| j.and_then(|j| diff.get(j)).is_none_or(|&d| d < diff[i] / 2.0);
            let jump = diff[i] >= min_diff.max(4.0 * around(&diff, i, half_second))
                && calm(i.checked_sub(2))
                && calm(Some(i + 2));
            colours || jump
        })
        .collect()
}

/// Approach C on every decoded frame: cuts and sudden motion.
pub fn video(changes: &[Change], s: Sensitivity) -> Vec<Moment> {
    if changes.len() < 3 {
        return Vec::new();
    }
    let step = frame_step(changes);
    let cut = cut_frames(changes, s);
    let mut cuts: Vec<Moment> = Vec::new();
    for (i, c) in changes.iter().enumerate() {
        if cut[i] && cuts.last().is_none_or(|l| c.time - l.time >= 1.0) {
            cuts.push(Moment {
                time: c.time,
                score: c.histogram.max(c.difference / 100.0),
                sources: "cut".into(),
            });
        }
    }
    // Motion: the difference over the last 0.2 s, with cut frames standing
    // in for the frame before, against what is usual over ±10 s.
    let mut steady = Vec::with_capacity(changes.len());
    for (i, c) in changes.iter().enumerate() {
        let d = if cut[i] {
            steady.last().copied().unwrap_or(0.0)
        } else {
            c.difference
        };
        steady.push(d);
    }
    let window = ((0.2 / step).round() as usize).max(1);
    let motion: Vec<f32> = (0..steady.len())
        .map(|i| {
            let from = (i + 1).saturating_sub(window);
            steady[from..=i].iter().sum::<f32>() / (i - from + 1) as f32
        })
        .collect();
    let usual = local_quantile(&motion, step, 10.0, 0.5);
    let factor = s.pick(3.5, 2.5, 2.0);
    let over: Vec<f32> = motion
        .iter()
        .zip(&usual)
        .map(|(&m, &u)| m / (factor * u).max(3.0) - 1.0)
        .collect();
    let min_run = ((0.1 / step).round() as usize).max(1);
    let moves = runs(&over, |i| changes[i].time, min_run, 2.0, "motion");
    merge(&[cuts, moves], 1.0)
}

/// Approach C on key frames only: pairs of neighbouring key frames whose
/// colours differ a lot, so a cut lies between them. The exact frame is
/// found by decoding only that stretch ([`refine`]).
pub fn key_cuts(changes: &[Change], s: Sensitivity) -> Vec<(f64, f64)> {
    let min_hist = s.pick(0.35, 0.25, 0.15);
    let mut out = Vec::new();
    for (i, c) in changes.iter().enumerate() {
        if c.histogram >= min_hist {
            let before = if i == 0 { 0.0 } else { changes[i - 1].time };
            out.push((before, c.time));
        }
    }
    out
}

/// The frame of a cut within a stretch decoded frame by frame: the
/// biggest colour change, or the biggest single-frame jump.
pub fn refine(changes: &[Change]) -> Option<Moment> {
    let best = changes.iter().max_by(|a, b| {
        (a.histogram + a.difference / 255.0).total_cmp(&(b.histogram + b.difference / 255.0))
    })?;
    Some(Moment {
        time: best.time,
        score: best.histogram,
        sources: "cut".into(),
    })
}

/// Puts the moments of several detectors together: moments within
/// `window` seconds of the first of a group are one, at the earliest time,
/// with the best score and every source.
pub fn merge(lists: &[Vec<Moment>], window: f64) -> Vec<Moment> {
    let mut all: Vec<Moment> = lists.iter().flatten().cloned().collect();
    all.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut out: Vec<Moment> = Vec::new();
    for m in all {
        match out.last_mut() {
            Some(last) if m.time - last.time <= window => {
                last.score = last.score.max(m.score);
                for s in m.sources.split('+') {
                    if !last.sources.split('+').any(|x| x == s) {
                        last.sources.push('+');
                        last.sources.push_str(s);
                    }
                }
            }
            _ => out.push(m),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(levels: &[(f64, f32)], seconds: f64) -> Features {
        // A steady level with louder stretches: (start time, power) pairs
        // last one second each.
        let step = Features::step();
        let n = (seconds / step) as usize;
        let mut power = vec![1e-4f32; n];
        for &(t, p) in levels {
            let from = (t / step) as usize;
            for v in power.iter_mut().skip(from).take((1.0 / step) as usize) {
                *v = p;
            }
        }
        // Flux follows the level changes.
        let mut flux = vec![1e-3f32; n];
        for &(t, p) in levels {
            flux[(t / step) as usize] = p.sqrt();
        }
        Features {
            start: 0.0,
            power,
            flux,
        }
    }

    #[test]
    fn loudness_finds_bursts_and_ignores_the_usual_level() {
        let f = stream(&[(20.0, 1e-1), (40.0, 2e-4), (60.0, 5e-2)], 90.0);
        let found = loudness(&f, Role::Mic, Sensitivity::Medium);
        let times: Vec<f64> = found.iter().map(|m| m.time).collect();
        assert_eq!(times.len(), 2, "{times:?}");
        assert!((times[0] - 20.0).abs() < 0.2);
        assert!((times[1] - 60.0).abs() < 0.2);
    }

    #[test]
    fn flux_weights_the_mic_above_the_game() {
        let f = stream(&[(30.0, 1e-2)], 60.0);
        let mic = flux(&[(Role::Mic, &f)], Sensitivity::Medium);
        assert_eq!(mic.len(), 1);
        assert!((mic[0].time - 30.0).abs() < 0.2);
        assert_eq!(mic[0].sources, "mic");
        let both = flux(&[(Role::Mic, &f), (Role::Game, &f)], Sensitivity::Medium);
        assert_eq!(both.len(), 1);
        assert!(both[0].score > mic[0].score);
        assert_eq!(both[0].sources, "mic+game");
    }

    fn frames(n: usize, at: impl Fn(usize) -> (f32, f32)) -> Vec<Change> {
        (0..n)
            .map(|i| {
                let (difference, histogram) = at(i);
                Change {
                    time: i as f64 / 60.0,
                    difference,
                    histogram,
                }
            })
            .collect()
    }

    #[test]
    fn video_finds_a_cut_a_jump_and_a_pan() {
        // 60 s at 60 fps of gentle motion; a cut to new colours at 10 s, a
        // jump within the same view at 20 s, a fast pan from 30 to 30.4 s.
        let changes = frames(3600, |i| match i {
            600 => (60.0, 0.6),
            1200 => (50.0, 0.02),
            1800..=1824 => (40.0, 0.03),
            _ => (2.0 + (i % 7) as f32 * 0.1, 0.01),
        });
        let found = video(&changes, Sensitivity::Medium);
        let summary: Vec<(i64, &str)> = found
            .iter()
            .map(|m| ((m.time * 10.0).round() as i64, m.sources.as_str()))
            .collect();
        assert_eq!(summary, vec![(100, "cut"), (200, "cut"), (300, "motion")]);
    }

    #[test]
    fn motion_times_come_from_the_frames_not_a_frame_rate() {
        // Times rounded to milliseconds, as MKV stores them: the gaps are
        // 16 and 17 ms, and counting the usual gap would drift by seconds.
        let changes: Vec<Change> = (0..7200)
            .map(|i| Change {
                time: (i as f64 * 1000.0 / 60.0).round() / 1000.0,
                difference: if (6000..6024).contains(&i) { 40.0 } else { 2.0 },
                histogram: 0.01,
            })
            .collect();
        let found = video(&changes, Sensitivity::Medium);
        assert_eq!(found.len(), 1);
        assert!((found[0].time - 100.0).abs() < 0.05, "{}", found[0].time);
    }

    #[test]
    fn key_frames_point_at_the_stretch_with_the_cut() {
        let keys = vec![
            Change {
                time: 2.0,
                difference: 30.0,
                histogram: 0.05,
            },
            Change {
                time: 4.0,
                difference: 30.0,
                histogram: 0.5,
            },
            Change {
                time: 6.0,
                difference: 30.0,
                histogram: 0.04,
            },
        ];
        assert_eq!(key_cuts(&keys, Sensitivity::Medium), vec![(2.0, 4.0)]);
    }

    #[test]
    fn merge_joins_nearby_moments_and_keeps_every_source() {
        let m = |time, sources: &str| Moment {
            time,
            score: 1.0,
            sources: sources.into(),
        };
        let merged = merge(
            &[
                vec![m(10.0, "mic"), m(30.0, "mic")],
                vec![m(10.8, "cut"), m(50.0, "motion")],
            ],
            1.5,
        );
        let summary: Vec<(f64, &str)> = merged.iter().map(|m| (m.time, m.sources.as_str())).collect();
        assert_eq!(summary, vec![(10.0, "mic+cut"), (30.0, "mic"), (50.0, "motion")]);
    }

    #[test]
    fn quantiles() {
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(quantile(&v, 0.5), 3.0);
        assert_eq!(quantile(&v, 0.0), 1.0);
        assert_eq!(quantile(&v, 1.0), 5.0);
    }
}
