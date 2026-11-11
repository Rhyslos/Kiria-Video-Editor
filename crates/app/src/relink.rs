//! Media files that moved: noticed when the project opens or changes, and
//! found again in a folder the user picks.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, Project};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

/// What a search came back with.
pub struct Found {
    pub folder: PathBuf,
    pub files: Vec<(MediaId, PathBuf)>,
    pub looked_for: usize,
}

#[derive(Default)]
pub struct Relinker {
    /// The media (and where the project says it is) last checked.
    checked: Vec<(MediaId, PathBuf)>,
    checking: Option<JobHandle<Vec<MediaId>>>,
    /// Media the project has that isn't on disk.
    pub missing: Vec<MediaId>,
    dialog: Option<Receiver<Option<PathBuf>>>,
    searching: Option<(PathBuf, JobHandle<Found>)>,
}

impl Relinker {
    /// Checks again whenever the project's media changes. Call every frame;
    /// the disk is only touched on a worker.
    pub fn watch(&mut self, jobs: &JobSystem, ctx: &egui::Context, project: &Project) {
        if let Some(handle) = &self.checking
            && let Some(result) = handle.try_result()
        {
            self.missing = result.unwrap_or_default();
            if !self.missing.is_empty() {
                tracing::info!(count = self.missing.len(), "media files missing");
            }
            self.checking = None;
        }
        let now = media_list(project);
        if self.checking.is_none() && now != self.checked {
            self.checked = now;
            let (project, ctx) = (project.clone(), ctx.clone());
            self.checking = Some(jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "missing media",
                move |_| {
                    let missing = media::relink::missing(&project);
                    ctx.request_repaint();
                    Ok(missing)
                },
            ));
        }
    }

    /// Files are missing (or still being checked): the banner says so, so
    /// pictures that can't be shown need no error of their own.
    pub fn explains_errors(&self) -> bool {
        self.checking.is_some() || !self.missing.is_empty()
    }

    pub fn searching(&self) -> Option<&PathBuf> {
        self.searching.as_ref().map(|(f, _)| f)
    }

    /// Asks for a folder to look in (on its own thread: the dialog blocks).
    pub fn choose_folder(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.searching.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Where are the missing files?")
                .pick_folder();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some(rx);
    }

    /// Starts the search once a folder is picked, and returns its result
    /// when done.
    pub fn poll(&mut self, jobs: &JobSystem, project: &Project) -> Option<Found> {
        if let Some(rx) = &self.dialog
            && let Ok(picked) = rx.try_recv()
        {
            self.dialog = None;
            if let Some(folder) = picked {
                let wanted: Vec<_> = self
                    .missing
                    .iter()
                    .filter_map(|id| project.media.get(id))
                    .map(|m| (m.id, m.fingerprint.clone()))
                    .collect();
                let f = folder.clone();
                let handle = jobs.submit(
                    PoolKind::Analysis,
                    Priority::Interactive,
                    "find media",
                    move |job| {
                        let files = media::relink::find(&f, &wanted, || job.is_cancelled());
                        Ok(Found {
                            folder: f,
                            files,
                            looked_for: wanted.len(),
                        })
                    },
                );
                self.searching = Some((folder, handle));
            }
        }
        let (_, handle) = self.searching.as_ref()?;
        // Asked again until the search is done; it is quick to check.
        let result = handle.try_result()?;
        self.searching = None;
        match result {
            Ok(found) => Some(found),
            Err(err) => {
                tracing::warn!("{}", err.report());
                None
            }
        }
    }
}

/// The media and where the project says each file is.
fn media_list(project: &Project) -> Vec<(MediaId, PathBuf)> {
    let mut list: Vec<_> = project.media.values().map(|m| (m.id, m.path.clone())).collect();
    list.sort();
    list
}

/// "2 files can't be found: a.mkv, b.mkv" for the banner.
pub fn summary(project: &Project, missing: &[MediaId]) -> String {
    let names: Vec<String> = missing
        .iter()
        .filter_map(|id| project.media.get(id))
        .filter_map(|m| m.path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let more = names.len().saturating_sub(3);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    match names.len() {
        1 => format!("1 file can't be found: {shown}"),
        n => format!("{n} files can't be found: {shown}{tail}"),
    }
}

/// The outcome of a search, for the status line.
pub fn outcome(found: &Found) -> String {
    let folder = found.folder.file_name().map_or_else(
        || found.folder.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match found.files.len() {
        0 => format!("None of the missing files are in {folder} or its subfolders."),
        n if n == found.looked_for => format!("Found all {n} in {folder}."),
        n => format!(
            "Found {n} of {} in {folder}; look in another folder for the rest.",
            found.looked_for
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_reads_well() {
        let found = |n: usize| Found {
            folder: PathBuf::from("/media/Recordings"),
            files: (0..n).map(|i| (MediaId(i as u64), PathBuf::from("x"))).collect(),
            looked_for: 2,
        };
        assert_eq!(
            outcome(&found(0)),
            "None of the missing files are in Recordings or its subfolders."
        );
        assert_eq!(
            outcome(&found(1)),
            "Found 1 of 2 in Recordings; look in another folder for the rest."
        );
        assert_eq!(outcome(&found(2)), "Found all 2 in Recordings.");
    }
}
