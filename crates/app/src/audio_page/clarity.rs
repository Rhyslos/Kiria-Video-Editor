//! The clarity check as the page runs it: which check is wanted (at the
//! playhead, or over the region once asked), started on the Analysis pool
//! when the playhead has settled and playback has stopped, cancelled when
//! what is wanted changes, and its findings kept for the band
//! (`clarity_draw.rs`) and the timeline's coloured stretches.
//!
//! Every frame costs a comparison of the open timeline's audio tracks (a
//! pointer comparison while nothing changed) and a poll; the check itself
//! never runs here.

use engine::clarity::{Clarity, Finding, Outcome, Problem, Report, Request};
use jobs::JobSystem;
use project::{Project, SequenceId, Time, Track};
use std::time::{Duration, Instant};
use ui_kit::TimelineSpan;
use ui_kit::theme::clarity as colours;

/// How long the playhead (or the tracks) must stay put before a check at
/// the playhead starts: a drag or a run of edits starts one check, at the end.
pub const SETTLE: Duration = Duration::from_millis(400);

/// What the band checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Around the playhead, again whenever it settles somewhere else.
    #[default]
    Playhead,
    /// Over the region, while there is one.
    Region,
}

/// A check as it was asked for: the stretch, the timeline, and the
/// tracks as they were (counted up whenever they change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    request: Request,
    sequence: SequenceId,
    tracks: u64,
}

/// The band's state, kept by the page.
#[derive(Debug, Default)]
pub struct ClarityBand {
    pub scope: Scope,
    wanted: Option<Wanted>,
    /// When `wanted` last changed.
    since: Option<Instant>,
    running: Option<(Clarity, Wanted)>,
    report: Option<(Report, Wanted)>,
    /// What the open timeline's audio tracks were, and how many times they
    /// have changed.
    tracks: Option<imbl::Vector<Track>>,
    changes: u64,
    /// The finding the band shows in its picture (index, worst first).
    pub chosen: usize,
}

/// A finding's colour.
pub fn colour(problem: Problem) -> egui::Color32 {
    match problem {
        Problem::CoversVoice => colours::COVERED,
        Problem::Muddy => colours::MUDDY,
    }
}

impl ClarityBand {
    /// Every frame on the Audio page: works out what is wanted, cancels a
    /// check that is not, takes a finished one, and starts the wanted one
    /// once things have settled. Returns a check that failed, explained.
    pub fn follow(
        &mut self,
        jobs: &JobSystem,
        project: &Project,
        playing: bool,
        playhead: Time,
        now: Instant,
    ) -> Option<diag::AppError> {
        let seq = project.active()?;
        let same = self
            .tracks
            .as_ref()
            .is_some_and(|t| t.ptr_eq(&seq.audio_tracks) || *t == seq.audio_tracks);
        if !same {
            self.tracks = Some(seq.audio_tracks.clone());
            self.changes += 1;
        }
        if self.scope == Scope::Region && seq.region.is_none() {
            self.scope = Scope::Playhead;
        }
        let request = match (self.scope, seq.region) {
            (Scope::Region, Some(r)) => Request::Region(r.start, r.end),
            _ => Request::Playhead(playhead),
        };
        let wanted = Wanted {
            request,
            sequence: seq.id,
            tracks: self.changes,
        };
        if self.wanted != Some(wanted) {
            self.wanted = Some(wanted);
            self.since = Some(now);
        }
        if let Some((check, asked)) = &self.running
            && *asked != wanted
        {
            check.cancel();
            self.running = None;
        }
        let mut failed = None;
        if let Some((check, asked)) = &self.running
            && let Some(outcome) = check.poll()
        {
            match outcome {
                Outcome::Done(report) => {
                    self.report = Some((report, *asked));
                    self.chosen = 0;
                }
                Outcome::Cancelled => {}
                Outcome::Failed(err) => failed = Some(err),
            }
            self.running = None;
        }
        let shown = self.report.as_ref().is_some_and(|(_, w)| *w == wanted);
        // At the playhead only once playback stops and the playhead rests;
        // a region asked for is checked at once.
        let waited = self.since.is_none_or(|s| now.duration_since(s) >= SETTLE);
        let go = match self.scope {
            Scope::Playhead => !playing && waited,
            Scope::Region => true,
        };
        if self.running.is_none() && !shown && failed.is_none() && go {
            let check = Clarity::start(jobs, project.clone(), seq.id, request);
            self.running = Some((check, wanted));
        }
        failed
    }

    /// Nothing more to do for now: the check wanted is shown (for
    /// screenshots).
    pub fn settled(&self) -> bool {
        self.running.is_none() && self.report.as_ref().map(|(_, w)| Some(*w)) == Some(self.wanted)
    }

    /// The share of the running check done, if one runs.
    pub fn progress(&self) -> Option<f32> {
        self.running.as_ref().map(|(c, _)| c.progress())
    }

    /// The last report, and whether it is the check wanted now (else a
    /// newer one is on its way or waits for the playhead to settle).
    pub fn report(&self) -> Option<(&Report, bool)> {
        self.report.as_ref().map(|(r, w)| (r, Some(*w) == self.wanted))
    }

    /// The findings to show, on the open timeline.
    pub fn findings(&self, sequence: SequenceId) -> &[Finding] {
        match &self.report {
            Some((r, _)) if r.sequence == sequence => &r.findings,
            _ => &[],
        }
    }

    /// The findings as coloured stretches for the shared timeline.
    pub fn spans(&self, sequence: SequenceId) -> Vec<TimelineSpan> {
        self.findings(sequence)
            .iter()
            .map(|f| TimelineSpan {
                start: f.range.0.as_secs_f64(),
                end: f.range.1.as_secs_f64(),
                colour: colour(f.problem),
                tracks: f.tracks.iter().map(|t| t.0).collect(),
                tip: f.words.clone(),
            })
            .collect()
    }

    /// Check again now (after a fix), without waiting for things to settle.
    pub fn again(&mut self) {
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Region, TrackKind};

    fn jobs() -> JobSystem {
        JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()))
    }

    /// An empty project with two audio tracks (nothing to hear: each check
    /// is quick and finds nothing).
    fn project() -> Project {
        let p = Project::new_automatic("T");
        let seq = p.active_sequence;
        Command::AddTrack {
            sequence: seq,
            kind: TrackKind::Audio,
            name: "A2".into(),
        }
        .apply(&p)
        .unwrap()
    }

    /// Follows until the band has settled (or gives up).
    fn settle(band: &mut ClarityBand, jobs: &JobSystem, p: &Project, at: Time, mut now: Instant) -> Instant {
        for _ in 0..500 {
            assert!(band.follow(jobs, p, false, at, now).is_none());
            if band.settled() {
                return now;
            }
            std::thread::sleep(Duration::from_millis(5));
            now += Duration::from_millis(50);
        }
        panic!("never settled");
    }

    #[test]
    fn the_playhead_is_checked_once_it_rests_and_playback_stops() {
        let (jobs, p) = (jobs(), project());
        let mut band = ClarityBand::default();
        let t0 = Instant::now();
        band.follow(&jobs, &p, false, Time::seconds(3), t0);
        assert!(band.progress().is_none(), "waits for the playhead to rest");
        band.follow(&jobs, &p, true, Time::seconds(3), t0 + SETTLE);
        assert!(band.progress().is_none(), "nothing while playing");
        let now = settle(&mut band, &jobs, &p, Time::seconds(3), t0 + SETTLE);
        let (report, current) = band.report().unwrap();
        assert!(current && report.findings.is_empty());
        assert_eq!(report.request, Request::Playhead(Time::seconds(3)));
        // Moved: not current until it rests and is checked again.
        band.follow(&jobs, &p, false, Time::seconds(5), now);
        assert!(!band.report().unwrap().1 && !band.settled());
        settle(&mut band, &jobs, &p, Time::seconds(5), now);
        assert_eq!(
            band.report().unwrap().0.request,
            Request::Playhead(Time::seconds(5))
        );
    }

    #[test]
    fn the_region_is_checked_at_once_and_a_change_cancels_the_running_check() {
        let (jobs, p) = (jobs(), project());
        let seq = p.active_sequence;
        let p = Command::SetRegion {
            sequence: seq,
            region: Some(Region {
                start: Time::seconds(1),
                end: Time::seconds(4),
            }),
        }
        .apply(&p)
        .unwrap();
        let mut band = ClarityBand {
            scope: Scope::Region,
            ..ClarityBand::default()
        };
        let t0 = Instant::now();
        band.follow(&jobs, &p, true, Time::ZERO, t0);
        assert!(band.progress().is_some(), "at once, playing or not");
        // The tracks change: the running check is for the old ones.
        let changed = Command::SetTrackMix {
            sequence: seq,
            track: p.active().unwrap().audio_tracks[0].id,
            muted: true,
            solo: false,
            gain_db: 0.0,
        }
        .apply(&p)
        .unwrap();
        settle(&mut band, &jobs, &changed, Time::ZERO, t0);
        let report = band.report().unwrap().0;
        assert_eq!(
            report.request,
            Request::Region(Time::seconds(1), Time::seconds(4))
        );
        assert_eq!(report.tracks.len(), 1, "checked with the first track muted");
        // The region taken away: back to the playhead.
        band.follow(&jobs, &project(), false, Time::ZERO, t0);
        assert_eq!(band.scope, Scope::Playhead);
    }
}
