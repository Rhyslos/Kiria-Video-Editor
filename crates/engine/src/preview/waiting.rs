//! The requests waiting for the worker: only the newest counts.

use super::Request;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

/// What came of waiting for a request for a while.
pub(super) enum Next {
    Request(Box<Request>),
    /// Nothing was asked for in that time.
    Idle,
    /// Nobody asks any more.
    Closed,
}

/// The requests that have arrived and not been worked on: only the newest
/// counts. It is taken in while a picture is being decoded too, so the
/// decode can be asked whether it is still wanted.
pub(super) struct Waiting<'a> {
    inbox: &'a Receiver<Request>,
    pub(super) newest: Option<Request>,
    /// Nobody asks any more: the preview was closed or dropped.
    pub(super) closed: bool,
}

impl<'a> Waiting<'a> {
    pub(super) fn new(inbox: &'a Receiver<Request>) -> Waiting<'a> {
        Waiting {
            inbox,
            newest: None,
            closed: false,
        }
    }

    /// Takes in what has arrived. Returns whether a newer request did.
    pub(super) fn take_in(&mut self) -> bool {
        let mut newer = false;
        loop {
            match self.inbox.try_recv() {
                Ok(request) => {
                    self.newest = Some(request);
                    newer = true;
                }
                Err(TryRecvError::Empty) => return newer,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    return newer;
                }
            }
        }
    }

    /// The request to work on next, waiting at most `patience` for one to arrive.
    pub(super) fn next_within(&mut self, patience: Duration) -> Next {
        self.take_in();
        if self.newest.is_none() && !self.closed {
            match self.inbox.recv_timeout(patience) {
                Ok(request) => self.newest = Some(request),
                Err(RecvTimeoutError::Timeout) => return Next::Idle,
                Err(RecvTimeoutError::Disconnected) => self.closed = true,
            }
            self.take_in();
        }
        match self.newest.take() {
            Some(request) if !self.closed => Next::Request(Box::new(request)),
            _ => Next::Closed,
        }
    }

    /// The request to work on next: the newest waiting, or the next to
    /// arrive. `None` when nobody asks any more.
    pub(super) fn next(&mut self) -> Option<Request> {
        self.take_in();
        if self.newest.is_none() && !self.closed {
            self.newest = self.inbox.recv().ok();
            self.take_in();
        }
        if self.closed { None } else { self.newest.take() }
    }
}
