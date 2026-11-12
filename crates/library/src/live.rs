//! The index on a thread of its own, so the interface never waits for it.
//!
//! The interface hands over each new project snapshot ([`Live::follow`],
//! which only sends one when the media, the library or the timelines
//! changed) and asks queries under a name ([`Live::ask`]); the worker
//! answers the newest of each and publishes the answer, which the
//! interface reads without waiting ([`Live::answer`]). After every sync
//! the worker answers every asked query again, so results follow edits.

use crate::index::Index;
use crate::query::Query;
use project::{MediaId, Project};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Who asked: each asker keeps its own newest query and answer.
pub type Asker = &'static str;

/// The worker's answer to a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub query: Query,
    pub media: Vec<MediaId>,
    /// How long the database took.
    pub took: Duration,
}

enum Message {
    Sync(Box<Project>),
    Ask(Asker, Query),
    Forget,
}

#[derive(Default)]
struct Shared {
    answers: HashMap<Asker, Arc<Answer>>,
    error: Option<diag::AppError>,
    /// Snapshots synced so far, so the interface can tell when its edits are in.
    synced: u64,
}

/// The index, running on its own thread.
pub struct Live {
    send: Sender<Message>,
    shared: Arc<Mutex<Shared>>,
    /// What was last sent, to send again only on change.
    followed: Option<Project>,
    asked: HashMap<Asker, Query>,
    sent: u64,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("sent", &self.sent)
            .finish_non_exhaustive()
    }
}

impl Live {
    /// Starts the worker. `wake` is called after each new answer (the
    /// interface asks for a repaint there).
    pub fn start(wake: impl Fn() + Send + 'static) -> Live {
        let (send, receive) = channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("library index".into())
            .spawn(move || run(&receive, &worker_shared, &wake));
        if let Err(err) = spawned {
            tracing::warn!("the library index thread didn't start: {err}");
            lock(&shared).error = Some(
                diag::AppError::new("The media search can't start")
                    .why(format!("The system refused a new thread: {err}."))
                    .fix("Close other programs and open the project again."),
            );
        }
        Live {
            send,
            shared,
            followed: None,
            asked: HashMap::new(),
            sent: 0,
        }
    }

    /// Hands the worker `project` when its media, library or timelines
    /// differ from the last one handed over. Cheap enough for every frame:
    /// snapshots share unchanged parts, so this compares pointers.
    pub fn follow(&mut self, project: &Project) {
        let same = self.followed.as_ref().is_some_and(|old| {
            old.media.ptr_eq(&project.media)
                && old.library.labels.ptr_eq(&project.library.labels)
                && old.library.tagged.ptr_eq(&project.library.tagged)
                && old.library.notes.ptr_eq(&project.library.notes)
                && old.library.lists.ptr_eq(&project.library.lists)
                && old.sequences.ptr_eq(&project.sequences)
        });
        if same {
            return;
        }
        self.followed = Some(project.clone());
        self.sent += 1;
        let _ = self.send.send(Message::Sync(Box::new(project.clone())));
    }

    /// Drops the project (it was closed): answers are forgotten.
    pub fn forget(&mut self) {
        self.followed = None;
        self.asked.clear();
        let _ = self.send.send(Message::Forget);
    }

    /// Asks `query` for `asker`, unless it is what that asker asked last.
    pub fn ask(&mut self, asker: Asker, query: &Query) {
        if self.asked.get(asker) == Some(query) {
            return;
        }
        self.asked.insert(asker, query.clone());
        let _ = self.send.send(Message::Ask(asker, query.clone()));
    }

    /// The newest answer for `asker`: perhaps to an earlier query, or from
    /// before the last edit, while the worker catches up.
    pub fn answer(&self, asker: Asker) -> Option<Arc<Answer>> {
        lock(&self.shared).answers.get(asker).cloned()
    }

    /// Whether the newest answer for `asker` is to the query it asked last
    /// and the project as last followed.
    pub fn is_current(&self, asker: Asker) -> bool {
        let shared = lock(&self.shared);
        shared.synced == self.sent
            && shared
                .answers
                .get(asker)
                .is_some_and(|a| self.asked.get(asker) == Some(&a.query))
    }

    /// Why the index stopped working, once.
    pub fn take_error(&self) -> Option<diag::AppError> {
        lock(&self.shared).error.take()
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The worker: takes everything waiting, keeps the newest project and the
/// newest query per asker, then syncs and answers.
fn run(receive: &Receiver<Message>, shared: &Mutex<Shared>, wake: &dyn Fn()) {
    let mut index = match Index::new() {
        Ok(index) => index,
        Err(err) => {
            lock(shared).error = Some(err);
            wake();
            return;
        }
    };
    let mut queries: HashMap<Asker, Query> = HashMap::new();
    while let Ok(first) = receive.recv() {
        let mut project = None;
        let mut synced = 0;
        let mut asked: Vec<Asker> = Vec::new();
        let mut forget = false;
        for message in std::iter::once(first).chain(receive.try_iter()) {
            match message {
                Message::Sync(p) => {
                    project = Some(p);
                    synced += 1;
                }
                Message::Ask(asker, query) => {
                    queries.insert(asker, query);
                    if !asked.contains(&asker) {
                        asked.push(asker);
                    }
                }
                Message::Forget => {
                    forget = true;
                    project = None;
                }
            }
        }
        if forget {
            if let Ok(empty) = Index::new() {
                index = empty;
            }
            let mut s = lock(shared);
            s.answers.clear();
            queries.retain(|asker, _| asked.contains(asker));
        }
        let mut all = asked;
        if let Some(project) = project {
            let started = Instant::now();
            match index.sync(&project) {
                Ok(done) => tracing::debug!(?done, took = ?started.elapsed(), "library index synced"),
                Err(err) => lock(shared).error = Some(err),
            }
            all = queries.keys().copied().collect();
        }
        for asker in all {
            let Some(query) = queries.get(asker) else { continue };
            let started = Instant::now();
            match index.search(query) {
                Ok(media) => {
                    let answer = Answer {
                        query: query.clone(),
                        media,
                        took: started.elapsed(),
                    };
                    lock(shared).answers.insert(asker, Arc::new(answer));
                }
                Err(err) => lock(shared).error = Some(err),
            }
        }
        lock(shared).synced += synced;
        wake();
    }
}
