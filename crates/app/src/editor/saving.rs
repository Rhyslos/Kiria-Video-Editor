//! Saving: Ctrl+S and the idle autosave (a snapshot on the interface
//! thread, the write on a worker, one at a time), and the launcher's picture
//! of the project written after a save.

use super::*;

/// How long closing the app waits for the launcher's picture being
/// written (one small frame).
pub(super) const THUMBNAIL_ENDS_WITHIN: Duration = Duration::from_secs(5);

/// Records what a finished job writing the launcher's picture means for
/// `shows`, what the picture is known to show: after a failure, nothing.
fn thumbnail_finished(result: diag::Result<bool>, shows: &mut Option<engine::thumbnail::Shows>) {
    if let Err(err) = result {
        tracing::info!("no project picture: {}", err.report());
        *shows = None;
    }
}

/// Waits at most `within` for `job`, the launcher's picture being written,
/// and records its outcome (see [`thumbnail_finished`]). Whether nothing is
/// writing the picture any more, so it may be written now.
pub(super) fn thumbnail_settled(
    job: Option<JobHandle<bool>>,
    within: Duration,
    shows: &mut Option<engine::thumbnail::Shows>,
) -> bool {
    let Some(job) = job else { return true };
    match job.wait_within(within) {
        Some(result) => {
            thumbnail_finished(result, shows);
            true
        }
        None => false,
    }
}

/// A full save running on a worker.
pub(super) struct PendingSave {
    /// The newest journal entry it includes.
    pub(super) seq: u64,
    /// Asked for (Ctrl+S): its end is said in the status line.
    pub(super) asked: bool,
    pub(super) handle: JobHandle<()>,
}

/// Whether the launcher's picture of the project needs writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Thumbnail {
    No,
    /// On opening: only for a project that has none yet.
    IfMissing,
    Now,
}

impl Editor {
    /// Ctrl+S: a snapshot now (cheap: the project shares its data), written
    /// on a worker. It waits there behind an autosave under way, and an
    /// older snapshot never replaces it (`project::document`). The top bar
    /// says "saving…" until it is done; the status line says how it went.
    pub(super) fn save_now(&mut self) -> diag::Result<()> {
        let mut job = None;
        self.with_document(|d| {
            if d.is_read_only() {
                return d.save(); // refused, with the reason
            }
            job = Some(d.prepare_save());
            Ok(())
        })?;
        let Some(job) = job else {
            return Ok(());
        };
        self.submit_save(job, true);
        self.status = "Saving…".into();
        Ok(())
    }

    fn submit_save(&mut self, job: project::document::SaveJob, asked: bool) {
        let seq = job.journal_seq();
        // Asked for: it runs even while playing. An autosave waits.
        let priority = if asked {
            Priority::Interactive
        } else {
            Priority::Background
        };
        let name = if asked { "save" } else { "autosave" };
        let handle = self
            .jobs
            .submit_write(PoolKind::Export, priority, name, move |_| job.run());
        self.pending_saves.push(PendingSave { seq, asked, handle });
    }

    /// Hands due saves to a worker and collects finished ones.
    pub(super) fn autosave(&mut self, now: Instant, idle: bool) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let mut finished = Vec::new();
        self.pending_saves.retain(|save| match save.handle.try_result() {
            Some(result) => {
                finished.push((save.seq, save.asked, result));
                false
            }
            None => true,
        });
        for (seq, asked, result) in finished {
            match document.finish_save(seq, result) {
                Ok(()) => {
                    self.thumbnail_due = Thumbnail::Now;
                    if asked {
                        self.status = "Saved.".into();
                    }
                }
                Err(err) => {
                    if asked {
                        self.status = "The project could not be saved.".into();
                    }
                    self.errors.push(&err);
                }
            }
        }
        // A journal that can't be written keeps the edit on screen; the
        // person is told here, once.
        if let Some(err) = document.take_journal_problem() {
            self.errors.push(&err);
        }
        if let Some(job) = document.tick(now, idle) {
            self.submit_save(job, false);
        }
    }

    /// Writes the launcher's picture of the project on a worker when due:
    /// the frame at the playhead, or the first clip's in a gap.
    pub(super) fn thumbnails(&mut self) {
        if let Some(handle) = &self.thumbnail
            && let Some(result) = handle.try_result()
        {
            thumbnail_finished(result, &mut self.thumbnail_shows);
            self.thumbnail = None;
        }
        let Some(document) = &self.document else { return };
        if self.thumbnail.is_some() || self.thumbnail_due == Thumbnail::No || document.is_read_only() {
            return;
        }
        let only_if_missing = self.thumbnail_due == Thumbnail::IfMissing;
        self.thumbnail_due = Thumbnail::No;
        let project = document.project();
        let Some(shows) = engine::thumbnail::shows(project, project.active_sequence, self.viewer.playhead)
        else {
            return;
        };
        // After a save that left the picture as it was (the playhead on the
        // same frame of the same file), nothing is decoded again.
        if self.thumbnail_shows.as_ref() == Some(&shows) {
            return;
        }
        // When only written if missing, what the one there shows is unknown.
        self.thumbnail_shows = (!only_if_missing).then(|| shows.clone());
        let out = project::thumbnail_path(document.path());
        // Written even when closing comes first: closing waits for it.
        self.thumbnail = Some(self.jobs.submit_write(
            PoolKind::Analysis,
            Priority::Background,
            "project picture",
            move |_| {
                if only_if_missing && out.exists() {
                    return Ok(false);
                }
                engine::thumbnail::write_shown(&shows, &out).map(|()| true)
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closing waits for the launcher's picture being written before it
    /// writes one itself, and learns what that one shows.
    #[test]
    fn closing_waits_for_the_picture_being_written() {
        use jobs::{BackgroundPolicy, JobSystem, PoolKind, Priority, Resolved};
        let jobs = JobSystem::new(Resolved {
            worker_threads: 2,
            ram_cache_mb: 64,
            disk_cache_gb: 1,
            background: BackgroundPolicy::Always,
        });
        let shown = engine::thumbnail::Shows {
            path: "clip.mkv".into(),
            stream: 0,
            rotation: 0,
            at: Time::ZERO,
        };
        let gated = |result: diag::Result<bool>| {
            let (go, gate) = std::sync::mpsc::channel::<()>();
            let job = jobs.submit_write(PoolKind::Analysis, Priority::Background, "picture", move |_| {
                let _ = gate.recv();
                result
            });
            (go, job)
        };
        assert!(
            thumbnail_settled(None, Duration::ZERO, &mut None),
            "nothing running"
        );

        // Still writing after the wait: not free, and nothing learnt.
        let (go, job) = gated(Ok(true));
        let mut shows = Some(shown.clone());
        assert!(!thumbnail_settled(
            Some(job),
            Duration::from_millis(50),
            &mut shows
        ));
        assert_eq!(shows.as_ref(), Some(&shown));
        drop(go);

        // Ends while closing waits: free, and what it wrote is known.
        let (go, job) = gated(Ok(true));
        let opens = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = go.send(());
        });
        assert!(thumbnail_settled(Some(job), Duration::from_secs(60), &mut shows));
        assert_eq!(
            shows.as_ref(),
            Some(&shown),
            "the same picture is not written again"
        );
        opens.join().unwrap();

        // A failed write leaves what the picture shows unknown.
        let (go, job) = gated(Err(diag::AppError::new("test")));
        go.send(()).unwrap();
        assert!(thumbnail_settled(Some(job), Duration::from_secs(60), &mut shows));
        assert_eq!(shows, None);
    }
}
