//! The labels offered in every project ("everywhere"), as the editor keeps
//! them: read once at start (a few hundred bytes), changed from a file's
//! Labels menu in the media drawer ("Every project"), and saved on a worker.
//! Not project edits, so they make no undo step; see
//! `project::global_labels` for the file and the rules.

use crate::media_drawer::Everywhere as Change;
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::GlobalLabels;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Everywhere {
    pub labels: GlobalLabels,
    /// The newest save, while it runs.
    saving: Option<JobHandle<()>>,
    /// The file couldn't be read (newer or damaged): it is never written
    /// over, so nothing in it is lost.
    read_only: bool,
}

impl Everywhere {
    /// Reads the set; a file that can't be read is explained once and left
    /// alone (every project's own labels still work).
    pub fn load(data_dir: &Path, errors: &mut ui_kit::ErrorCenter) -> Everywhere {
        match GlobalLabels::load(data_dir) {
            Ok(labels) => Everywhere {
                labels,
                ..Everywhere::default()
            },
            Err(err) => {
                errors.push(&err);
                Everywhere {
                    read_only: true,
                    ..Everywhere::default()
                }
            }
        }
    }

    /// Offers a label everywhere or stops offering one, and saves the set on
    /// a worker. Returns the status line.
    pub fn change(&mut self, jobs: &JobSystem, data_dir: &Path, change: Change) -> diag::Result<String> {
        if self.read_only {
            return Err(diag::AppError::new("Labels for every project can't be changed")
                .why("Their file couldn't be read when the app started, so it is left as it is.")
                .fix("See the message about it in Diagnostics; each project's own labels still work."));
        }
        let line = match change {
            Change::Offer(name, color) => {
                self.labels.add(&name, color)?;
                format!("\"{name}\" is now offered in every project's Labels menu")
            }
            Change::StopOffering(name) => {
                self.labels.remove(&name);
                format!("\"{name}\" is no longer offered everywhere; projects that use it keep it")
            }
        };
        let (labels, dir) = (self.labels.clone(), data_dir.to_path_buf());
        self.saving = Some(jobs.submit(
            PoolKind::Analysis,
            Priority::Interactive,
            "save labels for every project",
            move |_| labels.save(&dir),
        ));
        Ok(line)
    }

    /// Why the last save failed, once it has.
    pub fn poll(&mut self) -> Option<diag::AppError> {
        let result = self.saving.as_ref()?.try_result()?;
        self.saving = None;
        result.err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::LabelColor;

    #[test]
    fn changes_are_saved_and_a_file_that_cant_be_read_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut errors = ui_kit::ErrorCenter::default();
        let mut set = Everywhere::load(dir.path(), &mut errors);
        set.change(
            &jobs,
            dir.path(),
            Change::Offer("Drone".into(), LabelColor::Green),
        )
        .unwrap();
        while set.saving.is_some() {
            assert!(set.poll().is_none());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(GlobalLabels::load(dir.path()).unwrap().labels.len(), 1);

        std::fs::write(
            project::global_labels::path(dir.path()),
            "(format_version: 99, labels: [])",
        )
        .unwrap();
        let mut newer = Everywhere::load(dir.path(), &mut errors);
        assert_eq!(errors.all().len(), 1, "explained");
        assert!(
            newer
                .change(&jobs, dir.path(), Change::StopOffering("Drone".into()))
                .is_err()
        );
    }
}
