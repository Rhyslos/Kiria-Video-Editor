//! One pool of worker threads with a priority queue.

use crate::{JobContext, JobHandle, Priority, ProgressCell};
use diag::AppError;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;

type Work = Box<dyn FnOnce() + Send + 'static>;

struct Task {
    priority: Priority,
    /// Writes something that must be on disk when the app ends (a save):
    /// it runs even when the pool closes before it started.
    writes: bool,
    order: u64,
    work: Work,
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Task {}
impl PartialOrd for Task {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Task {
    /// The heap pops the greatest: most urgent priority first, then oldest first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.order.cmp(&self.order))
    }
}

#[derive(Default)]
struct State {
    queue: BinaryHeap<Task>,
    next_order: u64,
    background_paused: bool,
    shutting_down: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// `State::background_paused`, readable by running jobs without the lock.
    held_back: Arc<AtomicBool>,
}

impl Shared {
    /// Locks the state even if a worker panicked while holding it; the state
    /// itself is always left consistent.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A named set of worker threads.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// `lowered`: its threads run below the interface thread's priority
    /// (`os::lower_this_thread`), for work nobody watches frame by frame.
    pub(crate) fn new(name: &str, threads: usize, lowered: bool) -> Pool {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            held_back: Arc::new(AtomicBool::new(false)),
        });
        let workers = (0..threads.max(1))
            .filter_map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("{name}-{i}"))
                    .spawn(move || {
                        if lowered && !os::lower_this_thread() {
                            tracing::debug!("the system kept a worker at normal priority");
                        }
                        worker(&shared);
                    })
                    .map_err(|err| tracing::error!("could not start a {name} worker: {err}"))
                    .ok()
            })
            .collect();
        Pool { shared, workers }
    }

    pub(crate) fn threads(&self) -> usize {
        self.workers.len()
    }

    pub(crate) fn submit<T, F>(&self, priority: Priority, writes: bool, name: &str, job: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> diag::Result<T> + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ProgressCell::waiting());
        let context = JobContext {
            cancelled: Arc::clone(&cancelled),
            progress: Arc::clone(&progress),
            held_back: (priority == Priority::Background).then(|| Arc::clone(&self.shared.held_back)),
        };
        let name = name.to_owned();
        let work: Work = Box::new(move || {
            let _span = tracing::debug_span!("job", name = %name).entered();
            // From here the handle sees it as running.
            context.set_progress(0.0);
            let result = catch_unwind(AssertUnwindSafe(|| job(&context))).unwrap_or_else(|_| {
                Err(AppError::new("A background task failed")
                    .why(format!("The task \"{name}\" stopped unexpectedly."))
                    .fix("Try again. If it keeps happening, report it with the Diagnostics details."))
            });
            // The handle may have been dropped; then nobody wants the result.
            let _ = sender.send(result);
        });
        let mut state = self.shared.lock();
        let order = state.next_order;
        state.next_order += 1;
        state.queue.push(Task {
            priority,
            writes,
            order,
            work,
        });
        drop(state);
        self.shared.wake.notify_one();
        JobHandle {
            receiver,
            cancelled,
            progress,
        }
    }

    pub(crate) fn set_background_paused(&self, paused: bool) {
        self.shared.lock().background_paused = paused;
        self.shared.held_back.store(paused, AtomicOrdering::Relaxed);
        self.shared.wake.notify_all();
    }

    pub(crate) fn queued(&self) -> usize {
        self.shared.lock().queue.len()
    }
}

impl Drop for Pool {
    /// Finishes the jobs running and the queued ones that write (see
    /// [`Task::writes`]), then stops the threads. Other queued jobs never
    /// start: nobody is left to want what they work out, and closing would
    /// wait for them (their handles say the work was given up).
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        let dropped = state.queue.len();
        state.queue.retain(|t| t.writes);
        let dropped = dropped - state.queue.len();
        if dropped > 0 {
            tracing::debug!(dropped, "queued jobs given up on closing");
        }
        state.shutting_down = true;
        state.background_paused = false;
        self.shared.held_back.store(false, AtomicOrdering::Relaxed);
        drop(state);
        self.shared.wake.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker(shared: &Shared) {
    loop {
        let mut state = shared.lock();
        let task = loop {
            let runnable = state
                .queue
                .peek()
                .is_some_and(|t| t.priority != Priority::Background || !state.background_paused);
            if runnable {
                if let Some(task) = state.queue.pop() {
                    break task;
                }
            } else if state.shutting_down && state.queue.is_empty() {
                return;
            }
            state = shared
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        };
        drop(state);
        (task.work)();
    }
}
