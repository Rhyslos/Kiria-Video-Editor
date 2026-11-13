//! From band levels frame by frame to findings in plain words.
//!
//! **Covering the voice.** In each frame where the dialogue speaks, each
//! other track is weighed against the dialogue tracks together, band by
//! band from 315 Hz to 4 kHz: a band where the track is [`CLEAR_DB`] or more
//! under the voice is clear, one where it is [`COVERED_DB`] or more over it
//! is covered, and in between it is covered in part, in a straight line.
//! How bad a frame is: the covered share of the bands the voice has sound
//! in, each weighted by how much it matters to following speech
//! ([`super::bands::IMPORTANCE`]). 0 is clear, 1 is covered everywhere.
//!
//! **Muddy.** In each frame, the tracks whose sound between 200 and
//! 500 Hz is over [`MUD_FLOOR_DB`] and within [`MUD_NEAR_DB`] of the loudest
//! there are counted; with [`MUD_TRACKS`] or more, how much the mix's low
//! middle stands over its presence (1 to 4 kHz, per octave so a balanced
//! sound reads 0 dB) says how bad: nothing at [`MUD_FROM_DB`], fully at
//! [`MUD_FULL_DB`].
//!
//! **Stretches.** Each measure is smoothed over a short time; where the
//! smoothed value is over a threshold the frames make a stretch,
//! stretches with a short gap between are joined, and short ones are left
//! out, so a word or a hit alone never makes a finding.

use super::bands::{self, CENTRES, COUNT, IMPORTANCE, MUD, PRESENCE, SPEECH, db};
use super::fix::{self, Fix, Remedy, called};
use super::listen::HOP;
use super::{Finding, Problem};
use project::{SequenceId, Time, Track, TrackId};
use std::ops::Range;

/// Dialogue quieter than this in the speech bands (dBFS) is not speaking.
pub const SPEECH_FLOOR_DB: f32 = -50.0;
/// A band of the voice quieter than this (dBFS) carries nothing to cover.
const BAND_FLOOR_DB: f32 = -80.0;
/// This far under the voice in a band, another track leaves it clear.
pub const CLEAR_DB: f32 = 6.0;
/// This far over the voice in a band, another track covers it.
pub const COVERED_DB: f32 = 6.0;
/// How many tracks make a pile-up.
pub const MUD_TRACKS: usize = 3;
/// A track counts in a pile-up within this of the loudest there.
pub const MUD_NEAR_DB: f32 = 10.0;
/// ... and over this (dBFS, between 200 and 500 Hz).
pub const MUD_FLOOR_DB: f32 = -50.0;
/// The low middle this far over the presence (per octave) starts to be
/// muddy...
pub const MUD_FROM_DB: f32 = 3.0;
/// ... and this far is as muddy as it gets.
pub const MUD_FULL_DB: f32 = 12.0;

/// How a measure becomes stretches, in seconds and its own units.
struct Stretching {
    smooth_s: f64,
    threshold: f32,
    gap_s: f64,
    shortest_s: f64,
}

const COVERS: Stretching = Stretching {
    smooth_s: 0.3,
    threshold: 0.3,
    gap_s: 0.5,
    shortest_s: 0.4,
};

const MUDDY: Stretching = Stretching {
    smooth_s: 0.5,
    threshold: 0.25,
    gap_s: 0.5,
    shortest_s: 0.5,
};

/// From this severity on, the words say it plainly.
const PLAIN: f32 = 0.6;

/// A track other than dialogue, frame by frame.
struct Masker {
    /// Index into the tracks.
    track: usize,
    severity: Vec<f32>,
    /// How covered each speech band is, in 255ths, weighted by importance.
    bands: Vec<[u8; SPEECH.end - SPEECH.start]>,
}

/// Collects the measures frame by frame.
pub struct Judge {
    rate: u32,
    from: u64,
    tracks: Vec<Track>,
    key: Vec<bool>,
    /// The dialogue's level in the speech bands per frame (dBFS), `None`
    /// where it doesn't speak.
    speech_db: Vec<Option<f32>>,
    maskers: Vec<Masker>,
    mud: Vec<f32>,
    /// Each track's power between 200 and 500 Hz per frame.
    low_middle: Vec<Vec<f32>>,
    sums: Vec<[f64; COUNT]>,
    frames: usize,
}

impl std::fmt::Debug for Judge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Judge({} tracks, {} frames)", self.tracks.len(), self.frames)
    }
}

impl Judge {
    /// For `tracks` in the order frames list them; frame 0 stands for
    /// timeline sample `from` on, at `rate`.
    pub fn new(tracks: Vec<Track>, rate: u32, from: u64) -> Judge {
        let key: Vec<bool> = tracks
            .iter()
            .map(|t| t.sound.role == project::TrackRole::Dialogue)
            .collect();
        let has_dialogue = key.iter().any(|k| *k);
        let maskers = key
            .iter()
            .enumerate()
            .filter(|(_, k)| has_dialogue && !**k)
            .map(|(track, _)| Masker {
                track,
                severity: Vec::new(),
                bands: Vec::new(),
            })
            .collect();
        Judge {
            rate,
            from,
            low_middle: vec![Vec::new(); tracks.len()],
            sums: vec![[0.0; COUNT]; tracks.len()],
            tracks,
            key,
            speech_db: Vec::new(),
            maskers,
            mud: Vec::new(),
            frames: 0,
        }
    }

    /// Takes one frame: each track's power per band.
    pub fn add(&mut self, frame: &[[f32; COUNT]]) {
        self.frames += 1;
        for (sum, track) in self.sums.iter_mut().zip(frame) {
            for (s, p) in sum.iter_mut().zip(track) {
                *s += f64::from(*p);
            }
        }
        let mut voice = [0f32; COUNT];
        for (bands, _) in frame.iter().zip(&self.key).filter(|(_, k)| **k) {
            for (v, p) in voice.iter_mut().zip(bands) {
                *v += p;
            }
        }
        let speech = db(voice[SPEECH].iter().sum());
        let speaking = speech >= SPEECH_FLOOR_DB;
        self.speech_db.push(speaking.then_some(speech));
        for m in &mut self.maskers {
            let (severity, bands) = match frame.get(m.track) {
                Some(other) if speaking => covered(&voice, other),
                _ => (0.0, [0; SPEECH.end - SPEECH.start]),
            };
            m.severity.push(severity);
            m.bands.push(bands);
        }
        self.mud.push(muddy(frame));
        for (low, bands) in self.low_middle.iter_mut().zip(frame) {
            low.push(bands[MUD].iter().sum());
        }
    }

    /// Each track's mean level per band over every frame (dBFS).
    pub fn levels(&self) -> Vec<(TrackId, [f32; COUNT])> {
        let n = self.frames.max(1) as f64;
        self.tracks
            .iter()
            .zip(&self.sums)
            .map(|(t, sums)| (t.id, sums.map(|s| db((s / n) as f32))))
            .collect()
    }

    /// The findings, worst first.
    pub fn findings(&self, sequence: SequenceId) -> Vec<Finding> {
        let mut out = Vec::new();
        for m in &self.maskers {
            for run in self.stretches(&m.severity, &COVERS) {
                if let Some(f) = self.covering(sequence, m, run) {
                    out.push(f);
                }
            }
        }
        for run in self.stretches(&self.mud, &MUDDY) {
            if let Some(f) = self.muddy_finding(sequence, run) {
                out.push(f);
            }
        }
        out.sort_by(|a, b| b.severity.total_cmp(&a.severity));
        out
    }

    fn frames_in(&self, seconds: f64) -> usize {
        ((seconds * f64::from(self.rate) / HOP as f64).round() as usize).max(1)
    }

    fn stretches(&self, values: &[f32], how: &Stretching) -> Vec<Range<usize>> {
        runs(
            values,
            self.frames_in(how.smooth_s),
            how.threshold,
            self.frames_in(how.gap_s),
            self.frames_in(how.shortest_s),
        )
    }

    fn time(&self, frame: usize) -> Time {
        let sample = self.from + (frame * HOP) as u64;
        Time::from_ratio(sample as i64, u64::from(self.rate)).unwrap_or(Time::ZERO)
    }

    /// How loud the dialogue speaks: the middle of its speaking frames.
    fn speaking_db(&self) -> Option<f32> {
        let mut levels: Vec<f32> = self.speech_db.iter().flatten().copied().collect();
        levels.sort_by(f32::total_cmp);
        levels.get(levels.len() / 2).copied()
    }

    fn covering(&self, sequence: SequenceId, m: &Masker, run: Range<usize>) -> Option<Finding> {
        let speaking: Vec<usize> = run.clone().filter(|i| self.speech_db[*i].is_some()).collect();
        if speaking.is_empty() {
            return None;
        }
        let severity = speaking.iter().map(|i| m.severity[*i]).sum::<f32>() / speaking.len() as f32;
        let mut per_band = [0f32; SPEECH.end - SPEECH.start];
        for i in &speaking {
            for (sum, b) in per_band.iter_mut().zip(&m.bands[*i]) {
                *sum += f32::from(*b);
            }
        }
        let (first, last) = widest(&per_band);
        let (first, last) = (SPEECH.start + first, SPEECH.start + last);
        let track = &self.tracks[m.track];
        let words = format!(
            "{} covers the voice {}: {}.",
            capital(&called(track)),
            between(first, last),
            if severity >= PLAIN {
                "words may be hard to follow"
            } else {
                "the voice may sound less clear"
            }
        );
        let mut fixes = Vec::new();
        let d = &track.sound.ducking;
        if !(d.on && d.depth_mdb <= fix::DEEPEST_DUCK_MDB) {
            let threshold = self
                .speaking_db()
                .map_or(-40.0, |l| (l - 15.0).clamp(-60.0, -25.0));
            fixes.push(Fix::new(
                sequence,
                track,
                Remedy::LowerUnderDialogue {
                    threshold_mdb: (threshold * 1_000.0).round() as i32,
                },
            ));
        }
        // 1.6 to 3.15 kHz: where the dip for the voice works.
        if first <= 13 && last >= 10 && fix::dip_fits(&track.sound, fix::ROOM_HZ) {
            fixes.push(Fix::new(sequence, track, Remedy::MakeRoomForVoice));
        }
        // Up to 630 Hz: the low middle.
        if first <= 6 && fix::dip_fits(&track.sound, fix::CUT_HZ) {
            fixes.push(Fix::new(sequence, track, Remedy::CutLowMiddle));
        }
        Some(Finding {
            problem: Problem::CoversVoice,
            range: (self.time(run.start), self.time(run.end)),
            tracks: vec![track.id],
            band_hz: (CENTRES[first] as u32, CENTRES[last] as u32),
            severity,
            words,
            fixes,
        })
    }

    fn muddy_finding(&self, sequence: SequenceId, run: Range<usize>) -> Option<Finding> {
        let severity = self.mud[run.clone()].iter().sum::<f32>() / run.len().max(1) as f32;
        let mean: Vec<f32> = self
            .low_middle
            .iter()
            .map(|low| low[run.clone()].iter().sum::<f32>() / run.len().max(1) as f32)
            .collect();
        let loudest = mean.iter().copied().fold(0f32, f32::max);
        let mut involved: Vec<usize> = (0..mean.len())
            .filter(|i| db(mean[*i]) >= MUD_FLOOR_DB && db(mean[*i]) >= db(loudest) - MUD_NEAR_DB)
            .collect();
        involved.sort_by(|a, b| mean[*b].total_cmp(&mean[*a]));
        if involved.len() < 2 {
            return None;
        }
        let names: Vec<String> = involved.iter().map(|i| called(&self.tracks[*i])).collect();
        let words = format!(
            "{} pile up between 200 and 500 Hz: {}.",
            capital(&listed(&names)),
            if severity >= PLAIN {
                "the mix sounds muddy"
            } else {
                "the mix may sound a little muddy"
            }
        );
        let fixes = involved
            .iter()
            .map(|i| &self.tracks[*i])
            .filter(|t| t.sound.role != project::TrackRole::Dialogue && fix::dip_fits(&t.sound, fix::CUT_HZ))
            .take(3)
            .map(|t| Fix::new(sequence, t, Remedy::CutLowMiddle))
            .collect();
        Some(Finding {
            problem: Problem::Muddy,
            range: (self.time(run.start), self.time(run.end)),
            tracks: involved.iter().map(|i| self.tracks[*i].id).collect(),
            band_hz: (200, 500),
            severity,
            words,
            fixes,
        })
    }
}

/// How much `other` covers `voice` in this frame, and per speech band (in
/// 255ths, weighted by importance).
fn covered(voice: &[f32; COUNT], other: &[f32; COUNT]) -> (f32, [u8; SPEECH.end - SPEECH.start]) {
    let mut bands = [0u8; SPEECH.end - SPEECH.start];
    let (mut weight, mut sum) = (0f32, 0f32);
    for (b, k) in bands.iter_mut().zip(SPEECH) {
        let v = db(voice[k]);
        if v < BAND_FLOOR_DB {
            continue;
        }
        let margin = v - db(other[k]);
        let share = ((CLEAR_DB - margin) / (CLEAR_DB + COVERED_DB)).clamp(0.0, 1.0);
        weight += IMPORTANCE[k];
        sum += IMPORTANCE[k] * share;
        // The largest importance is under 0.1: scaled so it fits.
        *b = (share * IMPORTANCE[k] * 2_550.0).round().min(255.0) as u8;
    }
    let severity = if weight > 0.0 { sum / weight } else { 0.0 };
    (severity, bands)
}

/// How muddy one frame is: see the module notes.
fn muddy(frame: &[[f32; COUNT]]) -> f32 {
    let low: Vec<f32> = frame.iter().map(|b| b[MUD].iter().sum()).collect();
    let loudest = low.iter().copied().fold(0f32, f32::max);
    let piled = low
        .iter()
        .filter(|p| db(**p) >= MUD_FLOOR_DB && db(**p) >= db(loudest) - MUD_NEAR_DB)
        .count();
    if piled < MUD_TRACKS {
        return 0.0;
    }
    let mix_low: f32 = low.iter().sum();
    let mix_presence: f32 = frame.iter().map(|b| b[PRESENCE].iter().sum::<f32>()).sum();
    let over = db(mix_low / bands::octaves(&MUD)) - db(mix_presence / bands::octaves(&PRESENCE));
    ((over - MUD_FROM_DB) / (MUD_FULL_DB - MUD_FROM_DB)).clamp(0.0, 1.0)
}

/// Stretches where `values`, averaged over `smooth` frames around each,
/// reach `threshold`; joined across gaps of up to `gap` frames, and only
/// those at least `shortest` frames long.
pub fn runs(values: &[f32], smooth: usize, threshold: f32, gap: usize, shortest: usize) -> Vec<Range<usize>> {
    let mut prefix = Vec::with_capacity(values.len() + 1);
    prefix.push(0f64);
    for v in values {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + f64::from(*v));
    }
    let half = smooth / 2;
    let mut out: Vec<Range<usize>> = Vec::new();
    for i in 0..values.len() {
        let (a, b) = (i.saturating_sub(half), (i + half + 1).min(values.len()));
        let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
        if mean < f64::from(threshold) {
            continue;
        }
        match out.last_mut() {
            Some(last) if i - last.end <= gap => last.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out.retain(|r| r.len() >= shortest);
    out
}

/// The bands around the worst one that are at least half as bad, as the
/// first and last index.
fn widest(per_band: &[f32]) -> (usize, usize) {
    let peak = (0..per_band.len())
        .max_by(|a, b| per_band[*a].total_cmp(&per_band[*b]))
        .unwrap_or(0);
    let half = per_band.get(peak).copied().unwrap_or(0.0) * 0.5;
    let mut first = peak;
    while first > 0 && per_band[first - 1] >= half {
        first -= 1;
    }
    let mut last = peak;
    while last + 1 < per_band.len() && per_band[last + 1] >= half {
        last += 1;
    }
    (first, last)
}

/// "between 1 and 3.2 kHz", "between 315 and 630 Hz", "around 2.5 kHz".
fn between(first: usize, last: usize) -> String {
    let (a, b) = (CENTRES[first], CENTRES[last]);
    let khz = |hz: f32| {
        let k = (hz / 100.0).round() / 10.0;
        if k.fract() == 0.0 {
            format!("{k:.0}")
        } else {
            format!("{k:.1}")
        }
    };
    if first == last {
        if a >= 1_000.0 {
            format!("around {} kHz", khz(a))
        } else {
            format!("around {a:.0} Hz")
        }
    } else if a >= 1_000.0 {
        format!("between {} and {} kHz", khz(a), khz(b))
    } else if b < 1_000.0 {
        format!("between {a:.0} and {b:.0} Hz")
    } else {
        format!("between {a:.0} Hz and {} kHz", khz(b))
    }
}

/// "A, B and C".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capital(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_blips_are_left_out_and_near_stretches_joined() {
        let mut v = vec![0f32; 100];
        v[10] = 1.0;
        for x in &mut v[30..45] {
            *x = 1.0;
        }
        for x in &mut v[48..60] {
            *x = 1.0;
        }
        assert_eq!(runs(&v, 1, 0.5, 5, 5), vec![30..60]);
    }

    #[test]
    fn bands_are_said_in_plain_numbers() {
        assert_eq!(between(8, 13), "between 1 and 3.2 kHz");
        assert_eq!(between(3, 6), "between 315 and 630 Hz");
        assert_eq!(between(5, 11), "between 500 Hz and 2 kHz");
        assert_eq!(between(12, 12), "around 2.5 kHz");
        assert_eq!(listed(&["a".into(), "b".into(), "c".into()]), "a, b and c");
    }

    #[test]
    fn a_track_level_with_the_voice_covers_half() {
        let voice = [1e-3f32; COUNT];
        let (same, _) = covered(&voice, &voice);
        assert!((same - 0.5).abs() < 1e-3);
        let (under, _) = covered(&voice, &voice.map(|p| p * 1e-2));
        assert_eq!(under, 0.0);
        let (over, _) = covered(&voice, &voice.map(|p| p * 10.0));
        assert!((over - 1.0).abs() < 1e-3);
    }
}
