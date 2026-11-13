//! The clarity check: where tracks share frequencies, at the playhead or
//! over the region, found on a worker and said in plain words, each with
//! one-click fixes.
//!
//! - [`listen`]: each heard track's sound after its own effects, as the
//!   mixer hears it (`crates/audio`'s strips, ducking included), in bands.
//! - [`bands`]: the third-octave bands and how much each matters to speech.
//! - [`judge`]: where another track covers the voice, and where several
//!   pile up between 200 and 500 Hz; stretches, words and fixes.
//! - [`fix`]: the fixes, each an ordinary undoable `SetTrackSound`.
//!
//! [`Clarity::start`] runs [`analyse`] on the jobs' Analysis pool; the
//! interface only clones the project (cheap) and polls.

pub mod bands;
pub mod fix;
pub mod judge;
pub mod listen;

#[cfg(test)]
mod tests;

pub use fix::{Fix, Remedy};
pub use listen::{Clips, DrySound};

use audio::settings::sample_at;
use diag::AppError;
use jobs::{JobHandle, JobProgress, JobSystem, PoolKind, Priority};
use project::{Project, SequenceId, Time, TrackId};

/// How far either side of the playhead a check at the playhead listens.
pub const AROUND_PLAYHEAD: Time = Time::seconds(1);

/// What to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// A short stretch around a moment: [`AROUND_PLAYHEAD`] either side.
    Playhead(Time),
    /// A stretch of the timeline (the region), start and end.
    Region(Time, Time),
}

impl Request {
    /// The stretch it listens to.
    pub fn stretch(self) -> (Time, Time) {
        match self {
            Request::Playhead(t) => ((t - AROUND_PLAYHEAD).max(Time::ZERO), t + AROUND_PLAYHEAD),
            Request::Region(a, b) => (a.max(Time::ZERO), b.max(a.max(Time::ZERO))),
        }
    }
}

/// What kind of trouble a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Problem {
    /// Another track covers the dialogue where words are understood.
    CoversVoice,
    /// Several tracks pile up between 200 and 500 Hz.
    Muddy,
}

/// One stretch of trouble.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub problem: Problem,
    /// Where on the timeline, start and end.
    pub range: (Time, Time),
    /// The tracks involved: the covering track, or those piling up
    /// (loudest first).
    pub tracks: Vec<TrackId>,
    /// The band, lowest and highest third-octave centre, Hz.
    pub band_hz: (u32, u32),
    /// How bad, 0 (fine) to 1 (as bad as it gets).
    pub severity: f32,
    /// One plain sentence: "The music on A2 covers the voice between 1
    /// and 3.2 kHz: words may be hard to follow."
    pub words: String,
    /// The fixes on offer, the one that helps most first.
    pub fixes: Vec<Fix>,
}

/// One track's mean level per band over the stretch: what the page draws
/// to show where tracks share frequencies.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackBands {
    pub track: TrackId,
    /// dBFS per band of [`bands::CENTRES`].
    pub levels_db: [f32; bands::COUNT],
}

/// What a check found.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub sequence: SequenceId,
    pub request: Request,
    /// The stretch listened to.
    pub stretch: (Time, Time),
    /// Worst first; empty for a clear mix.
    pub findings: Vec<Finding>,
    /// Every heard track, dialogue first.
    pub tracks: Vec<TrackBands>,
}

impl Report {
    /// How much trouble of a kind there is, involving a track: severity
    /// times seconds, added up (0 when none). For telling whether a fix
    /// helped.
    pub fn amount(&self, problem: Problem, track: TrackId) -> f32 {
        self.findings
            .iter()
            .filter(|f| f.problem == problem && f.tracks.contains(&track))
            .map(|f| f.severity * (f.range.1 - f.range.0).as_secs_f64() as f32)
            .sum()
    }
}

fn no_timeline() -> AppError {
    AppError::new("Clarity can't be checked")
        .why("The timeline is not in the project any more.")
        .fix("Open another timeline and check again.")
}

/// Checks `request` on `sequence`, taking each track's clips from `dry`.
/// Takes about as long as mixing the stretch, so call it on a worker.
/// `go_on` is asked between frames (`false` stops it: `Ok(None)`);
/// `progress` is told the share done, 0 to 1.
pub fn analyse(
    project: &Project,
    sequence: SequenceId,
    request: Request,
    dry: &mut dyn DrySound,
    go_on: &dyn Fn() -> bool,
    progress: &dyn Fn(f32),
) -> diag::Result<Option<Report>> {
    let seq = project.sequence(sequence).ok_or_else(no_timeline)?;
    let rate = seq.settings.sample_rate.max(8_000);
    let stretch = request.stretch();
    let (from, to) = (sample_at(stretch.0, rate), sample_at(stretch.1, rate));
    let frames = (to.saturating_sub(from)).div_ceil(listen::HOP as u64) as usize;
    let mut listening = listen::Listening::new(seq, rate, from);
    let mut judge = judge::Judge::new(listening.tracks().cloned().collect(), rate, from);
    for done in 0..frames {
        if !go_on() {
            return Ok(None);
        }
        judge.add(&listening.next_frame(project, dry));
        progress(done as f32 / frames.max(1) as f32);
    }
    progress(1.0);
    Ok(Some(Report {
        sequence,
        request,
        stretch,
        findings: judge.findings(sequence),
        tracks: judge
            .levels()
            .into_iter()
            .map(|(track, levels_db)| TrackBands { track, levels_db })
            .collect(),
    }))
}

/// How a check on a worker ended.
#[derive(Debug)]
pub enum Outcome {
    Done(Report),
    Cancelled,
    Failed(AppError),
}

/// A clarity check running on the Analysis pool. Starting, polling,
/// cancelling and reading progress never wait.
#[derive(Debug)]
pub struct Clarity {
    handle: JobHandle<Option<Report>>,
    request: Request,
}

impl Clarity {
    /// Starts a check of `sequence` with its clips decoded from their files.
    pub fn start(jobs: &JobSystem, project: Project, sequence: SequenceId, request: Request) -> Clarity {
        let rate = project
            .sequence(sequence)
            .map_or(48_000, |s| s.settings.sample_rate.max(8_000));
        Clarity::start_with(jobs, project, sequence, request, Box::new(Clips::new(rate)))
    }

    /// As [`Clarity::start`], with the clips' sound from `dry`.
    pub fn start_with(
        jobs: &JobSystem,
        project: Project,
        sequence: SequenceId,
        request: Request,
        mut dry: Box<dyn DrySound>,
    ) -> Clarity {
        let handle = jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "clarity check",
            move |ctx| {
                analyse(
                    &project,
                    sequence,
                    request,
                    dry.as_mut(),
                    &|| !ctx.is_cancelled(),
                    &|done| ctx.set_progress(done),
                )
            },
        );
        Clarity { handle, request }
    }

    pub fn request(&self) -> Request {
        self.request
    }

    /// The share done, 0 to 1 (0 while waiting to start).
    pub fn progress(&self) -> f32 {
        match self.handle.progress() {
            JobProgress::Waiting => 0.0,
            JobProgress::Running(done) => done,
        }
    }

    /// Asks it to stop; [`Clarity::poll`] then says `Cancelled`.
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    /// How it ended, once (`None` while it runs).
    pub fn poll(&self) -> Option<Outcome> {
        Some(match self.handle.try_result()? {
            Ok(Some(report)) => Outcome::Done(report),
            Ok(None) => Outcome::Cancelled,
            Err(err) => Outcome::Failed(err),
        })
    }

    /// Waits for the end: workers and tests only, never the interface.
    pub fn wait(self) -> Outcome {
        match self.handle.wait() {
            Ok(Some(report)) => Outcome::Done(report),
            Ok(None) => Outcome::Cancelled,
            Err(err) => Outcome::Failed(err),
        }
    }
}
