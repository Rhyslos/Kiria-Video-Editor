//! Puts the autosave journal on the disk itself, on a thread of its own.
//!
//! Each change is written to the journal the moment it happens, so a crash
//! of the app loses nothing: the system holds what was written. Asking the
//! disk to keep it through a power cut too (`sync_data`) can take tens of
//! milliseconds, which the interface thread can't spend (an import of
//! hundreds of files writes a journal line for each). So that wait happens
//! here: [`JournalSync::request`] hands the thread a handle to the journal
//! and returns at once; requests that come while the disk is busy are put
//! on it together. A failure is kept and reported by the next request.

use std::fs::File;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

/// The journal's disk writer: one thread per open project.
#[derive(Debug)]
pub(crate) struct JournalSync {
    /// `None` when the thread could not start: then requests sync inline.
    to_thread: Option<Sender<File>>,
    shared: Arc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct State {
    requested: u64,
    /// Requests put on the disk (several per `sync_data` when they came together).
    done: u64,
    /// How many times `sync_data` ran.
    syncs: u64,
    failed: Option<io::Error>,
}

impl JournalSync {
    pub(crate) fn start() -> JournalSync {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("journal sync".into())
            .spawn(move || run(&rx, &thread_shared));
        let to_thread = match started {
            Ok(_) => Some(tx),
            Err(err) => {
                tracing::warn!("the journal's disk writer did not start ({err}); syncing on the spot");
                None
            }
        };
        JournalSync { to_thread, shared }
    }

    /// Asks for everything written to `journal` so far to be put on the
    /// disk, without waiting. Returns an earlier request's failure, if one
    /// failed since the last call.
    pub(crate) fn request(&self, journal: &File) -> io::Result<()> {
        let mut state = lock(&self.shared.state);
        if let Some(err) = state.failed.take() {
            return Err(err);
        }
        state.requested += 1;
        drop(state);
        let handed = match (&self.to_thread, journal.try_clone()) {
            (Some(tx), Ok(handle)) => tx.send(handle).is_ok(),
            _ => false,
        };
        if !handed {
            let result = journal.sync_data();
            let mut state = lock(&self.shared.state);
            state.done += 1;
            state.syncs += 1;
            result?;
        }
        Ok(())
    }

    /// Waits until every request so far is on the disk; returns how many
    /// times the disk was asked.
    #[cfg(test)]
    pub(crate) fn wait(&self) -> u64 {
        let mut state = lock(&self.shared.state);
        while state.done < state.requested {
            state = self.shared.changed.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        state.syncs
    }
}

fn run(rx: &Receiver<File>, shared: &Shared) {
    while let Ok(mut journal) = rx.recv() {
        let mut taken = 1;
        // Requests that came meanwhile go on the disk together: the newest
        // handle is enough, since each is the same journal (or the fresh one
        // a save started, which makes the older lines unneeded).
        while let Ok(newer) = rx.try_recv() {
            journal = newer;
            taken += 1;
        }
        let result = journal.sync_data();
        let mut state = lock(&shared.state);
        state.done += taken;
        state.syncs += 1;
        if let Err(err) = result {
            tracing::warn!("the autosave journal could not be put on the disk: {err}");
            state.failed = Some(err);
        }
        drop(state);
        shared.changed.notify_all();
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::Write;

    #[test]
    fn requests_return_at_once_and_reach_the_disk_together() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.journal");
        let mut journal = File::create(&path).unwrap();
        let sync = JournalSync::start();
        for i in 0..200 {
            writeln!(journal, "line {i}").unwrap();
            sync.request(&journal).unwrap();
        }
        let syncs = sync.wait();
        assert!((1..=200).contains(&syncs), "{syncs}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 200);
        assert_eq!(text.lines().last(), Some("line 199"));
    }

    #[test]
    fn a_failure_is_reported_by_the_next_request() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = File::create(tmp.path().join("p.journal")).unwrap();
        let sync = JournalSync::start();
        lock(&sync.shared.state).failed = Some(io::Error::other("disk gone"));
        assert!(sync.request(&journal).is_err());
        assert!(sync.request(&journal).is_ok(), "reported once");
        sync.wait();
    }
}
