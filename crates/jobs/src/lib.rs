//! Worker pools, priorities and resource budgets.
//!
//! Heavy work never runs on the interface thread. It is submitted to one of
//! four pools (decode, render, analysis, export) with a priority: playback
//! work first, then work someone is waiting for, then background work, which
//! can pause while the timeline plays. Pool sizes come from the resource
//! budgets in Settings, and one CPU thread is always left for the interface.

mod budget;
mod pool;
mod usage;

pub use budget::{BackgroundPolicy, Budgets, Limit, Machine, Resolved};
pub use usage::{Usage, UsageMonitor};

use diag::AppError;
use pool::Pool;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

/// How urgent a job is. Lower variants run first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Needed for the next frames of playback.
    Playback,
    /// Someone is waiting for it (a thumbnail on screen, an export they started).
    Interactive,
    /// Proxies, analysis, pre-rendering, autosave: may pause while playing.
    Background,
}

/// Which pool a job runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolKind {
    Decode,
    Render,
    Analysis,
    Export,
}

/// How far a job has come, as its handle sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobProgress {
    /// Not started: in the queue, or held back while the timeline plays.
    Waiting,
    /// Running: the share done, 0 to 1, as the job last reported it (0 for
    /// a job that reports nothing).
    Running(f32),
}

/// A job's progress, shared between its worker and its handle: the share
/// done as the bits of an `f32`, negative until the job starts. One atomic
/// number, so neither side ever waits for the other.
#[derive(Debug)]
pub(crate) struct ProgressCell(AtomicU32);

impl ProgressCell {
    pub(crate) fn waiting() -> ProgressCell {
        ProgressCell(AtomicU32::new((-1.0f32).to_bits()))
    }

    fn set(&self, done: f32) {
        let done = if done.is_finite() {
            done.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.0.store(done.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> JobProgress {
        let done = f32::from_bits(self.0.load(Ordering::Relaxed));
        if done < 0.0 {
            JobProgress::Waiting
        } else {
            JobProgress::Running(done)
        }
    }
}

/// Handed to every job.
#[derive(Debug, Clone)]
pub struct JobContext {
    cancelled: Arc<AtomicBool>,
    progress: Arc<ProgressCell>,
}

impl JobContext {
    /// Long jobs check this now and then and stop early when it is true.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Says how much of the job is done, from 0 to 1. Costs next to
    /// nothing, so long jobs can call it as often as is handy; the handle
    /// reads it with [`JobHandle::progress`].
    pub fn set_progress(&self, done: f32) {
        self.progress.set(done);
    }
}

/// The result of a submitted job, once it is done.
#[derive(Debug)]
pub struct JobHandle<T> {
    receiver: Receiver<diag::Result<T>>,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ProgressCell>,
}

impl<T> JobHandle<T> {
    /// The result if the job has finished; `None` while it is still queued or running.
    pub fn try_result(&self) -> Option<diag::Result<T>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(lost())),
        }
    }

    /// Waits for the result. Never call this on the interface thread.
    pub fn wait(self) -> diag::Result<T> {
        self.receiver.recv().unwrap_or_else(|_| Err(lost()))
    }

    /// Asks the job to stop early. A job that has not started yet still runs,
    /// sees the request and can return straight away.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether the job has started and how far it says it has come. Never
    /// blocks: safe to ask every frame.
    pub fn progress(&self) -> JobProgress {
        self.progress.get()
    }
}

fn lost() -> AppError {
    AppError::new("A background task disappeared")
        .why("Its worker stopped before returning a result.")
        .fix("Try again. If it keeps happening, report it with the Diagnostics details.")
}

/// All worker pools.
pub struct JobSystem {
    decode: Pool,
    render: Pool,
    analysis: Pool,
    export: Pool,
    budgets: Resolved,
    activity: Mutex<Activity>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Activity {
    playing: bool,
    idle: bool,
}

impl std::fmt::Debug for JobSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobSystem")
            .field("threads", &self.threads())
            .field("budgets", &self.budgets)
            .finish()
    }
}

impl JobSystem {
    /// Starts the pools. The worker budget is split 40 % decode, 30 % render,
    /// 15 % analysis, 15 % export, with at least one thread each.
    pub fn new(budgets: Resolved) -> JobSystem {
        let total = budgets.worker_threads.max(1);
        let share = |percent: usize| (total * percent / 100).max(1);
        let system = JobSystem {
            decode: Pool::new("decode", share(40)),
            render: Pool::new("render", share(30)),
            analysis: Pool::new("analysis", share(15)),
            export: Pool::new("export", share(15)),
            budgets,
            activity: Mutex::new(Activity::default()),
        };
        tracing::info!(threads = ?system.threads(), "worker pools started");
        system
    }

    /// Runs `job` in `pool`. The result arrives through the returned handle.
    pub fn submit<T, F>(&self, pool: PoolKind, priority: Priority, name: &str, job: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> diag::Result<T> + Send + 'static,
    {
        self.pool(pool).submit(priority, name, job)
    }

    /// Tell the pools whether the timeline is playing.
    pub fn set_playing(&self, playing: bool) {
        self.update(|a| a.playing = playing);
    }

    /// Tell the pools whether the app is idle (nobody has touched it for a while).
    pub fn set_idle(&self, idle: bool) {
        self.update(|a| a.idle = idle);
    }

    /// Threads per pool: decode, render, analysis, export.
    pub fn threads(&self) -> [usize; 4] {
        [
            self.decode.threads(),
            self.render.threads(),
            self.analysis.threads(),
            self.export.threads(),
        ]
    }

    /// Jobs waiting to start, across all pools.
    pub fn queued(&self) -> usize {
        [&self.decode, &self.render, &self.analysis, &self.export]
            .iter()
            .map(|p| p.queued())
            .sum()
    }

    pub fn budgets(&self) -> &Resolved {
        &self.budgets
    }

    fn update(&self, change: impl FnOnce(&mut Activity)) {
        let mut activity = self.activity.lock().unwrap_or_else(|p| p.into_inner());
        change(&mut activity);
        let paused = match self.budgets.background {
            BackgroundPolicy::Always => false,
            BackgroundPolicy::PauseWhilePlaying => activity.playing,
            BackgroundPolicy::OnlyWhenIdle => !activity.idle || activity.playing,
        };
        for pool in [&self.decode, &self.render, &self.analysis, &self.export] {
            pool.set_background_paused(paused);
        }
    }

    fn pool(&self, kind: PoolKind) -> &Pool {
        match kind {
            PoolKind::Decode => &self.decode,
            PoolKind::Render => &self.render,
            PoolKind::Analysis => &self.analysis,
            PoolKind::Export => &self.export,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn system(threads: usize, background: BackgroundPolicy) -> JobSystem {
        JobSystem::new(Resolved {
            worker_threads: threads,
            ram_cache_mb: 256,
            disk_cache_gb: 1,
            background,
        })
    }

    #[test]
    fn jobs_return_their_results() {
        let jobs = system(4, BackgroundPolicy::Always);
        let handle = jobs.submit(PoolKind::Analysis, Priority::Interactive, "add", |_| Ok(2 + 2));
        assert_eq!(handle.wait().unwrap(), 4);
    }

    #[test]
    fn errors_and_panics_come_back_as_explained_errors() {
        let jobs = system(2, BackgroundPolicy::Always);
        let fails = jobs.submit(PoolKind::Render, Priority::Interactive, "fails", |_| {
            Err::<(), _>(AppError::new("nope"))
        });
        assert_eq!(fails.wait().unwrap_err().what(), "nope");
        let panics = jobs.submit(
            PoolKind::Render,
            Priority::Interactive,
            "panics",
            |_| -> diag::Result<()> { panic!("boom") },
        );
        assert!(panics.wait().unwrap_err().what().contains("failed"));
    }

    #[test]
    fn urgent_jobs_run_before_background_jobs() {
        // One analysis thread, kept busy so the next jobs queue up.
        let jobs = system(1, BackgroundPolicy::Always);
        let gate = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        let blocker = jobs.submit(PoolKind::Analysis, Priority::Interactive, "blocker", move |_| {
            while !g.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });
        let order = Arc::new(Mutex::new(Vec::new()));
        let record = |name: &'static str| {
            let order = Arc::clone(&order);
            move |_: &JobContext| {
                order.lock().unwrap().push(name);
                Ok(())
            }
        };
        let bg = jobs.submit(
            PoolKind::Analysis,
            Priority::Background,
            "bg",
            record("background"),
        );
        let play = jobs.submit(PoolKind::Analysis, Priority::Playback, "play", record("playback"));
        gate.store(true, Ordering::Relaxed);
        blocker.wait().unwrap();
        play.wait().unwrap();
        bg.wait().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["playback", "background"]);
    }

    #[test]
    fn background_work_pauses_while_playing() {
        let jobs = system(2, BackgroundPolicy::PauseWhilePlaying);
        jobs.set_playing(true);
        let bg = jobs.submit(PoolKind::Export, Priority::Background, "bg", |_| Ok(()));
        std::thread::sleep(Duration::from_millis(100));
        assert!(bg.try_result().is_none(), "paused while playing");
        let urgent = jobs.submit(PoolKind::Export, Priority::Interactive, "urgent", |_| Ok(()));
        urgent.wait().unwrap();
        jobs.set_playing(false);
        bg.wait().unwrap();
    }

    #[test]
    fn cancelled_jobs_can_stop_early() {
        let jobs = system(2, BackgroundPolicy::Always);
        let handle = jobs.submit(PoolKind::Analysis, Priority::Background, "long", |ctx| {
            let start = Instant::now();
            while !ctx.is_cancelled() && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(ctx.is_cancelled())
        });
        std::thread::sleep(Duration::from_millis(20));
        handle.cancel();
        let start = Instant::now();
        let receiver_result = loop {
            if let Some(r) = handle.try_result() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(receiver_result.unwrap());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_job_reports_how_far_it_is() {
        // One analysis thread, so the second job waits behind the first.
        let jobs = system(1, BackgroundPolicy::Always);
        let gate = Arc::new(AtomicBool::new(false));
        let reporting = |gate: Arc<AtomicBool>| {
            move |ctx: &JobContext| {
                ctx.set_progress(0.25);
                while !gate.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                ctx.set_progress(7.0);
                Ok(())
            }
        };
        let first = jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "first",
            reporting(Arc::clone(&gate)),
        );
        let second = jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "second",
            reporting(Arc::clone(&gate)),
        );
        let start = Instant::now();
        while first.progress() != JobProgress::Running(0.25) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(first.progress(), JobProgress::Running(0.25));
        assert_eq!(second.progress(), JobProgress::Waiting, "still in the queue");
        gate.store(true, Ordering::Relaxed);
        while first.try_result().is_none() || second.try_result().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(first.progress(), JobProgress::Running(1.0), "kept within 0 to 1");
        // A job that reports nothing is running at 0 once it has started.
        let quiet = jobs.submit(PoolKind::Analysis, Priority::Interactive, "quiet", |_| Ok(()));
        while quiet.try_result().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(quiet.progress(), JobProgress::Running(0.0));
    }

    #[test]
    fn pools_get_their_share_and_at_least_one_thread() {
        assert_eq!(system(10, BackgroundPolicy::Always).threads(), [4, 3, 1, 1]);
        assert_eq!(system(1, BackgroundPolicy::Always).threads(), [1, 1, 1, 1]);
    }
}
