//! The background log writer.
//!
//! Log lines are handed to a thread of their own that writes them to the
//! file, so logging never waits for the disk. Unlike a plain background
//! writer it can be asked to [`flush`](BackgroundWriter::flush): to write
//! everything queued so far and say so, with a time limit. The crash
//! handler does that before it reads the log back, so a report has the lines
//! written just before the panic.

use crossbeam_channel::{Receiver, Sender};
use std::cell::Cell;
use std::io::{self, Write};
use std::time::{Duration, Instant};
use tracing_subscriber::fmt::MakeWriter;

/// How many lines may wait for the disk before new ones are dropped.
const QUEUE_LINES: usize = 128_000;

thread_local! {
    /// True on the writer's own thread, which must never wait for itself.
    static ON_WRITER_THREAD: Cell<bool> = const { Cell::new(false) };
}

enum Message {
    Line(Vec<u8>),
    /// Write everything before this, then answer.
    Flush(Sender<()>),
}

/// Hands log lines to the writer thread. Cheap to clone.
#[derive(Debug, Clone)]
pub(crate) struct BackgroundWriter {
    sender: Sender<Message>,
}

impl BackgroundWriter {
    /// Starts the writer thread, which writes to `out` until every
    /// `BackgroundWriter` is gone.
    pub(crate) fn spawn(out: impl Write + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = crossbeam_channel::bounded(QUEUE_LINES);
        std::thread::Builder::new()
            .name("log writer".into())
            .spawn(move || write_lines(out, &receiver))?;
        Ok(Self { sender })
    }

    /// Waits at most `limit` until every line queued before this call is in
    /// the file. Returns whether it got there. Never waits on the writer's
    /// own thread (a panic while writing the log), nor once the writer has
    /// stopped.
    pub(crate) fn flush(&self, limit: Duration) -> bool {
        if ON_WRITER_THREAD.with(Cell::get) {
            return false;
        }
        let deadline = Instant::now() + limit;
        let (done, answer) = crossbeam_channel::bounded(1);
        if self.sender.send_deadline(Message::Flush(done), deadline).is_err() {
            return false;
        }
        answer.recv_deadline(deadline).is_ok()
    }
}

fn write_lines(mut out: impl Write, receiver: &Receiver<Message>) {
    ON_WRITER_THREAD.with(|on| on.set(true));
    for message in receiver {
        match message {
            // A line that can't be written has nowhere else to go.
            Message::Line(line) => drop(out.write_all(&line)),
            Message::Flush(done) => {
                let _ = out.flush();
                let _ = done.send(());
            }
        }
    }
    let _ = out.flush();
}

impl Write for &BackgroundWriter {
    /// Queues one formatted event. When the queue is full the line is
    /// dropped rather than holding up the program.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // A full queue or a stopped writer loses the line, never the program.
        let _ = self.sender.try_send(Message::Line(buf.to_vec()));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BackgroundWriter {
    type Writer = &'a BackgroundWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A writer that keeps what it was given.
    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<u8>>>);

    impl Write for Kept {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn flush_waits_for_every_queued_line() {
        let kept = Kept::default();
        let writer = BackgroundWriter::spawn(kept.clone()).unwrap();
        for n in 0..1000 {
            (&writer).write_all(format!("line {n}\n").as_bytes()).unwrap();
        }
        assert!(writer.flush(Duration::from_secs(10)));
        let text = String::from_utf8(kept.0.lock().unwrap().clone()).unwrap();
        assert!(text.ends_with("line 999\n"), "{text}");
    }

    #[test]
    fn flush_on_the_writer_thread_does_not_wait() {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let writer = BackgroundWriter { sender };
        let probe = writer.clone();
        let handle = std::thread::spawn(move || {
            ON_WRITER_THREAD.with(|on| on.set(true));
            let started = Instant::now();
            assert!(!probe.flush(Duration::from_secs(30)));
            started.elapsed()
        });
        assert!(handle.join().unwrap() < Duration::from_secs(5));
        drop((writer, receiver));
    }

    #[test]
    fn flush_gives_up_after_its_limit() {
        // Nobody reads the queue: the answer never comes.
        let (sender, _receiver) = crossbeam_channel::bounded(4);
        let writer = BackgroundWriter { sender };
        assert!(!writer.flush(Duration::from_millis(50)));
    }
}
