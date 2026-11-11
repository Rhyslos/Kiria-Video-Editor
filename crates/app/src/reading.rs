//! How far a piece of background reading has come, the way the timeline
//! shows it. Shared by waveforms, clip pictures and the line by each
//! track's name, so they all tell progress and time left the same way.

use jobs::JobProgress;
use std::time::Instant;
use ui_kit::timeline::LoadState;

/// Time left is only told once the work has run this long and come this
/// far; before that the estimate jumps about.
const STEADY_AFTER_SECS: f64 = 2.0;
const STEADY_AFTER_SHARE: f32 = 0.05;

/// Seconds left for work that went from the share `from` to `done` (0 to 1)
/// in `elapsed` seconds, if it keeps that pace. `None` until that can be
/// told steadily.
pub fn seconds_left(elapsed: f64, from: f32, done: f32) -> Option<f32> {
    let gained = done - from;
    if elapsed < STEADY_AFTER_SECS || gained < STEADY_AFTER_SHARE || done >= 1.0 {
        return None;
    }
    Some((elapsed * f64::from(1.0 - done) / f64::from(gained)) as f32)
}

/// Remembers when a piece of work was first seen moving and how far it was
/// then, to tell the time left from its pace since.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Pace {
    since: Option<(Instant, f32)>,
}

impl Pace {
    /// What to show for a job that reports `progress`, at `now`.
    pub fn of_job(&mut self, progress: JobProgress, now: Instant) -> LoadState {
        match progress {
            JobProgress::Waiting => {
                self.since = None;
                LoadState::Waiting
            }
            JobProgress::Running(done) => self.reading(done, now),
        }
    }

    /// What to show for work that is `done` (0 to 1) at `now`.
    pub fn reading(&mut self, done: f32, now: Instant) -> LoadState {
        let (since, from) = *self.since.get_or_insert((now, done));
        LoadState::Reading {
            done,
            seconds_left: seconds_left(now.duration_since(since).as_secs_f64(), from, done),
        }
    }
}

/// Stops a worker where it is when its share done reaches `hold_at`, until
/// the job is cancelled: `--hold-reading`, so a screenshot can show reading
/// in progress. Gives up after a minute so nothing hangs for good.
pub fn hold(hold_at: Option<f32>, done: f32, job: &jobs::JobContext) {
    if hold_at.is_none_or(|at| done < at) {
        return;
    }
    let started = Instant::now();
    while !job.is_cancelled() && started.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn time_left_follows_the_pace_so_far() {
        // A quarter done in 10 s: three quarters take 30 s more.
        assert_eq!(seconds_left(10.0, 0.0, 0.25), Some(30.0));
        // Seen from 20 % on: 30 % more in 6 s, so the last 50 % take 10 s.
        assert_eq!(seconds_left(6.0, 0.2, 0.5), Some(10.0));
    }

    #[test]
    fn no_time_left_is_told_before_it_is_steady() {
        assert_eq!(seconds_left(0.5, 0.0, 0.4), None, "too early");
        assert_eq!(seconds_left(30.0, 0.0, 0.01), None, "too little done to judge");
        assert_eq!(seconds_left(30.0, 0.5, 0.5), None, "standing still");
        assert_eq!(seconds_left(30.0, 0.0, 1.0), None, "done");
    }

    #[test]
    fn a_job_is_waiting_until_it_runs_and_then_paced_from_there() {
        let mut pace = Pace::default();
        let start = Instant::now();
        assert_eq!(pace.of_job(JobProgress::Waiting, start), LoadState::Waiting);
        let later = start + Duration::from_secs(60);
        assert_eq!(
            pace.of_job(JobProgress::Running(0.0), later),
            LoadState::Reading {
                done: 0.0,
                seconds_left: None
            }
        );
        // The minute spent waiting doesn't count: 50 % in 5 s of running.
        assert_eq!(
            pace.of_job(JobProgress::Running(0.5), later + Duration::from_secs(5)),
            LoadState::Reading {
                done: 0.5,
                seconds_left: Some(5.0)
            }
        );
    }

    #[test]
    fn holding_does_nothing_unless_asked_and_reached() {
        let jobs = jobs::JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let handle = jobs.submit(
            jobs::PoolKind::Analysis,
            jobs::Priority::Interactive,
            "hold",
            |job| {
                hold(None, 1.0, job);
                hold(Some(0.5), 0.2, job);
                Ok(())
            },
        );
        handle.wait().unwrap();
        // Reached: held until cancelled.
        let handle = jobs.submit(
            jobs::PoolKind::Analysis,
            jobs::Priority::Interactive,
            "hold",
            |job| {
                hold(Some(0.5), 0.6, job);
                Ok(job.is_cancelled())
            },
        );
        std::thread::sleep(Duration::from_millis(60));
        assert!(handle.try_result().is_none(), "held");
        handle.cancel();
        assert!(handle.wait().unwrap());
    }
}
