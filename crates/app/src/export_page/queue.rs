//! The export queue along the bottom of the Export page: exports run in the
//! background one at a time, each with progress, pause and cancel, while
//! editing goes on. It belongs to the window, not to the page, so it keeps
//! running when another page is shown.
//!
//! Each job is the timeline as it was when it was added (later edits don't
//! change it). Export puts a job in line to run as soon as nothing else
//! does; Add to queue keeps it waiting until its Start is pressed.

use diag::AppError;
use engine::export::{Devices, Export, ExportSettings, Exported, Outcome, Progress};
use project::{Project, SequenceId};
use std::sync::Arc;
use std::time::Duration;

/// Where a job is.
#[derive(Debug)]
pub enum State {
    /// Added for later: waits until Start is pressed.
    Held,
    /// Runs as soon as nothing else does.
    Next,
    Running(Export),
    Done(Exported),
    /// What went wrong, and why (the error itself went to the window's
    /// error strip once).
    Failed {
        what: String,
        why: String,
    },
    Cancelled,
}

impl State {
    pub fn is_running(&self) -> bool {
        matches!(self, State::Running(_))
    }

    /// Finished one way or another: it can be removed or tried again.
    pub fn is_over(&self) -> bool {
        matches!(self, State::Done(_) | State::Failed { .. } | State::Cancelled)
    }
}

/// One export in the queue.
#[derive(Debug)]
pub struct Job {
    pub id: u64,
    /// The preset it was made with, or what it is ("Your settings").
    pub preset: String,
    pub settings: ExportSettings,
    /// What the page said when it was added: Edit brings it back.
    pub choice: super::choice::Choice,
    /// How large it should come to (shown when it is done, as the file's size isn't asked for).
    pub estimated_bytes: u64,
    project: Project,
    sequence: SequenceId,
    devices: Option<Devices>,
    pub state: State,
}

impl Job {
    pub fn new(
        preset: String,
        settings: ExportSettings,
        choice: super::choice::Choice,
        estimated_bytes: u64,
        (project, sequence): (Project, SequenceId),
        devices: Option<Devices>,
    ) -> Job {
        Job {
            id: 0,
            preset,
            settings,
            choice,
            estimated_bytes,
            project,
            sequence,
            devices,
            state: State::Held,
        }
    }

    /// The file's name.
    pub fn name(&self) -> String {
        self.settings.path.file_name().map_or_else(
            || self.settings.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    pub fn progress(&self) -> Option<Progress> {
        match &self.state {
            State::Running(export) => Some(export.progress()),
            _ => None,
        }
    }
}

/// Runs when a job's worker has news (a repaint, in the window).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// What came of a poll.
#[derive(Debug, Default)]
pub struct Polled {
    pub finished: Vec<Exported>,
    pub failed: Vec<AppError>,
}

pub struct Queue {
    jobs: Vec<Job>,
    next_id: u64,
    notify: Notify,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("jobs", &self.jobs)
            .finish_non_exhaustive()
    }
}

impl Default for Queue {
    fn default() -> Queue {
        Queue::new(Arc::new(|| {}))
    }
}

impl Queue {
    pub fn new(notify: Notify) -> Queue {
        Queue {
            jobs: Vec::new(),
            next_id: 1,
            notify,
        }
    }

    /// Sets what runs when a worker has news (the window's repaint).
    pub fn notify_with(&mut self, notify: Notify) {
        self.notify = notify;
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Whether an export runs (or waits to run, and will on the next poll).
    pub fn busy(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(j.state, State::Running(_) | State::Next))
    }

    /// Adds a job: to run as soon as nothing else does (`now`), or held
    /// until started. Returns its id.
    pub fn add(&mut self, mut job: Job, now: bool) -> u64 {
        job.id = self.next_id;
        self.next_id += 1;
        job.state = if now { State::Next } else { State::Held };
        let id = job.id;
        self.jobs.push(job);
        self.run_next();
        id
    }

    /// Puts a held, failed or cancelled job in line to run.
    pub fn start(&mut self, id: u64) {
        if let Some(job) = self.job_mut(id)
            && matches!(job.state, State::Held | State::Failed { .. } | State::Cancelled)
        {
            job.state = State::Next;
        }
        self.run_next();
    }

    pub fn pause(&mut self, id: u64, pause: bool) {
        if let Some(Job {
            state: State::Running(export),
            ..
        }) = self.job_mut(id)
        {
            if pause {
                export.pause();
            } else {
                export.resume();
            }
        }
    }

    /// Stops a running job (nothing is left behind), or takes a waiting
    /// one out of line.
    pub fn cancel(&mut self, id: u64) {
        if let Some(job) = self.job_mut(id) {
            match &job.state {
                State::Running(export) => export.cancel(),
                State::Next | State::Held => job.state = State::Cancelled,
                _ => {}
            }
        }
    }

    /// Takes a job that isn't running off the list. Returns it.
    pub fn remove(&mut self, id: u64) -> Option<Job> {
        let at = self
            .jobs
            .iter()
            .position(|j| j.id == id && !j.state.is_running())?;
        Some(self.jobs.remove(at))
    }

    /// Takes in what the workers did, and starts the next job when one
    /// ended. Cheap: call it every frame.
    pub fn poll(&mut self) -> Polled {
        let mut polled = Polled::default();
        for job in &mut self.jobs {
            let State::Running(export) = &job.state else {
                continue;
            };
            let Some(outcome) = export.finished() else {
                continue;
            };
            job.state = match outcome {
                Outcome::Done(done) => {
                    tracing::info!(
                        "exported {} frames to {} in {:.1} s with {}",
                        done.frames,
                        done.path.display(),
                        done.took.as_secs_f64(),
                        done.encoder
                    );
                    polled.finished.push(done.clone());
                    State::Done(done)
                }
                Outcome::Cancelled => State::Cancelled,
                Outcome::Failed(err) => {
                    let state = State::Failed {
                        what: err.what().to_owned(),
                        why: err.why_text().unwrap_or_default().to_owned(),
                    };
                    polled.failed.push(err);
                    state
                }
            };
        }
        self.run_next();
        polled
    }

    /// Stops the export under way, if any, and waits briefly for it to let
    /// go of its file: for closing the app. Jobs still waiting are dropped.
    pub fn stop(&mut self) {
        for job in &mut self.jobs {
            if let State::Running(export) = &job.state {
                export.cancel();
                let give_up = std::time::Instant::now() + Duration::from_secs(5);
                while export.finished().is_none() && std::time::Instant::now() < give_up {
                    std::thread::sleep(Duration::from_millis(20));
                }
                job.state = State::Cancelled;
            }
        }
    }

    fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Starts the first job in line when none runs.
    fn run_next(&mut self) {
        if self.jobs.iter().any(|j| j.state.is_running()) {
            return;
        }
        let Some(job) = self.jobs.iter_mut().find(|j| matches!(j.state, State::Next)) else {
            return;
        };
        let notify = Arc::clone(&self.notify);
        job.state = State::Running(Export::start(
            job.project.clone(),
            job.sequence,
            job.settings.clone(),
            job.devices.clone(),
            move || notify(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_page::choice::Choice;
    use engine::export::{AudioCodec, Container};
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    fn recording() -> (Project, SequenceId) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/game-3-audio.mkv");
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture).unwrap();
        (
            project::Command::Import { sequence: seq, media }
                .apply(&project)
                .unwrap(),
            seq,
        )
    }

    fn sound_job(path: PathBuf, timeline: (Project, SequenceId)) -> Job {
        let settings = ExportSettings {
            container: Container::Wav,
            video: None,
            audio: AudioCodec::Pcm24,
            ..ExportSettings::mp4(path)
        };
        Job::new(
            "Audio only".into(),
            settings,
            Choice::default(),
            0,
            timeline,
            None,
        )
    }

    fn until(queue: &mut Queue, done: impl Fn(&Queue) -> bool) -> Polled {
        let give_up = Instant::now() + Duration::from_secs(60);
        let mut all = Polled::default();
        while !done(queue) {
            assert!(Instant::now() < give_up, "the queue did not get there");
            let polled = queue.poll();
            all.finished.extend(polled.finished);
            all.failed.extend(polled.failed);
            std::thread::sleep(Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn jobs_run_one_at_a_time_and_held_ones_wait_for_start() {
        let dir = tempfile::tempdir().unwrap();
        let mut queue = Queue::default();
        let first = queue.add(sound_job(dir.path().join("a.wav"), recording()), true);
        let held = queue.add(sound_job(dir.path().join("b.wav"), recording()), false);
        let second = queue.add(sound_job(dir.path().join("c.wav"), recording()), true);
        assert!(queue.jobs()[0].state.is_running());
        assert!(matches!(queue.jobs()[1].state, State::Held));
        assert!(
            matches!(queue.jobs()[2].state, State::Next),
            "waits for the first"
        );
        let polled = until(&mut queue, |q| q.jobs()[2].state.is_over());
        assert_eq!(polled.finished.len(), 2);
        assert!(polled.failed.is_empty());
        assert!(dir.path().join("a.wav").exists() && dir.path().join("c.wav").exists());
        assert!(matches!(queue.jobs()[1].state, State::Held), "still held");
        assert!(!dir.path().join("b.wav").exists());
        queue.start(held);
        until(&mut queue, |q| q.jobs()[1].state.is_over());
        assert!(dir.path().join("b.wav").exists());
        assert_eq!(queue.jobs()[0].name(), "a.wav");
        assert!(queue.remove(first).is_some());
        assert!(queue.remove(second).is_some());
        assert_eq!(queue.jobs().len(), 1);
        assert!(!queue.busy());
    }

    #[test]
    fn a_job_pauses_cancels_and_fails_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mut queue = Queue::default();
        let id = queue.add(sound_job(dir.path().join("a.wav"), recording()), true);
        queue.pause(id, true);
        std::thread::sleep(Duration::from_millis(100));
        queue.poll();
        assert!(queue.jobs()[0].progress().is_some_and(|p| p.paused));
        assert!(queue.remove(id).is_none(), "a running job can't be removed");
        queue.cancel(id);
        until(&mut queue, |q| q.jobs()[0].state.is_over());
        assert!(matches!(queue.jobs()[0].state, State::Cancelled));
        assert!(!dir.path().join("a.wav").exists());
        // Tried again, it runs to the end.
        queue.start(id);
        until(&mut queue, |q| q.jobs()[0].state.is_over());
        assert!(matches!(queue.jobs()[0].state, State::Done(_)));

        // An empty timeline fails, and says why once.
        let empty = Project::new("T", project::SequenceSettings::GAME_CLIP);
        let seq = empty.active_sequence;
        queue.add(sound_job(dir.path().join("e.wav"), (empty, seq)), true);
        let polled = until(&mut queue, |q| q.jobs()[1].state.is_over());
        assert_eq!(polled.failed.len(), 1);
        assert!(matches!(queue.jobs()[1].state, State::Failed { .. }));
        // A held job cancelled is taken out of line.
        let held = queue.add(sound_job(dir.path().join("h.wav"), recording()), false);
        queue.cancel(held);
        assert!(matches!(queue.jobs()[2].state, State::Cancelled));
    }
}
