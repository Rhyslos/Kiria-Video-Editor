//! Work that must not hold up the interface thread (it waits on the disk,
//! the system or a program), done on a thread of its own: [`answer`]
//! starts it and hands back a receiver the page looks at each frame.

use std::sync::mpsc::{Receiver, channel};

/// Runs `work` on a thread named `name` and sends its answer, then asks
/// `ctx` to redraw so the page sees it. When the thread can't start, that
/// is logged and `failed` turns the reason into the answer at once (an
/// explained error, or "nothing", as the caller needs).
pub fn answer<T: Send + 'static>(
    ctx: &egui::Context,
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
    failed: impl FnOnce(std::io::Error) -> T,
) -> Receiver<T> {
    let (tell, answer) = channel();
    let (sender, ctx) = (tell.clone(), ctx.clone());
    let started = std::thread::Builder::new().name(name.to_owned()).spawn(move || {
        // The page may have stopped waiting; then nobody needs it.
        let _ = sender.send(work());
        ctx.request_repaint();
    });
    if let Err(err) = started {
        tracing::error!("the thread {name:?} could not start: {err}");
        let _ = tell.send(failed(err));
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_answer_arrives_from_a_thread_of_its_own() {
        let ctx = egui::Context::default();
        let answer = answer(
            &ctx,
            "test worker",
            || std::thread::current().name().map(str::to_owned),
            |_| None,
        );
        assert_eq!(
            answer.recv_timeout(Duration::from_secs(10)).unwrap(),
            Some("test worker".to_owned())
        );
    }
}
