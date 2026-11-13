//! Finding the quiet stretches of a sound track, for "Remove silence".
//!
//! Works from the waveform peaks (100 per second) already read for the
//! timeline, so it is instant. Gaps between clips count as silent; the
//! track's end is where its last clip ends.
//!
//! Those peaks are the recording as it is, before the track's own effects:
//! hiss that Clean-up takes out still counts as sound here. So the search
//! also says how loud the recording's quietest moments are
//! ([`floor_db`]), and a threshold under that finds nothing
//! ([`suggested_threshold`] is one just over it).

/// What counts as silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Quieter than this (decibels below full scale) is silent.
    pub threshold_db: f32,
    /// Only stretches at least this long (seconds) are removed.
    pub min_length: f64,
    /// Kept at each edge of a stretch (seconds), so speech isn't clipped.
    pub keep: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            threshold_db: -45.0,
            min_length: 0.8,
            keep: 0.15,
        }
    }
}

/// One clip of the track as the search needs it: where it sits, where in
/// its file it starts, and the file's peaks (`per_second` of them).
#[derive(Debug)]
pub struct ClipSound<'a> {
    pub start: f64,
    pub duration: f64,
    pub source_in: f64,
    pub peaks: &'a [f32],
    pub per_second: u32,
    /// How fast the clip plays its file.
    pub speed: f64,
    /// Its speed ramp, when its speed changes over time (`speed` then
    /// means nothing).
    pub ramp: Option<&'a project::Ramp>,
}

/// Steps per second of the track's loudness line.
const STEPS: f64 = 100.0;

/// The loudest peak in each step of the track, and whether a clip is
/// there (gaps stay 0, silent); and where the track ends, seconds.
fn levels(clips: &[ClipSound]) -> (Vec<f32>, Vec<bool>, f64) {
    let end = clips.iter().map(|c| c.start + c.duration).fold(0.0, f64::max);
    let steps = (end * STEPS).ceil() as usize;
    let mut level = vec![0f32; steps];
    let mut covered = vec![false; steps];
    for c in clips {
        let first = (c.start * STEPS).floor() as usize;
        let last = (((c.start + c.duration) * STEPS).ceil() as usize).min(steps);
        for (i, slot) in level.iter_mut().enumerate().take(last).skip(first) {
            covered[i] = true;
            let into = i as f64 / STEPS - c.start;
            let t = match c.ramp {
                Some(ramp) => ramp.source_offset_f64(into),
                None => into * c.speed,
            } + c.source_in;
            let at = (t * f64::from(c.per_second)).floor();
            if at >= 0.0 {
                let peak = c.peaks.get(at as usize).copied().unwrap_or(0.0);
                *slot = slot.max(peak);
            }
        }
    }
    (level, covered, end)
}

/// The silent stretches of the track, as (start, end) seconds on the
/// timeline, in order.
pub fn find(clips: &[ClipSound], settings: Settings) -> Vec<(f64, f64)> {
    let (level, _, end) = levels(clips);
    let steps = level.len();
    let quiet = 10f32.powf(settings.threshold_db / 20.0);
    let mut found = Vec::new();
    let mut run_start = None;
    for (i, l) in level.iter().chain(std::iter::once(&f32::MAX)).enumerate() {
        match (run_start, *l < quiet) {
            (None, true) => run_start = Some(i),
            (Some(s), false) => {
                let (a, b) = (s as f64 / STEPS, i as f64 / STEPS);
                if b - a >= settings.min_length {
                    // Keep a little at each edge, but none before the start
                    // of the timeline or after the end of the track.
                    let a = if s == 0 { 0.0 } else { a + settings.keep };
                    let b = if i >= steps { end } else { b - settings.keep };
                    if b > a {
                        found.push((a, b));
                    }
                }
                run_start = None;
            }
            _ => {}
        }
    }
    found
}

/// How loud the recording is at its quietest, dBFS: the level its
/// quietest tenth of moments under the clips stays under (the hiss or hum
/// of a microphone between words). `None` for a track without clips, or
/// one that is truly silent somewhere (digital silence finds itself).
pub fn floor_db(clips: &[ClipSound]) -> Option<f32> {
    let (level, covered, _) = levels(clips);
    let mut under: Vec<f32> = level
        .iter()
        .zip(&covered)
        .filter(|(_, c)| **c)
        .map(|(l, _)| *l)
        .collect();
    if under.is_empty() {
        return None;
    }
    under.sort_by(f32::total_cmp);
    let tenth = under[under.len() / 10];
    (tenth > 0.0).then(|| 20.0 * tenth.log10())
}

/// A threshold that finds the pauses of a recording whose quietest
/// moments are at `floor_db`: 6 dB over it, in whole decibels, no
/// quieter than the usual and no louder than -20 dB.
pub fn suggested_threshold(floor_db: f32) -> f32 {
    (floor_db + 6.0)
        .ceil()
        .clamp(Settings::default().threshold_db, -20.0)
}

/// How long the stretches add up to, in seconds.
pub fn total(stretches: &[(f64, f64)]) -> f64 {
    stretches.iter().map(|(a, b)| b - a).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quiet_stretches_and_gaps() {
        // 10 s of peaks: loud, quiet from 2 s to 5 s, loud again.
        let peaks: Vec<f32> = (0..1000)
            .map(|i| if (200..500).contains(&i) { 0.001 } else { 0.5 })
            .collect();
        let clip = |start: f64, duration: f64| ClipSound {
            start,
            duration,
            source_in: 0.0,
            peaks: &peaks,
            per_second: 100,
            speed: 1.0,
            ramp: None,
        };
        let settings = Settings {
            threshold_db: -40.0,
            min_length: 1.0,
            keep: 0.1,
        };
        let one = find(&[clip(0.0, 10.0)], settings);
        assert_eq!(one.len(), 1);
        let (a, b) = one[0];
        assert!((a - 2.1).abs() < 0.02 && (b - 4.9).abs() < 0.02, "{a}..{b}");

        // A second copy after a 2 s gap: the gap is silent too.
        let two = find(&[clip(0.0, 10.0), clip(12.0, 10.0)], settings);
        assert_eq!(two.len(), 3, "{two:?}");
        assert!((two[1].0 - 10.1).abs() < 0.02 && (two[1].1 - 11.9).abs() < 0.02);
        assert!((total(&two) - (2.8 * 2.0 + 1.8)).abs() < 0.05);

        // Too short to count.
        let strict = Settings {
            min_length: 5.0,
            ..settings
        };
        assert!(find(&[clip(0.0, 10.0)], strict).is_empty());
    }

    /// A microphone with hiss at about -35 dBFS between words: the usual
    /// -45 dB finds nothing, the floor says why, and the threshold offered
    /// finds the four pauses.
    #[test]
    fn a_hissy_recording_says_its_floor_and_a_threshold_that_works() {
        // 20 s: talking (0.5) with four 3 s pauses of hiss (peaks about
        // -32 dBFS, varying a little).
        let peaks: Vec<f32> = (0..2_000)
            .map(|i| {
                let pause = [200..500, 700..1_000, 1_200..1_500, 1_650..1_950]
                    .iter()
                    .any(|r| r.contains(&i));
                if pause {
                    0.022 + 0.004 * ((i % 7) as f32 / 7.0)
                } else {
                    0.5
                }
            })
            .collect();
        let clip = ClipSound {
            start: 0.0,
            duration: 20.0,
            source_in: 0.0,
            peaks: &peaks,
            per_second: 100,
            speed: 1.0,
            ramp: None,
        };
        let clips = [clip];
        assert!(find(&clips, Settings::default()).is_empty(), "under the hiss");
        let floor = floor_db(&clips).unwrap();
        assert!((-34.0..-31.0).contains(&floor), "{floor}");
        let threshold = suggested_threshold(floor);
        assert!((-28.0..=-25.0).contains(&threshold), "{threshold}");
        let found = find(
            &clips,
            Settings {
                threshold_db: threshold,
                ..Settings::default()
            },
        );
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(suggested_threshold(-90.0), -45.0, "never quieter than the usual");
        assert!(floor_db(&[]).is_none());
    }
}
