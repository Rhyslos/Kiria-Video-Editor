//! The band across the middle of the Audio page and what it shows: the
//! clarity check, or one of four live pictures of the sound while it
//! plays. `View` (a setting) picks one; `Band` keeps what the pictures
//! draw from, gathered every frame from what the mixer measures (cheap:
//! a few numbers and one spectrum a frame, nothing waits).
//!
//! - Spectrum: the mix's (or the chosen track's) levels by frequency,
//!   with the highest held for a moment, and the last few seconds as a
//!   spectrogram beside it.
//! - Loudness: the mix's momentary and short-term loudness over the last
//!   30 seconds of playing, against the loudness aimed for, with true peaks
//!   over the ceiling marked.
//! - Tracks: a lane per track of how loud it was over the last 15 seconds,
//!   in its role's colour: who speaks when, and what plays under them.
//! - Stereo: left against right as a picture (straight up is mono, flat is
//!   out of phase), how alike the channels are and which side is louder,
//!   in plain words.

use super::{Live, Tool};
use audio::Stereo;
use audio::dsp::bands::COUNT;
use project::{TrackId, TrackRole};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// What the band shows. A setting, the same for every project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum View {
    #[default]
    Clarity,
    Spectrum,
    Loudness,
    Tracks,
    Stereo,
}

impl View {
    pub const ALL: [View; 5] = [
        View::Clarity,
        View::Spectrum,
        View::Loudness,
        View::Tracks,
        View::Stereo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Clarity => "Clarity",
            View::Spectrum => "Spectrum",
            View::Loudness => "Loudness",
            View::Tracks => "Tracks",
            View::Stereo => "Stereo",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            View::Clarity => "Where other tracks cover the voice, with fixes",
            View::Spectrum => "The sound by frequency now, and the last few seconds as a spectrogram",
            View::Loudness => "The mix's loudness over the last 30 seconds of playing, against your aim",
            View::Tracks => "How loud each track was over the last 15 seconds: who speaks when",
            View::Stereo => "Left against right: how wide the mix is, and whether it holds up on one speaker",
        }
    }

    /// `clarity`, `spectrum`, `loudness`, `tracks` or `stereo`.
    pub fn from_name(name: &str) -> Option<View> {
        View::ALL
            .into_iter()
            .find(|v| v.label().eq_ignore_ascii_case(name))
    }

    /// Whether it draws from the analyser.
    fn analyses(self) -> bool {
        matches!(self, View::Spectrum | View::Stereo)
    }
}

/// Whose sound Spectrum and Stereo show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The mix, as it goes out.
    #[default]
    Mix,
    /// The track chosen on the page (after its EQ).
    Track,
}

/// What the analyser should listen to: the EQ tool always hears its
/// track; otherwise the band's Spectrum and Stereo say; nothing else needs
/// it.
pub fn listen(tool: Tool, view: View, source: Source, track: Option<TrackId>) -> Option<engine::Listen> {
    if tool == Tool::Eq {
        return track.map(engine::Listen::Track);
    }
    if !view.analyses() {
        return None;
    }
    match source {
        Source::Mix => Some(engine::Listen::Mix),
        Source::Track => track.map(engine::Listen::Track),
    }
}

/// Seconds of loudness the Loudness view keeps.
pub const LOUDNESS_SECONDS: f64 = 30.0;
/// Seconds of each track the Tracks view keeps.
pub const TRACK_SECONDS: f64 = 15.0;
/// How often (seconds of the timeline) a reading is kept.
pub const EVERY: f64 = 0.1;
/// Spectrogram columns kept (one per analysed window: about 4 s at 48 kHz).
pub const COLUMNS: usize = 200;
/// How fast the spectrum's held highs fall, dB a second.
const HOLD_FALL_DB: f32 = 12.0;
/// How long a high is held before it falls, seconds.
const HOLD_SECONDS: f32 = 1.0;

/// One reading of the mix's loudness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The timeline's time, seconds.
    pub at: f64,
    pub momentary: f32,
    pub short_term: f32,
    /// The higher channel's true peak in the reading's stretch, dBTP.
    pub true_peak: f32,
}

/// A track's loudness at a moment, with its role for the colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackReading {
    pub at: f64,
    pub momentary: f32,
    /// How far ducking turned it down then, dB (positive).
    pub ducked: f32,
}

/// What the band's pictures draw from.
#[derive(Debug, Default)]
pub struct Band {
    pub source: Source,
    /// Spectrum: levels now, the highest held, how long each is held yet.
    pub levels: Option<[f32; COUNT]>,
    pub held: Option<[f32; COUNT]>,
    hold_left: [f32; COUNT],
    /// The spectrogram, oldest first, and whether it changed since drawn.
    pub columns: VecDeque<[f32; COUNT]>,
    pub columns_changed: bool,
    windows: u32,
    /// Stereo: the newest picture and the three before it (oldest first).
    pub stereo: VecDeque<Stereo>,
    /// Correlation, smoothed so its needle doesn't jitter.
    pub correlation: Option<f32>,
    pub balance: f32,
    /// Loudness: the mix's readings, oldest first.
    pub loudness: VecDeque<Reading>,
    /// The highest true peak since the last kept reading.
    peak_since: f32,
    /// Tracks: each one's readings, oldest first, and its role.
    pub tracks: HashMap<TrackId, (TrackRole, VecDeque<TrackReading>)>,
    /// Where the timeline was at the last frame followed (a jump back or
    /// far ahead starts the histories again).
    last_at: Option<f64>,
    /// The playhead's place now, seconds.
    pub now: f64,
    pub playing: bool,
}

impl Band {
    /// Every frame on the Audio page: takes what was measured. `at` is the
    /// playhead's place (seconds); `dt` the seconds since the last frame;
    /// `stereo` the analyser's picture, when it listens.
    pub fn follow(
        &mut self,
        live: &Live,
        stereo: Option<Stereo>,
        roles: &[(TrackId, TrackRole)],
        at: f64,
        dt: f32,
    ) {
        self.playing = live.playing;
        self.now = at;
        self.follow_spectrum(live.spectrum, dt);
        if let Some(stereo) = stereo {
            self.follow_stereo(stereo);
        }
        if !live.playing {
            self.last_at = None;
            return;
        }
        // A jump (a loop, a click on the timeline) starts afresh: the
        // pictures are of what was just heard, in order.
        let jumped = self
            .last_at
            .is_some_and(|last| at < last - 0.05 || at > last + 1.0);
        if jumped {
            self.loudness.clear();
            self.tracks.clear();
        }
        self.last_at = Some(at);
        if let Some(m) = live.master {
            self.peak_since = self.peak_since.max(m.true_peak_db[0].max(m.true_peak_db[1]));
            if self.loudness.back().is_none_or(|r| at - r.at >= EVERY) {
                self.loudness.push_back(Reading {
                    at,
                    momentary: m.momentary,
                    short_term: m.short_term,
                    true_peak: self.peak_since,
                });
                self.peak_since = f32::NEG_INFINITY;
            }
            keep_since(&mut self.loudness, at - LOUDNESS_SECONDS, |r| r.at);
        }
        for (id, role) in roles {
            let Some(levels) = live.tracks.get(id) else { continue };
            let (kept_role, readings) = self
                .tracks
                .entry(*id)
                .or_insert_with(|| (*role, VecDeque::new()));
            *kept_role = *role;
            if readings.back().is_none_or(|r| at - r.at >= EVERY) {
                readings.push_back(TrackReading {
                    at,
                    momentary: levels.momentary,
                    ducked: -levels.ducked_db.min(0.0),
                });
            }
            keep_since(readings, at - TRACK_SECONDS, |r| r.at);
        }
        self.tracks.retain(|id, _| roles.iter().any(|(t, _)| t == id));
    }

    fn follow_spectrum(&mut self, levels: Option<[f32; COUNT]>, dt: f32) {
        self.levels = levels;
        let Some(levels) = levels else { return };
        let held = self.held.get_or_insert(levels);
        for i in 0..COUNT {
            if levels[i] >= held[i] {
                held[i] = levels[i];
                self.hold_left[i] = HOLD_SECONDS;
            } else if self.hold_left[i] > 0.0 {
                self.hold_left[i] -= dt;
            } else {
                held[i] = (held[i] - HOLD_FALL_DB * dt).max(levels[i]);
            }
        }
    }

    fn follow_stereo(&mut self, stereo: Stereo) {
        if stereo.windows == self.windows {
            return;
        }
        self.windows = stereo.windows;
        if let Some(levels) = self.levels {
            self.columns.push_back(levels);
            while self.columns.len() > COLUMNS {
                self.columns.pop_front();
            }
            self.columns_changed = true;
        }
        if stereo.correlation.is_finite() {
            let c = self.correlation.unwrap_or(stereo.correlation);
            self.correlation = Some(c + (stereo.correlation - c) * 0.2);
            self.balance += (stereo.balance - self.balance) * 0.2;
        }
        self.stereo.push_back(stereo);
        while self.stereo.len() > 4 {
            self.stereo.pop_front();
        }
    }
}

/// Drops readings from before `from`.
fn keep_since<T>(readings: &mut VecDeque<T>, from: f64, at: impl Fn(&T) -> f64) {
    while readings.front().is_some_and(|r| at(r) < from) {
        readings.pop_front();
    }
}

/// What the stereo picture means, in plain words.
pub fn stereo_words(correlation: Option<f32>, balance: f32) -> String {
    let Some(c) = correlation else {
        return "Play to see how wide the sound is.".into();
    };
    let width = if c > 0.9 {
        "Close to mono: it sounds the same on one speaker."
    } else if c > 0.3 {
        "Stereo, and safe on one speaker."
    } else if c > -0.1 {
        "Very wide: some of it gets quieter on one speaker."
    } else {
        "Out of phase: parts cancel on one speaker, as on a phone."
    };
    let side = if balance < -0.3 {
        " Leans left."
    } else if balance > 0.3 {
        " Leans right."
    } else {
        ""
    };
    format!("{width}{side}")
}

/// A point of the stereo picture: left and right turned so mono is
/// straight up and opposite channels lie flat, as shares of the picture's
/// half-width (`scale` makes the loudest point reach the edge).
pub fn turned(l: f32, r: f32, scale: f32) -> (f32, f32) {
    let k = std::f32::consts::FRAC_1_SQRT_2 * scale;
    ((r - l) * k, (l + r) * k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::TrackLevels;

    fn live(at_loudness: f32) -> Live {
        let mut live = Live {
            playing: true,
            ..Live::default()
        };
        live.master = Some(audio::dsp::loudness::Measured {
            momentary: at_loudness,
            short_term: at_loudness - 1.0,
            true_peak_db: [-3.0, -0.5],
            ..audio::dsp::loudness::Measured::SILENT
        });
        live.tracks.insert(
            TrackId(4),
            TrackLevels {
                peak_db: [-10.0; 2],
                momentary: at_loudness - 3.0,
                short_term: at_loudness - 3.0,
                compressor_db: 0.0,
                cleanup_db: 0.0,
                ducked_db: -6.0,
                level_db: 0.0,
            },
        );
        live
    }

    #[test]
    fn the_analyser_hears_the_eqs_track_else_what_the_band_shows() {
        let t = Some(TrackId(2));
        assert_eq!(
            listen(Tool::Eq, View::Spectrum, Source::Mix, t),
            Some(engine::Listen::Track(TrackId(2)))
        );
        assert_eq!(
            listen(Tool::Compressor, View::Spectrum, Source::Mix, t),
            Some(engine::Listen::Mix)
        );
        assert_eq!(
            listen(Tool::Compressor, View::Stereo, Source::Track, t),
            Some(engine::Listen::Track(TrackId(2)))
        );
        assert_eq!(listen(Tool::Compressor, View::Loudness, Source::Mix, t), None);
        assert_eq!(listen(Tool::Compressor, View::Clarity, Source::Mix, t), None);
    }

    #[test]
    fn readings_are_kept_every_tenth_for_their_stretch_and_a_jump_starts_again() {
        let mut band = Band::default();
        let roles = [(TrackId(4), TrackRole::Dialogue)];
        let mut at = 0.0;
        for i in 0..2_000 {
            band.follow(&live(-20.0 + (i % 3) as f32), None, &roles, at, 1.0 / 60.0);
            at += 1.0 / 60.0;
        }
        // About 33 s played: the last 30 kept, one every tenth.
        let first = band.loudness.front().expect("readings").at;
        assert!(at - first <= LOUDNESS_SECONDS + 0.01, "{first}");
        assert!((290..=302).contains(&band.loudness.len()), "{}", band.loudness.len());
        let (role, track) = &band.tracks[&TrackId(4)];
        assert_eq!(*role, TrackRole::Dialogue);
        assert!(track.len() <= 151 && track.iter().all(|r| r.ducked == 6.0));
        assert!(band.loudness.iter().all(|r| r.true_peak == -0.5), "the higher channel");
        // A loop back to the start: what follows is a new picture.
        band.follow(&live(-20.0), None, &roles, 0.0, 1.0 / 60.0);
        assert_eq!(band.loudness.len(), 1);
        // A track taken away goes from the lanes.
        band.follow(&live(-20.0), None, &[], 0.02, 1.0 / 60.0);
        assert!(band.tracks.is_empty());
    }

    #[test]
    fn a_high_is_held_then_falls_and_each_window_adds_a_column() {
        let mut band = Band::default();
        let mut loud = [-90.0; COUNT];
        loud[10] = -6.0;
        let quiet = [-90.0; COUNT];
        let picture = |windows| Stereo {
            correlation: 1.0,
            balance: 0.0,
            windows,
            points: vec![(0.1, 0.1)],
        };
        let mut live = Live {
            spectrum: Some(loud),
            ..Live::default()
        };
        band.follow(&live, Some(picture(1)), &[], 0.0, 0.1);
        live.spectrum = Some(quiet);
        band.follow(&live, Some(picture(1)), &[], 0.0, 0.5);
        let held = band.held.expect("held");
        assert_eq!(held[10], -6.0, "held for a moment");
        for _ in 0..20 {
            band.follow(&live, Some(picture(2)), &[], 0.0, 0.1);
        }
        let held = band.held.expect("held");
        assert!(held[10] < -6.0 && held[10] >= -90.0, "then falls: {}", held[10]);
        assert_eq!(band.columns.len(), 2, "a column per new window, not per frame");
        assert_eq!(band.correlation, Some(1.0));
    }

    #[test]
    fn the_stereo_picture_is_turned_and_put_in_words() {
        let (x, y) = turned(0.5, 0.5, 1.0);
        assert!(x.abs() < 1e-6 && y > 0.0, "mono is straight up");
        let (x, y) = turned(0.5, -0.5, 1.0);
        assert!(y.abs() < 1e-6 && x < 0.0, "opposite lies flat");
        assert!(stereo_words(Some(0.97), 0.0).starts_with("Close to mono"));
        assert!(stereo_words(Some(-0.4), 0.0).starts_with("Out of phase"));
        assert!(stereo_words(Some(0.6), 0.5).ends_with("Leans right."));
        assert!(stereo_words(None, 0.0).starts_with("Play"));
    }

    #[test]
    fn views_are_found_by_name() {
        assert_eq!(View::from_name("stereo"), Some(View::Stereo));
        assert_eq!(View::from_name("nope"), None);
    }
}
