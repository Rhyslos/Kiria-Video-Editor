//! Running slow work off the interface thread.
//!
//! The browser never waits: it starts a [`Task`], keeps drawing, and picks
//! the result up on a later frame. A finished task asks for a redraw so the
//! result shows at once.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Work running on its own thread.
#[derive(Debug)]
pub struct Task<T> {
    result: Receiver<T>,
}

impl<T: Send + 'static> Task<T> {
    /// Starts `work` on a new thread. `ctx` is asked to redraw when it finishes.
    pub fn spawn(ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Task<T> {
        let (sender, result) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // The browser may have closed; then nobody needs the result.
            let _ = sender.send(work());
            ctx.request_repaint();
        });
        Task { result }
    }

    /// The result, once the work is done. `None` while it is still running.
    /// A task whose thread panicked is reported as finished with no result.
    pub fn poll(&self) -> Poll<T> {
        match self.result.try_recv() {
            Ok(value) => Poll::Done(value),
            Err(TryRecvError::Empty) => Poll::Running,
            Err(TryRecvError::Disconnected) => Poll::Lost,
        }
    }

    /// Blocks until the work is done. Only for shutting down.
    pub fn wait(self) -> Option<T> {
        self.result.recv().ok()
    }
}

/// What [`Task::poll`] found.
#[derive(Debug)]
pub enum Poll<T> {
    Running,
    Done(T),
    /// The thread stopped without a result (it panicked; the crash log has why).
    Lost,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_arrive_without_blocking() {
        let ctx = egui::Context::default();
        let task = Task::spawn(&ctx, || 6 * 7);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match task.poll() {
                Poll::Done(v) => {
                    assert_eq!(v, 42);
                    break;
                }
                Poll::Running if std::time::Instant::now() < deadline => std::thread::yield_now(),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
