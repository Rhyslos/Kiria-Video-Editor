//! How the full loudness overview counts integrated loudness, loudness
//! range and peaks: from readings of the meters taken every 100 ms of the
//! timeline while playing (`audio::dsp::loudness::Tally`), started again on
//! every play (as the master column does), kept until reset (adding up
//! over several plays), or only inside the region (started again when the
//! region changes).

use audio::dsp::loudness::Tally;
use project::{Region, Time, TrackId};
use serde::{Deserialize, Serialize};

/// How integrated loudness, range and peaks are counted. A setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Counting {
    /// Start again on every play (as on the Edit page).
    #[default]
    EveryPlay,
    /// Keep adding up over several plays until Reset.
    UntilReset,
    /// Only what plays inside the region; start again when it changes.
    Region,
}

impl Counting {
    pub const ALL: [Counting; 3] = [Counting::EveryPlay, Counting::UntilReset, Counting::Region];

    pub fn label(self) -> &'static str {
        match self {
            Counting::EveryPlay => "Every play",
            Counting::UntilReset => "Until reset",
            Counting::Region => "In the region",
        }
    }

    pub fn explained(self) -> &'static str {
        match self {
            Counting::EveryPlay => {
                "Integrated loudness, range and peaks start again each time playback starts."
            }
            Counting::UntilReset => "They keep adding up over every play until Reset is clicked.",
            Counting::Region => {
                "Only what plays inside the region counts; they start again when the region changes."
            }
        }
    }
}

/// One look at a meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub momentary: f32,
    pub short_term: f32,
    /// The highest peak since the last look, dB (true peak for the mix,
    /// sample peak for a track).
    pub peak_db: f32,
}

/// Seconds between readings.
const STEP: f64 = 0.1;
/// A move of the playhead further than this from where counting expected
/// it is a jump: counting goes on from there.
const JUMP: f64 = 0.6;
/// The most readings taken in one frame (a slow frame catches up so far).
const CATCH_UP: usize = 5;

/// What the overview has counted, for the mix and the track the tools are on.
#[derive(Debug, Clone)]
pub struct Counted {
    pub mix: Tally,
    pub track: Tally,
    track_id: Option<TrackId>,
    /// The timeline moment of the next reading.
    next: Option<f64>,
    was_playing: bool,
    region: Option<Region>,
    /// The highest peaks since the last reading.
    peaks: (f32, f32),
}

impl Default for Counted {
    fn default() -> Counted {
        Counted {
            mix: Tally::new(),
            track: Tally::new(),
            track_id: None,
            next: None,
            was_playing: false,
            region: None,
            peaks: (f32::NEG_INFINITY, f32::NEG_INFINITY),
        }
    }
}

/// What a frame hands the overview.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub counting: Counting,
    pub playing: bool,
    pub at: Time,
    pub region: Option<Region>,
    pub track: Option<TrackId>,
    pub mix: Option<Reading>,
    pub track_reading: Option<Reading>,
}

impl Counted {
    /// Starts everything again (the Reset button).
    pub fn reset(&mut self) {
        self.mix.clear();
        self.track.clear();
        self.peaks = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    }

    /// Seconds of sound counted (the mix's).
    pub fn seconds(&self) -> f64 {
        self.mix.readings() as f64 * STEP
    }

    /// Takes this frame's readings.
    pub fn follow(&mut self, f: Frame) {
        if f.track != self.track_id {
            self.track.clear();
            self.track_id = f.track;
        }
        let starting = f.playing && !self.was_playing;
        self.was_playing = f.playing;
        if starting && f.counting == Counting::EveryPlay {
            self.reset();
        }
        if f.counting == Counting::Region && f.region != self.region {
            self.reset();
        }
        self.region = f.region;
        if !f.playing {
            self.next = None;
            return;
        }
        if let Some(m) = f.mix {
            self.peaks.0 = self.peaks.0.max(m.peak_db);
        }
        if let Some(t) = f.track_reading {
            self.peaks.1 = self.peaks.1.max(t.peak_db);
        }
        let at = f.at.as_secs_f64();
        let next = match self.next {
            Some(next) if (at - next).abs() <= JUMP => next,
            // Started, or jumped: count on from here.
            _ => at + STEP,
        };
        let mut next = next;
        let mut taken = 0;
        while at >= next && taken < CATCH_UP {
            let inside = match f.counting {
                Counting::Region => f.region.is_some_and(|r| {
                    let t = next - STEP / 2.0;
                    r.start.as_secs_f64() <= t && t < r.end.as_secs_f64()
                }),
                _ => true,
            };
            if inside {
                if let Some(m) = f.mix {
                    self.mix.add(m.momentary, m.short_term, self.peaks.0);
                }
                if let Some(t) = f.track_reading {
                    self.track.add(t.momentary, t.short_term, self.peaks.1);
                }
                self.peaks = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            }
            next += STEP;
            taken += 1;
        }
        if at >= next {
            next = at + STEP;
        }
        self.next = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(lufs: f32) -> Option<Reading> {
        Some(Reading {
            momentary: lufs,
            short_term: lufs,
            peak_db: lufs + 10.0,
        })
    }

    fn play(
        counted: &mut Counted,
        counting: Counting,
        from: f64,
        seconds: f64,
        lufs: f32,
        region: Option<Region>,
    ) {
        let frames = (seconds * 60.0) as usize;
        for i in 0..=frames {
            let at = Time::from_ratio(((from + i as f64 / 60.0) * 1_000.0) as i64, 1_000).unwrap();
            counted.follow(Frame {
                counting,
                playing: true,
                at,
                region,
                track: Some(TrackId(1)),
                mix: reading(lufs),
                track_reading: reading(lufs - 6.0),
            });
        }
        counted.follow(Frame {
            counting,
            playing: false,
            at: Time::ZERO,
            region,
            track: Some(TrackId(1)),
            mix: None,
            track_reading: None,
        });
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn every_play_starts_again_and_until_reset_adds_up() {
        let mut each = Counted::default();
        play(&mut each, Counting::EveryPlay, 0.0, 5.0, -20.0, None);
        assert!((each.seconds() - 5.0).abs() < 0.15, "{}", each.seconds());
        assert!(near(each.mix.integrated(), -20.0));
        assert!(near(each.track.integrated(), -26.0));
        assert!(near(each.mix.peak(), -10.0));
        play(&mut each, Counting::EveryPlay, 0.0, 5.0, -30.0, None);
        assert!(near(each.mix.integrated(), -30.0), "only the last play");

        let mut kept = Counted::default();
        play(&mut kept, Counting::UntilReset, 0.0, 5.0, -20.0, None);
        play(&mut kept, Counting::UntilReset, 0.0, 5.0, -20.0, None);
        assert!((kept.seconds() - 10.0).abs() < 0.3, "{}", kept.seconds());
        kept.reset();
        assert_eq!(kept.mix.readings(), 0);
    }

    #[test]
    fn the_region_counts_only_inside_and_starts_again_when_it_changes() {
        let region = Region::between(Time::seconds(2), Time::seconds(4));
        let mut counted = Counted::default();
        play(&mut counted, Counting::Region, 0.0, 6.0, -20.0, region);
        assert!((counted.seconds() - 2.0).abs() < 0.15, "{}", counted.seconds());
        let other = Region::between(Time::seconds(1), Time::seconds(2));
        play(&mut counted, Counting::Region, 0.0, 3.0, -20.0, other);
        assert!(
            (counted.seconds() - 1.0).abs() < 0.15,
            "started again: {}",
            counted.seconds()
        );
    }

    #[test]
    fn another_track_starts_its_count_again() {
        let mut counted = Counted::default();
        play(&mut counted, Counting::UntilReset, 0.0, 2.0, -20.0, None);
        counted.follow(Frame {
            counting: Counting::UntilReset,
            playing: false,
            at: Time::ZERO,
            region: None,
            track: Some(TrackId(2)),
            mix: None,
            track_reading: None,
        });
        assert_eq!(counted.track.readings(), 0);
        assert!(counted.mix.readings() > 0, "the mix keeps its count");
    }
}
