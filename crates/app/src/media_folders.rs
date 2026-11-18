//! New recordings in the media folders (the launcher's "Media folders",
//! and `--watch-folder` for checks), watched while a project is open
//! (`library::Watcher`, on its own thread). Files changed since the project
//! was last saved, finished (no longer growing) and not in the project yet
//! show under "New in your folders" on the Library page and in the media drawer; nothing is added
//! by itself. The status line says when one appears.

use library::Found;
use project::Project;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Default)]
pub struct MediaFolders {
    watcher: Option<library::Watcher>,
    /// Folders watched besides the launcher's (`--watch-folder`).
    extra: Vec<PathBuf>,
    /// Files hidden with × this session.
    dismissed: HashSet<PathBuf>,
    /// Files the status line has told about.
    announced: HashSet<PathBuf>,
    /// The project's media as last looked at, and its files' full paths.
    known_for: Option<Project>,
    known: HashSet<PathBuf>,
    /// The watcher's list `new_files` last filtered and what it gave:
    /// filtered again only when the list, the project's media or the
    /// dismissed files change, not every frame.
    shown: Option<Shown>,
}

/// The watcher's list and the new files in it.
type Shown = (Arc<Vec<Found>>, Arc<[Found]>);

impl MediaFolders {
    pub fn new(extra: Vec<PathBuf>) -> MediaFolders {
        MediaFolders {
            extra,
            ..MediaFolders::default()
        }
    }

    /// Starts watching, once, for the project at `project_file`: new means
    /// changed after its last save. The launcher's folders are read from
    /// its index file before every look, on the watcher's thread.
    pub fn start(&mut self, ctx: &egui::Context, project_file: &Path, index_file: PathBuf) {
        if self.watcher.is_some() {
            return;
        }
        let since = std::fs::metadata(project_file)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now());
        let extra = self.extra.clone();
        let folders = Box::new(move || {
            let mut folders = launcher_media_folders(&index_file);
            folders.extend(extra.iter().cloned());
            folders
        });
        let ctx = ctx.clone();
        self.watcher = Some(library::Watcher::start(folders, since, move || {
            ctx.request_repaint()
        }));
    }

    /// Folders watched besides the launcher's (`--watch-folder`).
    pub fn extra(&self) -> &[PathBuf] {
        &self.extra
    }

    /// New files not in `project` and not dismissed, newest first.
    pub fn new_files(&mut self, project: &Project) -> Arc<[Found]> {
        let Some(watcher) = &self.watcher else {
            return Arc::default();
        };
        let found = watcher.found();
        self.new_among(found, project)
    }

    /// The files in `found` not in `project` and not dismissed, kept until
    /// one of the three changes.
    fn new_among(&mut self, found: Arc<Vec<Found>>, project: &Project) -> Arc<[Found]> {
        if found.is_empty() {
            return Arc::default();
        }
        if !self
            .known_for
            .as_ref()
            .is_some_and(|p| p.media.ptr_eq(&project.media))
        {
            self.known = project.media.values().map(|m| full_path(&m.path)).collect();
            self.known_for = Some(project.clone());
            self.shown = None;
        }
        if let Some((from, files)) = &self.shown
            && Arc::ptr_eq(from, &found)
        {
            return Arc::clone(files);
        }
        let files: Arc<[Found]> = found
            .iter()
            .filter(|f| !self.dismissed.contains(&f.path) && !self.known.contains(&full_path(&f.path)))
            .cloned()
            .collect();
        self.shown = Some((found, Arc::clone(&files)));
        files
    }

    /// Whether the folders have been looked through and nothing new is
    /// still being written (for screenshots).
    pub fn settled(&self) -> bool {
        self.watcher
            .as_ref()
            .is_none_or(|w| w.looks() >= 2 && w.settling() == 0)
    }

    /// Hides a file for the rest of the session.
    pub fn dismiss(&mut self, path: PathBuf) {
        self.dismissed.insert(path);
        self.shown = None;
    }

    /// A line for the status bar when files appeared that it hasn't told
    /// about yet.
    pub fn announce(&mut self, new: &[Found]) -> Option<String> {
        let fresh: Vec<&Found> = new.iter().filter(|f| !self.announced.contains(&f.path)).collect();
        let first = fresh.first()?;
        let text = match fresh.len() {
            1 => format!(
                "{} is new in your media folders: it is under New in your folders (Library page, media drawer), ready to add",
                library::file_name(&first.path)
            ),
            n => format!(
                "{n} new files in your media folders: they are under New in your folders (Library page, media drawer), ready to add"
            ),
        };
        self.announced.extend(fresh.iter().map(|f| f.path.clone()));
        Some(text)
    }
}

/// The launcher's media folders, or none when it has no index yet (the
/// file is never made here).
fn launcher_media_folders(index_file: &Path) -> Vec<PathBuf> {
    if !index_file.exists() {
        return Vec::new();
    }
    match project_index::Index::open(index_file).and_then(|index| index.folders()) {
        Ok(folders) => folders
            .into_iter()
            .filter(|f| f.kind == project_index::FolderKind::Media)
            .map(|f| f.path)
            .collect(),
        Err(err) => {
            tracing::warn!("media folders can't be read: {}", err.report());
            Vec::new()
        }
    }
}

/// A path made absolute without touching the disk, to compare paths given
/// relative to where the editor started with ones found in folders.
fn full_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &str) -> Found {
        Found {
            path: path.into(),
            folder: "/rec".into(),
            size: 1,
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_status_line_tells_about_each_new_file_once() {
        let mut folders = MediaFolders::new(Vec::new());
        let one = [found("/rec/a.mkv")];
        assert_eq!(
            folders.announce(&one).as_deref(),
            Some(
                "a.mkv is new in your media folders: it is under New in your folders (Library page, media drawer), ready to add"
            )
        );
        assert_eq!(folders.announce(&one), None);
        let more = [found("/rec/a.mkv"), found("/rec/b.mkv"), found("/rec/c.mkv")];
        assert_eq!(
            folders.announce(&more).as_deref(),
            Some(
                "2 new files in your media folders: they are under New in your folders (Library page, media drawer), ready to add"
            )
        );
    }

    #[test]
    fn launcher_folders_are_read_from_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(project_index::INDEX_FILE_NAME);
        assert!(launcher_media_folders(&file).is_empty());
        assert!(!file.exists(), "no index is made here");
        let index = project_index::Index::open(&file).unwrap();
        index
            .add_folder(&dir.path().join("Recordings"), project_index::FolderKind::Media)
            .unwrap();
        index
            .add_folder(&dir.path().join("Projects"), project_index::FolderKind::Projects)
            .unwrap();
        assert_eq!(launcher_media_folders(&file), [dir.path().join("Recordings")]);
    }

    #[test]
    fn new_files_are_kept_until_the_list_the_media_or_the_dismissed_change() {
        let project = crate::inspector::tests::project_with_a_clip();
        let in_project = full_path(Path::new("facecam.mkv")).display().to_string();
        let mut folders = MediaFolders::new(Vec::new());
        let list = Arc::new(vec![found("/rec/a.mkv"), found(&in_project), found("/rec/b.mkv")]);
        let first = folders.new_among(Arc::clone(&list), &project);
        assert_eq!(*first, [found("/rec/a.mkv"), found("/rec/b.mkv")]);
        let again = folders.new_among(Arc::clone(&list), &project);
        assert!(Arc::ptr_eq(&first, &again), "kept, not filtered again");
        // Dismissed: filtered again.
        folders.dismiss("/rec/a.mkv".into());
        let after = folders.new_among(Arc::clone(&list), &project);
        assert_eq!(*after, [found("/rec/b.mkv")]);
        // A new list from the watcher.
        let more = Arc::new(vec![found("/rec/c.mkv"), found("/rec/b.mkv")]);
        assert_eq!(
            *folders.new_among(more, &project),
            [found("/rec/c.mkv"), found("/rec/b.mkv")]
        );
        // The project's media changed: the file it took in is no longer new.
        let empty = Project::new("T", project::SequenceSettings::GAME_CLIP);
        let list = Arc::new(vec![found(&in_project)]);
        assert_eq!(folders.new_among(Arc::clone(&list), &empty).len(), 1);
        assert!(folders.new_among(list, &project).is_empty());
        assert!(folders.new_among(Arc::default(), &project).is_empty());
    }

    /// Prints how long the new files take per frame with 500 found in the
    /// folders, filtered afresh against kept (it is asked up to three times
    /// a frame).
    #[test]
    #[ignore = "a measurement: cargo test -p app -- --ignored --nocapture new_files_per_frame"]
    fn new_files_per_frame_filtered_afresh_and_kept() {
        let project = crate::inspector::tests::project_with_a_clip();
        let list: Arc<Vec<Found>> = Arc::new((0..500).map(|n| found(&format!("/rec/{n}.mkv"))).collect());
        let mut folders = MediaFolders::new(Vec::new());
        let frames = 100u32;
        let start = std::time::Instant::now();
        for _ in 0..frames {
            folders.shown = None;
            std::hint::black_box(folders.new_among(Arc::clone(&list), &project));
        }
        let afresh = start.elapsed() / frames;
        let start = std::time::Instant::now();
        for _ in 0..frames {
            std::hint::black_box(folders.new_among(Arc::clone(&list), &project));
        }
        let kept = start.elapsed() / frames;
        println!("new files per call, 500 found: afresh {afresh:?}, kept {kept:?}");
    }
}
