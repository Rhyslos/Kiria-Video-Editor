//! Sync by sound: where a separately recorded sound (a mic, a friend's
//! recording) lines up with a reference (the game recording), found by
//! comparing what they hear, on a worker.
//!
//! Both are read as one channel at 8 000 samples per second and compared
//! whole, every frequency of a voice weighed the same
//! ([`correlate::SPEECH_BAND`]), so a different mic or level compares the
//! same. Around the best fit a few seconds where both are loudest are read
//! again at 48 000 per second and compared once more, which finds the fit
//! to the sample. How sure the fit is comes from how far its peak stands
//! above the next best and above the rest ([`Sureness`]).
//!
//! The clip compared is the part of its file it plays; the other clip is
//! searched up to [`SEARCH`] either way of where it sits now, so partial
//! overlap and a clip cut from a longer recording are found too.

pub mod correlate;
#[cfg(test)]
mod speech;
#[cfg(test)]
mod tests;

use correlate::{Fit, Method};
use diag::AppError;
use media::AudioDecoder;
use project::{Clip, Time};
use std::path::{Path, PathBuf};

/// The rate the whole sounds are compared at.
pub const LOW_RATE: u32 = 8_000;
/// The rate the exact fit is found at: offsets are counted in its samples.
pub const FULL_RATE: u32 = 48_000;
/// Samples at [`FULL_RATE`] per sample at [`LOW_RATE`].
const STEP: i64 = (FULL_RATE / LOW_RATE) as i64;

/// The most of the reference compared (seconds): plenty to be sure, and
/// little enough to read quickly.
pub const REFERENCE_MOST: i64 = 120;
/// How far either way of where the other clip sits now it is looked for
/// (seconds).
pub const SEARCH: i64 = 180;
/// How long the stretch compared again at the full rate is (seconds).
const EXACT_STRETCH: i64 = 6;
/// How far either way of the rough fit the exact one is looked for
/// (samples at the full rate): the rough fit is within a sample or two at
/// the low rate, and a recorder's clock may drift a little more.
const EXACT_REACH: i64 = 480;
/// The least the two must overlap to count (seconds), unless one is shorter.
const LEAST_OVERLAP: i64 = 2;
/// Sound quieter than this (root mean square) is taken as silence.
const SILENT: f32 = 1e-4;

/// How sure a fit is, in the words the result shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sureness {
    /// Nothing clearly lines up: the sounds may not share anything.
    Unsure,
    /// One fit is clearly better than the rest.
    Likely,
    /// One fit stands far above the rest.
    Sure,
}

impl Sureness {
    /// From the rough fit's peak against the next best and the rest. Tuned
    /// on the made-up recordings of the tests: unrelated sounds stay under
    /// 1.2 and 7, the same speech through a drifting clock comes out near
    /// 1.5 and 9, and through different mics at clearly over 2 and 12.
    pub fn of(fit: &Fit) -> Sureness {
        if fit.ratio >= 2.0 && fit.prominence >= 12.0 {
            Sureness::Sure
        } else if fit.ratio >= 1.4 && fit.prominence >= 8.0 {
            Sureness::Likely
        } else {
            Sureness::Unsure
        }
    }

    /// What the result says.
    pub fn words(self) -> &'static str {
        match self {
            Sureness::Sure => "Sure",
            Sureness::Likely => "Likely",
            Sureness::Unsure => "Unsure — check by ear",
        }
    }
}

/// What was found for one clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    /// Where in the reference's file a moment is, less where it is in the
    /// other's file, in samples at [`FULL_RATE`]. `None` when nothing could
    /// be compared (silence, or no overlap).
    pub offset: Option<i64>,
    pub sureness: Sureness,
    /// The rough fit's peak over the next best (see [`Fit`]).
    pub ratio: f32,
    /// The rough fit's peak over the rest.
    pub prominence: f32,
}

impl Found {
    fn nothing() -> Found {
        Found {
            offset: None,
            sureness: Sureness::Unsure,
            ratio: 0.0,
            prominence: 0.0,
        }
    }
}

/// A stretch of a file's sound, in samples at [`FULL_RATE`] from its time 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub from: i64,
    pub length: i64,
}

/// Somewhere sound is read from.
pub trait Sound {
    /// `length` samples (one channel) at `rate` (a whole share of
    /// [`FULL_RATE`]) from sample `from` at that rate; silence where there
    /// is none (before the start, past the end). `watch` hears how much of
    /// it is read.
    fn read(&mut self, from: i64, length: usize, rate: u32, watch: &Watch) -> diag::Result<Vec<f32>>;
}

/// How a worker follows the work: how far it is (0 to 1), and whether to stop.
pub struct Watch<'a> {
    pub progress: &'a dyn Fn(f32),
    pub cancelled: &'a dyn Fn() -> bool,
}

impl std::fmt::Debug for Watch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Watch")
    }
}

impl Watch<'_> {
    fn stopped(&self) -> diag::Result<()> {
        if (self.cancelled)() {
            Err(AppError::new("Sync by sound was stopped").why("It was cancelled before it finished."))
        } else {
            Ok(())
        }
    }
}

/// A sound stream of a file, read with the media decoder.
#[derive(Debug)]
pub struct FileSound {
    path: PathBuf,
    stream: u32,
    /// The decoder open now, and the rate it reads at.
    open: Option<(u32, AudioDecoder)>,
}

impl FileSound {
    pub fn new(path: &Path, stream: u32) -> FileSound {
        FileSound {
            path: path.to_path_buf(),
            stream,
            open: None,
        }
    }
}

/// Samples read in one go: a second at the full rate, so a stop is heard soon.
const CHUNK: usize = FULL_RATE as usize;

impl Sound for FileSound {
    fn read(&mut self, from: i64, length: usize, rate: u32, watch: &Watch) -> diag::Result<Vec<f32>> {
        let mut out = vec![0.0f32; length];
        // Before the file's time 0 there is nothing: silence.
        let skip = usize::try_from(-from).unwrap_or(0).min(length);
        let from = from.max(0);
        if self.open.as_ref().is_none_or(|(r, _)| *r != rate) {
            self.open = Some((rate, AudioDecoder::open(&self.path, self.stream, rate)?));
        }
        let Some((_, decoder)) = self.open.as_mut() else {
            return Ok(out);
        };
        if decoder.next_sample() != from {
            decoder.seek_to_sample(from);
        }
        let mut stereo = vec![0.0f32; CHUNK * 2];
        let mut at = skip;
        while at < length {
            watch.stopped()?;
            let want = (length - at).min(CHUNK);
            let got = decoder.read(&mut stereo[..want * 2]);
            for (slot, pair) in out[at..at + got].iter_mut().zip(stereo.chunks_exact(2)) {
                *slot = 0.5 * (pair[0] + pair[1]);
            }
            at += got;
            (watch.progress)(at as f32 / length as f32);
            if got < want {
                break;
            }
        }
        Ok(out)
    }
}

/// What to compare for a clip, and where: its sound and the stretch of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSound {
    pub path: PathBuf,
    pub stream: u32,
    pub span: Span,
}

/// `t` in samples at [`FULL_RATE`], to the nearest.
pub fn samples(t: Time) -> i64 {
    let n = i128::from(t.num()) * i128::from(FULL_RATE);
    let d = i128::from(t.den());
    i64::try_from((2 * n + d).div_euclid(2 * d)).unwrap_or(i64::MAX)
}

/// The stretches to compare: the part of its file `reference` plays (at
/// most [`REFERENCE_MOST`], the part beside `other` when they overlap),
/// and the part of its file `other` plays that could line up with it if
/// `other` moved up to [`SEARCH`] either way.
pub fn spans(reference: &Clip, other: &Clip) -> (Span, Span) {
    let most = REFERENCE_MOST * i64::from(FULL_RATE);
    // The reference's stretch on the timeline: the part beside the other
    // clip when there is one, at most `most` long, within the clip.
    let (r_start, r_end) = (samples(reference.start), samples(reference.end()));
    let (o_start, o_end) = (samples(other.start), samples(other.end()));
    let side_by_side = r_start.max(o_start)..r_end.min(o_end);
    let mut from = if side_by_side.is_empty() {
        r_start
    } else {
        side_by_side.start
    };
    let to = (from + most).min(r_end);
    from = from.min((to - most).max(r_start));
    // In the reference's file.
    let r_in = samples(reference.source_in) - r_start;
    let reference_span = Span {
        from: from + r_in,
        length: to - from,
    };
    // In the other's file: that stretch as the other sits now, widened
    // by the search, within what the other plays.
    let search = SEARCH * i64::from(FULL_RATE);
    let o_in = samples(other.source_in) - o_start;
    let low = (from - search).max(o_start) + o_in;
    let high = (to + search).min(o_end) + o_in;
    let other_span = Span {
        from: low,
        length: (high - low).max(0),
    };
    (reference_span, other_span)
}

/// Where `other` would start on the timeline to line up with `reference`,
/// given the `offset` found (see [`Found::offset`]): exact, not on a frame.
pub fn lined_up_start(reference: &Clip, other: &Clip, offset: i64) -> Time {
    let offset = Time::from_ratio(offset, u64::from(FULL_RATE)).unwrap_or(Time::ZERO);
    reference.start - reference.source_in + other.source_in + offset
}

/// Lines `others` up with `reference`, one result each, in order. A clip
/// whose sound can't be read gets its error; a stop ends it all.
pub fn line_up_files(
    reference: &ClipSound,
    others: &[ClipSound],
    watch: &Watch,
) -> diag::Result<Vec<diag::Result<Found>>> {
    let mut sound = FileSound::new(&reference.path, reference.stream);
    let parts = others.len() as f32 + 1.0;
    let progress = |share: f32| (watch.progress)(share / parts);
    let reading = Watch {
        progress: &progress,
        cancelled: watch.cancelled,
    };
    let reference_low = read_low(&mut sound, reference.span, &reading)?;
    let mut results = Vec::with_capacity(others.len());
    for (i, other) in others.iter().enumerate() {
        let progress = |share: f32| (watch.progress)((i as f32 + 1.0 + share) / parts);
        let part = Watch {
            progress: &progress,
            cancelled: watch.cancelled,
        };
        let mut other_sound = FileSound::new(&other.path, other.stream);
        let found = line_up(
            &mut sound,
            reference.span,
            &reference_low,
            &mut other_sound,
            other.span,
            &part,
        );
        watch.stopped()?;
        results.push(found);
    }
    (watch.progress)(1.0);
    Ok(results)
}

/// `span` of `sound` at [`LOW_RATE`].
fn read_low(sound: &mut dyn Sound, span: Span, watch: &Watch) -> diag::Result<Vec<f32>> {
    let length = usize::try_from(span.length / STEP).unwrap_or(0);
    sound.read(span.from / STEP, length, LOW_RATE, watch)
}

/// Where `other_span` of `other` lines up with `reference_span` of
/// `reference` (already read at the low rate as `reference_low`).
pub fn line_up(
    reference: &mut dyn Sound,
    reference_span: Span,
    reference_low: &[f32],
    other: &mut dyn Sound,
    other_span: Span,
    watch: &Watch,
) -> diag::Result<Found> {
    let progress = |share: f32| (watch.progress)(share * 0.8);
    let reading = Watch {
        progress: &progress,
        cancelled: watch.cancelled,
    };
    let other_low = read_low(other, other_span, &reading)?;
    if loudness(reference_low) < SILENT || loudness(&other_low) < SILENT {
        return Ok(Found::nothing());
    }
    let shorter = reference_low.len().min(other_low.len());
    let least = (LEAST_OVERLAP as usize * LOW_RATE as usize)
        .min(shorter / 2)
        .max(1);
    let Some(rough) = correlate::fit(reference_low, &other_low, LOW_RATE, Method::Whitened, least)? else {
        return Ok(Found::nothing());
    };
    watch.stopped()?;
    (watch.progress)(0.9);
    // Rough: where in the reference's file less where in the other's.
    let rough_offset = (reference_span.from / STEP + rough.shift - other_span.from / STEP) * STEP;
    let exact = exact_offset(
        reference,
        other,
        rough.shift,
        reference_low,
        &other_low,
        reference_span.from / STEP,
        rough_offset,
        watch,
    )?;
    (watch.progress)(1.0);
    Ok(Found {
        offset: Some(exact.unwrap_or(rough_offset)),
        sureness: Sureness::of(&rough),
        ratio: rough.ratio,
        prominence: rough.prominence,
    })
}

/// The offset to the sample: a few seconds where both are loud, read
/// again at the full rate and compared near `rough_offset`. `shift` is the
/// rough fit (at the low rate) of `other_low` on `reference_low`, which
/// starts at sample `reference_from` (low rate) of its file.
#[allow(clippy::too_many_arguments)]
fn exact_offset(
    reference: &mut dyn Sound,
    other: &mut dyn Sound,
    shift: i64,
    reference_low: &[f32],
    other_low: &[f32],
    reference_from: i64,
    rough_offset: i64,
    watch: &Watch,
) -> diag::Result<Option<i64>> {
    // Side by side in the reference's samples: `m` there is `m - shift`
    // in the other's.
    let (lr, lo) = (reference_low.len() as i64, other_low.len() as i64);
    let (first, last) = (shift.max(0), lr.min(lo + shift));
    if last <= first {
        return Ok(None);
    }
    // The loudest stretch of both together, in half-second steps.
    let stretch = (EXACT_STRETCH * i64::from(LOW_RATE)).min(last - first);
    let step = i64::from(LOW_RATE) / 2;
    let energy = |x: &[f32], from: i64| -> f64 {
        let from = from.max(0) as usize;
        let to = (from + stretch as usize).min(x.len());
        x[from.min(to)..to]
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum()
    };
    let mut best = (first, f64::MIN);
    let mut m = first;
    while m + stretch <= last {
        let both = energy(reference_low, m).min(energy(other_low, m - shift));
        if both > best.1 {
            best = (m, both);
        }
        m += step;
    }
    let ignore = Watch {
        progress: &|_| {},
        cancelled: watch.cancelled,
    };
    let length = usize::try_from(stretch * STEP).unwrap_or(0);
    let r_from = (reference_from + best.0) * STEP;
    let a = reference.read(r_from, length, FULL_RATE, &ignore)?;
    let o_from = r_from - rough_offset - EXACT_REACH;
    let b = other.read(o_from, length + 2 * EXACT_REACH as usize, FULL_RATE, &ignore)?;
    // `b[n]` is heard at `a[n - EXACT_REACH]` when the rough fit is exact.
    let Some(fit) = correlate::fit_near(&a, &b, FULL_RATE, -EXACT_REACH, EXACT_REACH)? else {
        return Ok(None);
    };
    Ok(Some(r_from + fit.shift - o_from))
}

/// Root mean square.
fn loudness(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len() as f64).sqrt() as f32
}
