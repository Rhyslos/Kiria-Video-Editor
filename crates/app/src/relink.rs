//! Media files that moved: noticed when the project opens or changes, and
//! found again.
//!
//! When the project opens with files missing, they are looked for once, by
//! themselves, in the folders the app knows (`Known`): the project's own
//! folder, the launcher's project, media and other folders, and any
//! `--watch-folder`. The search runs on a worker, within
//! `media::relink::Limits::AUTOMATIC` (eight folders deep, 100 000
//! entries, 20 seconds), and hashes only files of a missing file's size.
//! What it finds is relinked as one undoable step, and the status line
//! names the files; the rest stay in the banner for "Find them…", which
//! searches a folder the person picks and relinks what it finds the same way.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{MediaId, Project};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// What a search came back with.
pub struct Found {
    /// The folder picked; `None` for the automatic search.
    pub folder: Option<PathBuf>,
    pub files: Vec<(MediaId, PathBuf)>,
    pub looked_for: usize,
}

/// Where the automatic search looks.
#[derive(Debug, Clone)]
pub struct Known {
    /// The open project's file: its folder is looked through first.
    pub project_file: PathBuf,
    /// The launcher's list of folders (read on the worker; none when it
    /// isn't there).
    pub index_file: PathBuf,
    /// Folders watched besides the launcher's (`--watch-folder`).
    pub extra: Vec<PathBuf>,
}

/// The automatic search, once per opened project.
#[derive(Default)]
enum Automatic {
    #[default]
    NotYet,
    Searching(JobHandle<Found>),
    Done,
}

#[derive(Default)]
pub struct Relinker {
    /// The media (and where the project says it is) last checked.
    checked: Vec<(MediaId, PathBuf)>,
    checking: Option<JobHandle<Vec<MediaId>>>,
    /// Media the project has that isn't on disk.
    pub missing: Vec<MediaId>,
    /// The first check since the project opened has come back.
    first_check_done: bool,
    automatic: Automatic,
    known: Option<Known>,
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
            self.first_check_done = true;
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

    /// Whether the first check, and the automatic search when files are
    /// missing, are done (for screenshots).
    pub fn settled(&self) -> bool {
        self.known.is_none()
            || (self.first_check_done && self.checking.is_none() && matches!(self.automatic, Automatic::Done))
    }

    /// Where the automatic search looks; set once a project is open.
    pub fn know(&mut self, known: Known) {
        if self.known.is_none() {
            self.known = Some(known);
        }
    }

    /// What a search under way is doing, for the banner.
    pub fn searching(&self) -> Option<String> {
        if let Some((folder, _)) = &self.searching {
            return Some(format!("Looking in {}…", folder.display()));
        }
        matches!(self.automatic, Automatic::Searching(_)).then(|| "Looking in your folders…".to_owned())
    }

    /// Starts the automatic search once the first check found files
    /// missing (once per opened project; `read_only` projects are left as
    /// they are), and returns what it found when done.
    pub fn automatic(&mut self, jobs: &JobSystem, project: &Project, read_only: bool) -> Option<Found> {
        if matches!(self.automatic, Automatic::NotYet) && self.first_check_done {
            if self.missing.is_empty() || read_only {
                self.automatic = Automatic::Done;
                return None;
            }
            let known = self.known.clone()?;
            let wanted = wanted(project, &self.missing);
            self.automatic = Automatic::Searching(jobs.submit(
                PoolKind::Analysis,
                Priority::Interactive,
                "find moved media",
                move |job| {
                    let folders = known_folders(&known);
                    tracing::info!(
                        folders = folders.len(),
                        files = wanted.len(),
                        "looking for moved media"
                    );
                    let files = media::relink::find_in_known(&folders, &wanted, || job.is_cancelled());
                    Ok(Found {
                        folder: None,
                        files,
                        looked_for: wanted.len(),
                    })
                },
            ));
        }
        let Automatic::Searching(handle) = &self.automatic else {
            return None;
        };
        let result = handle.try_result()?;
        self.automatic = Automatic::Done;
        match result {
            Ok(found) => Some(found),
            Err(err) => {
                tracing::warn!("{}", err.report());
                None
            }
        }
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
                let wanted = wanted(project, &self.missing);
                let f = folder.clone();
                let handle = jobs.submit(
                    PoolKind::Analysis,
                    Priority::Interactive,
                    "find media",
                    move |job| {
                        let files = media::relink::find(&f, &wanted, || job.is_cancelled());
                        Ok(Found {
                            folder: Some(f),
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

/// The missing files' fingerprints.
fn wanted(project: &Project, missing: &[MediaId]) -> Vec<(MediaId, project::Fingerprint)> {
    missing
        .iter()
        .filter_map(|id| project.media.get(id))
        .map(|m| (m.id, m.fingerprint.clone()))
        .collect()
}

/// The folders the automatic search looks through, in order: the
/// project's own, then the launcher's (project folders, media folders,
/// other folders, as listed), then any watched besides. Reads the
/// launcher's list: on a worker.
pub fn known_folders(known: &Known) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(own) = known.project_file.parent() {
        let own = if own.as_os_str().is_empty() {
            Path::new(".")
        } else {
            own
        };
        folders.push(own.to_path_buf());
    }
    if known.index_file.exists() {
        match project_index::Index::open(&known.index_file).and_then(|index| index.folders()) {
            Ok(listed) => folders.extend(listed.into_iter().map(|f| f.path)),
            Err(err) => tracing::warn!("the launcher's folders can't be read: {}", err.report()),
        }
    }
    folders.extend(known.extra.iter().cloned());
    folders
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

/// The outcome of a search, for the status line. `None` when the
/// automatic search found nothing (the banner still says what is missing).
pub fn outcome(project: &Project, found: &Found, undo: &str) -> Option<String> {
    let Some(folder) = &found.folder else {
        return automatic_outcome(project, found, undo);
    };
    let folder = folder.file_name().map_or_else(
        || folder.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    Some(match found.files.len() {
        0 => format!("None of the missing files are in {folder} or its subfolders."),
        n if n == found.looked_for => format!("Found all {n} in {folder}."),
        n => format!(
            "Found {n} of {} in {folder}; look in another folder for the rest.",
            found.looked_for
        ),
    })
}

/// "Found 2 moved files in your folders and relinked them: a.mkv, b.mkv.
/// Ctrl+Z undoes it."
fn automatic_outcome(project: &Project, found: &Found, undo: &str) -> Option<String> {
    let mut names: Vec<String> = found
        .files
        .iter()
        .filter_map(|(id, _)| project.media.get(id))
        .filter_map(|m| m.path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let more = match names.len().saturating_sub(3) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    let left = found.looked_for.saturating_sub(found.files.len());
    let rest = match left {
        0 => String::new(),
        1 => " 1 is still missing.".to_owned(),
        n => format!(" {n} are still missing."),
    };
    match found.files.len() {
        0 => None,
        1 => Some(format!(
            "Found a moved file in your folders and relinked it: {shown}.{rest} {undo} undoes it."
        )),
        n => Some(format!(
            "Found {n} moved files in your folders and relinked them: {shown}{more}.{rest} {undo} undoes it."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_reads_well() {
        let found = |n: usize| Found {
            folder: Some(PathBuf::from("/media/Recordings")),
            files: (0..n).map(|i| (MediaId(i as u64), PathBuf::from("x"))).collect(),
            looked_for: 2,
        };
        let p = Project::new("P", project::SequenceSettings::GAME_CLIP);
        let says = |n| outcome(&p, &found(n), "Ctrl+Z").unwrap();
        assert_eq!(
            says(0),
            "None of the missing files are in Recordings or its subfolders."
        );
        assert_eq!(
            says(1),
            "Found 1 of 2 in Recordings; look in another folder for the rest."
        );
        assert_eq!(says(2), "Found all 2 in Recordings.");
    }

    fn with_files(names: &[&str]) -> Project {
        let mut p = Project::new("P", project::SequenceSettings::GAME_CLIP);
        for name in names {
            p = project::Command::AddMedia {
                media: project::MediaRef {
                    id: MediaId(0),
                    path: PathBuf::from("/old").join(name),
                    fingerprint: project::Fingerprint {
                        size: 1,
                        sample_hash: (*name).into(),
                    },
                    kind: project::MediaKind::Video,
                    duration: project::Time::seconds(1),
                    info: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        p
    }

    #[test]
    fn the_automatic_search_names_what_it_relinked_and_what_is_still_missing() {
        let p = with_files(&["a.mkv", "b.mkv", "c.mkv", "d.mkv", "e.mkv"]);
        let ids: Vec<MediaId> = p.media.keys().copied().collect();
        let found = |n: usize| Found {
            folder: None,
            files: ids[..n].iter().map(|id| (*id, PathBuf::from("x"))).collect(),
            looked_for: 5,
        };
        assert_eq!(outcome(&p, &found(0), "Ctrl+Z"), None);
        assert_eq!(
            outcome(&p, &found(1), "Ctrl+Z").unwrap(),
            "Found a moved file in your folders and relinked it: a.mkv. 4 are still missing. Ctrl+Z undoes it."
        );
        assert_eq!(
            outcome(&p, &found(5), "Ctrl+Z").unwrap(),
            "Found 5 moved files in your folders and relinked them: a.mkv, b.mkv, c.mkv and 2 more. Ctrl+Z undoes it."
        );
    }

    #[test]
    fn known_folders_start_with_the_projects_own_then_the_launchers() {
        let dir = tempfile::tempdir().unwrap();
        let index_file = dir.path().join("index.db");
        let index = project_index::Index::open(&index_file).unwrap();
        index
            .add_folder(&dir.path().join("Recordings"), project_index::FolderKind::Media)
            .unwrap();
        index
            .add_folder(&dir.path().join("Projects"), project_index::FolderKind::Projects)
            .unwrap();
        drop(index);
        let known = Known {
            project_file: dir.path().join("Work/Trip.project"),
            index_file,
            extra: vec![dir.path().join("Watched")],
        };
        let folders = known_folders(&known);
        assert_eq!(folders.first(), Some(&dir.path().join("Work")));
        assert_eq!(folders.last(), Some(&dir.path().join("Watched")));
        assert!(folders.contains(&dir.path().join("Recordings")));
        assert!(folders.contains(&dir.path().join("Projects")));

        let without_list = Known {
            index_file: dir.path().join("none.db"),
            ..known
        };
        assert_eq!(known_folders(&without_list).len(), 2);
    }

    #[test]
    fn moved_files_are_found_by_themselves_in_a_known_folder_and_relinked_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("Recordings");
        std::fs::create_dir_all(&rec).unwrap();
        let (a, b) = (rec.join("a.mkv"), rec.join("b.mkv"));
        std::fs::write(&a, b"first recording").unwrap();
        std::fs::write(&b, b"second, longer recording").unwrap();
        let mut p = Project::new("P", project::SequenceSettings::GAME_CLIP);
        for path in [&a, &b] {
            p = project::Command::AddMedia {
                media: project::MediaRef {
                    id: MediaId(0),
                    path: path.clone(),
                    fingerprint: media::fingerprint(path).unwrap(),
                    kind: project::MediaKind::Video,
                    duration: project::Time::seconds(1),
                    info: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        // Both move into a dated folder inside the media folder; one is renamed.
        let dated = rec.join("2026-11");
        std::fs::create_dir_all(&dated).unwrap();
        std::fs::rename(&a, dated.join("a.mkv")).unwrap();
        std::fs::rename(&b, dated.join("renamed.mkv")).unwrap();
        let file = dir.path().join("Work/P.project");
        let missing = media::relink::missing(&p);
        assert_eq!(missing.len(), 2);

        let mut relinker = Relinker {
            missing,
            first_check_done: true,
            ..Relinker::default()
        };
        relinker.know(Known {
            project_file: file,
            index_file: dir.path().join("no index"),
            extra: vec![rec.clone()],
        });
        let jobs = JobSystem::new(jobs::Budgets::default().resolve(&jobs::Machine::detect()));
        let mut found = None;
        let started = std::time::Instant::now();
        while found.is_none() && started.elapsed() < std::time::Duration::from_secs(20) {
            found = relinker.automatic(&jobs, &p, false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let found = found.expect("the search ends");
        assert_eq!(found.looked_for, 2);
        let after = project::Command::RelinkFiles { files: found.files }
            .apply(&p)
            .unwrap();
        assert!(media::relink::missing(&after).is_empty());
        assert!(relinker.automatic(&jobs, &after, false).is_none(), "only once");
    }
}
