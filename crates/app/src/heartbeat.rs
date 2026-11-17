//! Notices when the window stops drawing, and writes it to the log with the
//! step it was on, so a freeze seen on someone else's computer can be found
//! from their log alone.
//!
//! The interface thread marks each frame ([`Heartbeat::beat`]) and the
//! steps worth knowing about ([`Heartbeat::step`]): two atomic stores, no
//! lock. A thread of its own looks every half second; when no frame has
//! been drawn for [`STALL`] it writes a warning naming the last step, and
//! when drawing starts again it writes how long it stopped for.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How long without a frame counts as stopped. A window that isn't being
/// drawn because nothing changes still asks for a frame now and then (the
/// status line, meters, autosave), so this is long enough not to mistake
/// a quiet window for a stopped one.
pub const STALL: Duration = Duration::from_secs(4);

/// What the interface thread was doing, as the log names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Between frames: the window toolkit drawing the last one on screen,
    /// or handling the window's events.
    BetweenFrames,
    /// Working out and drawing a frame.
    Frame,
    /// Asking the window to fill the screen, or to come back from it.
    AskingFullScreen,
    /// The first frame after that, at the new size.
    FirstFullScreenFrame,
    /// The window is put away: no frames come, and that is no stop.
    Minimized,
}

impl Step {
    const ALL: [Step; 5] = [
        Step::BetweenFrames,
        Step::Frame,
        Step::AskingFullScreen,
        Step::FirstFullScreenFrame,
        Step::Minimized,
    ];

    fn name(self) -> &'static str {
        match self {
            Step::BetweenFrames => "between frames (the window being drawn on screen, or its events)",
            Step::Frame => "working out a frame",
            Step::AskingFullScreen => "asking the window to fill the screen or come back",
            Step::FirstFullScreenFrame => "the first frame at the new size after going full screen",
            Step::Minimized => "put away",
        }
    }
}

/// The interface thread's side.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    start: Instant,
    /// Milliseconds from `start` when the last frame began.
    last: AtomicU64,
    step: AtomicUsize,
}

impl Heartbeat {
    /// Starts the watching thread, which lives as long as the program.
    pub fn start() -> Heartbeat {
        let shared = Arc::new(Shared {
            start: Instant::now(),
            last: AtomicU64::new(0),
            step: AtomicUsize::new(0),
        });
        let watched = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("heartbeat".into())
            .spawn(move || watch(&watched, STALL, |line| tracing::warn!("{line}"), || true));
        if let Err(err) = spawned {
            tracing::warn!("no watch for a stopped window: {err}");
        }
        Heartbeat { shared }
    }

    /// A frame is beginning.
    pub fn beat(&self) {
        let now = self.shared.start.elapsed().as_millis() as u64;
        self.shared.last.store(now, Ordering::Relaxed);
        self.step(Step::Frame);
    }

    /// What the interface thread is doing now.
    pub fn step(&self, step: Step) {
        let index = Step::ALL.iter().position(|s| *s == step).unwrap_or(0);
        self.shared.step.store(index, Ordering::Relaxed);
    }
}

/// Looks every half second (a tenth of `stall` in tests) while `go_on`
/// says so, telling `log` when no frame has begun for `stall` and when one
/// begins again.
fn watch(shared: &Shared, stall: Duration, log: impl Fn(String), go_on: impl Fn() -> bool) {
    let look = (stall / 8).min(Duration::from_millis(500));
    let mut stopped: Option<u64> = None;
    while go_on() {
        std::thread::sleep(look);
        let now = shared.start.elapsed().as_millis() as u64;
        let last = shared.last.load(Ordering::Relaxed);
        let quiet = now.saturating_sub(last);
        let step = Step::ALL[shared.step.load(Ordering::Relaxed).min(Step::ALL.len() - 1)];
        match stopped {
            None if quiet >= stall.as_millis() as u64 && step != Step::Minimized => {
                log(format!(
                    "The window has not drawn for {:.1} s; it was {}",
                    quiet as f64 / 1000.0,
                    step.name()
                ));
                stopped = Some(last);
            }
            Some(since) if last != since => {
                log(format!(
                    "The window draws again, after {:.1} s",
                    last.saturating_sub(since) as f64 / 1000.0
                ));
                stopped = None;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn a_stopped_window_is_logged_with_its_step_and_once_more_when_it_draws_again() {
        let beat = Heartbeat {
            shared: Arc::new(Shared {
                start: Instant::now(),
                last: AtomicU64::new(0),
                step: AtomicUsize::new(0),
            }),
        };
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let running = Arc::new(AtomicBool::new(true));
        let watcher = {
            let (shared, lines, running) =
                (Arc::clone(&beat.shared), Arc::clone(&lines), Arc::clone(&running));
            std::thread::spawn(move || {
                watch(
                    &shared,
                    Duration::from_millis(200),
                    |line| lines.lock().unwrap().push(line),
                    || running.load(Ordering::Relaxed),
                )
            })
        };
        // Drawing steadily: nothing to say.
        for _ in 0..10 {
            beat.beat();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(lines.lock().unwrap().is_empty());
        // Stuck while asking to fill the screen.
        beat.step(Step::AskingFullScreen);
        std::thread::sleep(Duration::from_millis(400));
        beat.beat();
        std::thread::sleep(Duration::from_millis(100));
        running.store(false, Ordering::Relaxed);
        watcher.join().unwrap();
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("fill the screen"), "{}", lines[0]);
        assert!(lines[1].starts_with("The window draws again"), "{}", lines[1]);
    }

    #[test]
    fn a_window_put_away_is_no_stop() {
        let shared = Arc::new(Shared {
            start: Instant::now(),
            last: AtomicU64::new(0),
            step: AtomicUsize::new(Step::ALL.iter().position(|s| *s == Step::Minimized).unwrap()),
        });
        let lines = Mutex::new(Vec::<String>::new());
        let looks = std::sync::atomic::AtomicU32::new(0);
        watch(
            &shared,
            Duration::from_millis(80),
            |line| lines.lock().unwrap().push(line),
            || looks.fetch_add(1, Ordering::Relaxed) < 20,
        );
        assert!(lines.lock().unwrap().is_empty());
    }
}
