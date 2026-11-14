//! Package project: the project and only the media its timelines use,
//! copied into one new folder that opens anywhere with nothing missing.
//!
//! ```text
//! <chosen folder>/<project name>/          ("<name> 2" when that is taken)
//!     <project file>                       a clean saved copy, no journal
//!     <project file>.thumbnail.png         the launcher's picture, if there is one
//!     <project file>.view                  the timeline layout, if there is one
//!     Media/<file name>                    every file a timeline uses
//! ```
//!
//! The packaged project's media paths are relative (`Media/<file name>`,
//! always written with `/`), and a relative media path means "from the
//! project file's folder" ([`file::resolve_media_paths`], used whenever a
//! project is opened), so the folder can be moved, copied to another
//! machine or renamed as a whole. Files no timeline uses are left out of
//! the copy and of its media list (see [`crate::prune`]). Two files with
//! the same name from different folders become "name.mkv" and
//! "name 2.mkv".
//!
//! Saved versions go in only when asked ([`Plan::versions`], `run`'s
//! `with_versions`): each is written into the package's own versions
//! folder with its media paths pointing inside the package, and the files
//! only versions use are copied into `Media/` too, so restoring one there
//! finds everything. A version that can't be read is left out.
//!
//! Left out: the autosave journal and anything set aside after a crash
//! (the copy is a clean save of the project as it is now), backups from
//! before upgrades, templates and labels offered in every project (they
//! belong to the person, not the project), and proxies (they belong to
//! recordings and are built again where the package is opened).
//!
//! [`plan`] reads file sizes (run it on a worker); [`run`] copies, a piece
//! at a time, saying how far it got and stopping when cancelled. Whatever
//! it made is taken away again when it is cancelled or fails; the
//! originals are only ever read.

use crate::file;
use crate::model::Project;
use crate::side_files::SideFile;
use diag::{AppError, Explain};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The folder inside a package that holds its media.
pub const MEDIA_FOLDER: &str = "Media";

/// How much is copied between two looks at "cancelled".
const PIECE: usize = 1024 * 1024;

/// One file to copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFile {
    pub from: PathBuf,
    pub to: PathBuf,
    pub size: u64,
}

/// What packaging will do, worked out before anything is written.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The new folder (not there yet).
    pub folder: PathBuf,
    /// The project file inside it.
    pub project_file: PathBuf,
    /// The packaged project: only used media, with paths inside the package.
    pub project: Project,
    /// Media first, then the side files kept.
    pub copies: Vec<CopyFile>,
    /// Media a timeline uses that isn't on disk: nothing can be packaged
    /// until they are found (relinked) or their clips removed.
    pub missing: Vec<PathBuf>,
    /// Files in the project that no timeline uses, left out.
    pub left_out: usize,
    /// The saved versions, copied only when asked.
    pub versions: Versions,
}

/// The saved versions, ready to go into the package.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Versions {
    /// Each version as it will be written (media paths inside the
    /// package), with where.
    pub files: Vec<(PathBuf, Project)>,
    /// The versions' size on disk (the copies are about the same).
    pub file_bytes: u64,
    /// Files only versions use, copied into `Media/` with them.
    pub media: Vec<CopyFile>,
    /// Files only versions use that can't be found: those versions miss
    /// them wherever they are restored.
    pub missing: usize,
    /// Versions that can't be read (damaged, or from a newer version), left out.
    pub unreadable: usize,
}

impl Versions {
    /// What including them adds, in bytes.
    pub fn bytes(&self) -> u64 {
        self.file_bytes + self.media.iter().map(|c| c.size).sum::<u64>()
    }
}

impl Plan {
    /// Everything that will be copied, in bytes (the project file itself,
    /// a few kilobytes, aside), with or without the saved versions.
    pub fn total_bytes(&self, with_versions: bool) -> u64 {
        let versions = if with_versions { self.versions.bytes() } else { 0 };
        self.copies.iter().map(|c| c.size).sum::<u64>() + versions
    }

    /// The number of media files copied.
    pub fn media_files(&self, with_versions: bool) -> usize {
        let media = self.folder.join(MEDIA_FOLDER);
        let own = self.copies.iter().filter(|c| c.to.starts_with(&media)).count();
        own + if with_versions {
            self.versions.media.len()
        } else {
            0
        }
    }
}

/// How packaging ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Packaged,
    /// Stopped when asked; nothing of it is left.
    Cancelled,
}

/// Works out the package for `project` (saved at `project_file`) in a new
/// folder inside `into`. Reads the sizes of the files: on a worker.
pub fn plan(project: &Project, project_file: &Path, into: &Path) -> diag::Result<Plan> {
    let folder = free_folder(into, &folder_name(&project.name));
    let file_name = project_file.file_name().map_or_else(
        || format!("{}.{}", folder_name(&project.name), brand::PROJECT_FILE_EXTENSION),
        |n| n.to_string_lossy().into_owned(),
    );
    let packaged_file = folder.join(&file_name);
    let used = crate::prune::used_media(project);
    let mut packaged = project.clone();
    let unused = crate::prune::unused_media(project);
    if !unused.is_empty() {
        crate::prune::remove_media(&mut packaged, &unused)?;
    }
    let mut copies = Vec::new();
    let mut missing = Vec::new();
    let mut names_taken: HashSet<String> = HashSet::new();
    // Where each original file is in the package, by its full path.
    let mut placed: HashMap<PathBuf, PathBuf> = HashMap::new();
    for id in &used {
        let Some(media) = packaged.media.get_mut(id) else {
            continue;
        };
        let size = match fs::metadata(&media.path) {
            Ok(meta) if meta.is_file() => meta.len(),
            _ => {
                missing.push(media.path.clone());
                continue;
            }
        };
        let name = free_name(&mut names_taken, &media_file_name(&media.path));
        copies.push(CopyFile {
            from: media.path.clone(),
            to: folder.join(MEDIA_FOLDER).join(&name),
            size,
        });
        let inside = PathBuf::from(format!("{MEDIA_FOLDER}/{name}"));
        placed.insert(media.path.clone(), inside.clone());
        media.path = inside;
    }
    for side in [SideFile::Thumbnail, SideFile::View] {
        let from = side.path(project_file);
        if let Ok(meta) = fs::metadata(&from)
            && meta.is_file()
        {
            copies.push(CopyFile {
                from,
                to: side.path(&packaged_file),
                size: meta.len(),
            });
        }
    }
    let versions = plan_versions(
        project_file,
        &packaged_file,
        &folder,
        &mut names_taken,
        &mut placed,
    );
    Ok(Plan {
        folder,
        project_file: packaged_file,
        project: packaged,
        copies,
        missing,
        left_out: unused.len(),
        versions,
    })
}

/// The saved versions of the project at `project_file`, each with its
/// media pointing inside the package; files only they use are added to
/// `Media/` under free names.
fn plan_versions(
    project_file: &Path,
    packaged_file: &Path,
    folder: &Path,
    names_taken: &mut HashSet<String>,
    placed: &mut HashMap<PathBuf, PathBuf>,
) -> Versions {
    let mut versions = Versions::default();
    let into = crate::versions::folder(packaged_file);
    for version in crate::versions::list(project_file) {
        let mut project = match crate::versions::load(&version) {
            Ok(project) => project,
            Err(err) => {
                tracing::info!(version = %version.path.display(), "left out of the package: {}", err.report());
                versions.unreadable += 1;
                continue;
            }
        };
        versions.file_bytes += fs::metadata(&version.path).map_or(0, |m| m.len());
        let ids: Vec<_> = project.media.keys().copied().collect();
        for id in ids {
            let Some(media) = project.media.get_mut(&id) else {
                continue;
            };
            if let Some(inside) = placed.get(&media.path) {
                media.path = inside.clone();
                continue;
            }
            let size = match fs::metadata(&media.path) {
                Ok(meta) if meta.is_file() => meta.len(),
                _ => {
                    versions.missing += 1;
                    continue;
                }
            };
            let name = free_name(names_taken, &media_file_name(&media.path));
            let inside = PathBuf::from(format!("{MEDIA_FOLDER}/{name}"));
            versions.media.push(CopyFile {
                from: media.path.clone(),
                to: folder.join(MEDIA_FOLDER).join(&name),
                size,
            });
            placed.insert(media.path.clone(), inside.clone());
            media.path = inside;
        }
        let file_name = version.path.file_name().map_or_else(
            || {
                format!(
                    "{} {}.{}",
                    version.saved,
                    version.name,
                    brand::PROJECT_FILE_EXTENSION
                )
            },
            |n| n.to_string_lossy().into_owned(),
        );
        versions.files.push((into.join(file_name), project));
    }
    versions
}

/// Makes the package, with the saved versions when `with_versions`.
/// `done` is set to the bytes copied so far; `cancelled` is asked between
/// pieces. Refused when files are missing or the folder appeared
/// meanwhile. On a cancel or a failure, what was made is taken away again.
pub fn run(
    plan: &Plan,
    with_versions: bool,
    done: &AtomicU64,
    cancelled: impl Fn() -> bool,
) -> diag::Result<Outcome> {
    if let Some(first) = plan.missing.first() {
        return Err(AppError::new("The project can't be packaged yet")
            .why(format!(
                "{} file(s) its timelines use can't be found, such as {}.",
                plan.missing.len(),
                first.display()
            ))
            .fix("Find the missing files first (the banner's \"Find them…\"), then package again."));
    }
    if plan.folder.exists() {
        return Err(AppError::new("The package folder is already there")
            .why(format!(
                "{} appeared after the package was planned.",
                plan.folder.display()
            ))
            .fix("Package again: a free name is chosen."));
    }
    let mut made = Made::default();
    match copy_all(plan, with_versions, done, &cancelled, &mut made) {
        Ok(Outcome::Packaged) => Ok(Outcome::Packaged),
        Ok(Outcome::Cancelled) => {
            made.take_away();
            Ok(Outcome::Cancelled)
        }
        Err(err) => {
            made.take_away();
            Err(err)
        }
    }
}

fn copy_all(
    plan: &Plan,
    with_versions: bool,
    done: &AtomicU64,
    cancelled: &impl Fn() -> bool,
    made: &mut Made,
) -> diag::Result<Outcome> {
    let cant_write = |path: &Path| {
        let path = path.display().to_string();
        move || {
            AppError::new("The package can't be written")
                .why(format!("Writing {path} failed."))
                .fix("Check that the disk has room and that you can write to that folder.")
        }
    };
    fs::create_dir_all(&plan.folder).or_explain(cant_write(&plan.folder))?;
    made.folders.push(plan.folder.clone());
    let media = plan.folder.join(MEDIA_FOLDER);
    fs::create_dir(&media).or_explain(cant_write(&media))?;
    made.folders.push(media);
    let mut buffer = vec![0u8; PIECE];
    let version_media = if with_versions {
        plan.versions.media.as_slice()
    } else {
        &[]
    };
    for copy in plan.copies.iter().chain(version_media) {
        if cancelled() {
            return Ok(Outcome::Cancelled);
        }
        let mut from = File::open(&copy.from).or_explain(|| {
            AppError::new("A file can't be read for the package")
                .why(format!("Reading {} failed.", copy.from.display()))
                .fix("Check that the file is still there and the drive is connected.")
        })?;
        made.files.push(copy.to.clone());
        let mut to = File::create(&copy.to).or_explain(cant_write(&copy.to))?;
        loop {
            if cancelled() {
                return Ok(Outcome::Cancelled);
            }
            let read = from.read(&mut buffer).or_explain(|| {
                AppError::new("A file can't be read for the package")
                    .why(format!("Reading {} failed partway.", copy.from.display()))
                    .fix("Check that the drive is still connected.")
            })?;
            if read == 0 {
                break;
            }
            to.write_all(&buffer[..read]).or_explain(cant_write(&copy.to))?;
            done.fetch_add(read as u64, Ordering::Relaxed);
        }
        to.sync_all().or_explain(cant_write(&copy.to))?;
    }
    made.files.push(plan.project_file.clone());
    file::save(&plan.project, &plan.project_file)?;
    if with_versions && !plan.versions.files.is_empty() {
        let into = crate::versions::folder(&plan.project_file);
        fs::create_dir(&into).or_explain(cant_write(&into))?;
        made.folders.push(into);
        for (to, version) in &plan.versions.files {
            if cancelled() {
                return Ok(Outcome::Cancelled);
            }
            made.files.push(to.clone());
            file::save(version, to)?;
            done.fetch_add(fs::metadata(to).map_or(0, |m| m.len()), Ordering::Relaxed);
        }
    }
    Ok(Outcome::Packaged)
}

/// What a run made, to take away again.
#[derive(Debug, Default)]
struct Made {
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
}

impl Made {
    /// Removes the files it wrote, then its folders if they are empty.
    /// Never anything else: a folder with something else in it stays.
    fn take_away(&self) {
        for path in &self.files {
            if let Err(err) = fs::remove_file(path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(file = %path.display(), "a packaged file could not be removed: {err}");
            }
        }
        for folder in self.folders.iter().rev() {
            if let Err(err) = fs::remove_dir(folder) {
                tracing::info!(folder = %folder.display(), "left in place: {err}");
            }
        }
    }
}

/// The package folder's name for a project called `name`: characters file
/// systems refuse become dashes, an empty name "Untitled".
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `into/name`, or "name 2", "name 3"… when that is taken.
fn free_folder(into: &Path, name: &str) -> PathBuf {
    let first = into.join(name);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| into.join(format!("{name} {n}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn media_file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "media".to_owned(), |n| n.to_string_lossy().into_owned())
}

/// `name`, or "stem 2.ext"… when a file of the package already has it
/// (whatever its capitals, as some file systems don't tell them apart).
fn free_name(taken: &mut HashSet<String>, name: &str) -> String {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy()));
    let mut candidate = name.to_owned();
    let mut n = 2;
    while taken.contains(&candidate.to_lowercase()) {
        candidate = format!("{stem} {n}{}", ext.as_deref().unwrap_or(""));
        n += 1;
    }
    taken.insert(candidate.to_lowercase());
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::ids::{ClipId, MediaId};
    use crate::model::{Clip, Fingerprint, MediaKind, MediaRef, SequenceSettings};
    use crate::time::Time;
    use crate::{Document, Recovery};

    /// A project saved in `dir/Work`, using `a.mkv` (from `dir/Rec`) and
    /// `dir/Other/a.mkv` (same name, other folder) on its timeline, and
    /// with `spare.wav` unused.
    fn saved_project(dir: &Path) -> PathBuf {
        let rec = dir.join("Rec");
        let other = dir.join("Other");
        let work = dir.join("Work");
        for folder in [&rec, &other, &work] {
            fs::create_dir_all(folder).unwrap();
        }
        fs::write(rec.join("a.mkv"), vec![1u8; 10_000]).unwrap();
        fs::write(other.join("A.mkv"), vec![2u8; 5_000]).unwrap();
        fs::write(rec.join("spare.wav"), vec![3u8; 700]).unwrap();
        let mut p = Project::new("Trip: day 1", SequenceSettings::GAME_CLIP);
        for path in [rec.join("a.mkv"), other.join("A.mkv"), rec.join("spare.wav")] {
            p = Command::AddMedia {
                media: MediaRef {
                    id: MediaId(0),
                    path,
                    fingerprint: Fingerprint {
                        size: 1,
                        sample_hash: "x".into(),
                    },
                    kind: MediaKind::Video,
                    duration: Time::seconds(30),
                    info: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        // The second file's name differs from the first only in capitals.
        let ids: Vec<MediaId> = p.media.keys().copied().collect();
        let seq = p.active_sequence;
        let track = p.active().unwrap().video_tracks[0].id;
        for (i, media) in ids[..2].iter().enumerate() {
            p = Command::PlaceClip {
                sequence: seq,
                track,
                clip: Clip {
                    lines: Default::default(),
                    effects: Default::default(),
                    mode: Default::default(),
                    id: ClipId(0),
                    media: *media,
                    stream: 0,
                    start: Time::seconds(10 * i as i64),
                    source_in: Time::ZERO,
                    duration: Time::seconds(5),
                    link: None,
                    speed: Default::default(),
                    look: Default::default(),
                    sound: Default::default(),
                    keyframes: Default::default(),
                    ramp: Default::default(),
                    blend: Default::default(),
                    kind: Default::default(),
                },
            }
            .apply(&p)
            .unwrap();
        }
        let file = work.join(format!("Trip.{}", brand::PROJECT_FILE_EXTENSION));
        Document::create(&file, p).unwrap();
        fs::write(SideFile::Thumbnail.path(&file), b"png").unwrap();
        fs::write(SideFile::View.path(&file), b"view").unwrap();
        file
    }

    #[test]
    fn a_package_holds_only_used_media_and_opens_elsewhere_with_nothing_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = saved_project(dir.path());
        let doc = Document::open(&file, Recovery::Recover).unwrap();
        let out = dir.path().join("Out");
        fs::create_dir_all(&out).unwrap();

        let plan = plan(doc.project(), &file, &out).unwrap();
        assert_eq!(plan.folder, out.join("Trip- day 1"));
        assert!(plan.missing.is_empty());
        assert_eq!(plan.left_out, 1, "spare.wav is used nowhere");
        assert_eq!(plan.media_files(false), 2);
        assert_eq!(plan.total_bytes(false), 10_000 + 5_000 + 3 + 4);
        let names: Vec<String> = plan
            .project
            .media
            .values()
            .map(|m| m.path.display().to_string())
            .collect();
        assert_eq!(names, ["Media/a.mkv", "Media/A 2.mkv"]);

        let done = AtomicU64::new(0);
        assert_eq!(run(&plan, false, &done, || false).unwrap(), Outcome::Packaged);
        assert_eq!(done.load(Ordering::Relaxed), plan.total_bytes(false));
        assert!(
            !crate::document::journal_path(&plan.project_file).exists(),
            "no journal"
        );

        // Move the package somewhere else and take the originals away.
        let moved = dir.path().join("Elsewhere").join("Moved package");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&plan.folder, &moved).unwrap();
        fs::remove_dir_all(dir.path().join("Rec")).unwrap();
        fs::remove_dir_all(dir.path().join("Other")).unwrap();

        let opened_file = moved.join(plan.project_file.file_name().unwrap());
        let info = file::read_info(&opened_file).unwrap();
        let summary = info.summary.unwrap();
        assert!(summary.media.iter().all(|m| m.exists()), "{:?}", summary.media);
        let opened = Document::open(&opened_file, Recovery::Recover).unwrap();
        assert_eq!(opened.project().media.len(), 2);
        for media in opened.project().media.values() {
            assert!(media.path.exists(), "{} is missing", media.path.display());
            assert!(media.path.starts_with(&moved));
        }
        assert_eq!(fs::read(moved.join("Media/A 2.mkv")).unwrap(), vec![2u8; 5_000]);
        assert!(SideFile::Thumbnail.path(&opened_file).exists());
        assert!(SideFile::View.path(&opened_file).exists());
        assert!(opened.project().problems().is_empty());
    }

    #[test]
    fn cancelling_or_failing_leaves_nothing_and_never_touches_the_originals() {
        let dir = tempfile::tempdir().unwrap();
        let file = saved_project(dir.path());
        let doc = Document::open(&file, Recovery::Recover).unwrap();
        let out = dir.path().join("Out");
        fs::create_dir_all(&out).unwrap();
        let plan = plan(doc.project(), &file, &out).unwrap();
        let done = AtomicU64::new(0);
        let asked = std::cell::Cell::new(0);
        // Cancelled after the first piece of the first file.
        let outcome = run(&plan, false, &done, || {
            asked.set(asked.get() + 1);
            asked.get() > 2
        })
        .unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
        assert!(!plan.folder.exists(), "nothing is left of the package");
        assert!(dir.path().join("Rec/a.mkv").exists());

        // A file that went away between planning and copying.
        fs::remove_file(dir.path().join("Rec/a.mkv")).unwrap();
        assert!(run(&plan, false, &done, || false).is_err());
        assert!(!plan.folder.exists());
        assert!(dir.path().join("Rec/spare.wav").exists());
    }

    #[test]
    fn missing_media_is_named_before_anything_is_written_and_taken_names_are_avoided() {
        let dir = tempfile::tempdir().unwrap();
        let file = saved_project(dir.path());
        let doc = Document::open(&file, Recovery::Recover).unwrap();
        let out = dir.path().join("Out");
        fs::create_dir_all(out.join("Trip- day 1")).unwrap();
        fs::remove_file(dir.path().join("Rec/a.mkv")).unwrap();
        let plan = plan(doc.project(), &file, &out).unwrap();
        assert_eq!(plan.folder, out.join("Trip- day 1 2"));
        assert_eq!(plan.missing, [dir.path().join("Rec/a.mkv")]);
        let err = run(&plan, false, &AtomicU64::new(0), || false).unwrap_err();
        assert!(err.what().contains("can't be packaged yet"));
        assert!(!plan.folder.exists());
    }

    #[test]
    fn saved_versions_go_in_only_when_asked_and_find_their_files_there() {
        let dir = tempfile::tempdir().unwrap();
        let file = saved_project(dir.path());
        let doc = Document::open(&file, Recovery::Recover).unwrap();
        // A version from before spare.wav was left unused: it still plays it.
        let mut older = doc.project().clone();
        let spare = *older.media.keys().nth(2).unwrap();
        let track = older.active().unwrap().video_tracks[0].id;
        let seq = older.active_sequence;
        let mut clip = older.active().unwrap().video_tracks[0].clips[0].clone();
        clip.media = spare;
        clip.start = Time::seconds(40);
        older = Command::PlaceClip {
            sequence: seq,
            track,
            clip,
        }
        .apply(&older)
        .unwrap();
        crate::versions::save(&older, &file, "With the spare take").unwrap();
        let out = dir.path().join("Out");
        fs::create_dir_all(&out).unwrap();

        let plan = plan(doc.project(), &file, &out).unwrap();
        assert_eq!(plan.versions.files.len(), 1);
        assert_eq!(
            plan.versions.media.len(),
            1,
            "spare.wav, which only the version uses"
        );
        assert_eq!(plan.media_files(true), 3);
        assert!(plan.total_bytes(true) > plan.total_bytes(false) + 700);

        // Without: no versions folder, no spare.wav.
        run(&plan, false, &AtomicU64::new(0), || false).unwrap();
        assert!(!crate::versions::folder(&plan.project_file).exists());
        assert!(!plan.folder.join("Media/spare.wav").exists());
        fs::remove_dir_all(&plan.folder).unwrap();

        // With: moved elsewhere, the version restores with every file found.
        run(&plan, true, &AtomicU64::new(0), || false).unwrap();
        let moved = dir.path().join("Moved");
        fs::rename(&plan.folder, &moved).unwrap();
        fs::remove_dir_all(dir.path().join("Rec")).unwrap();
        let opened_file = moved.join(plan.project_file.file_name().unwrap());
        let listed = crate::versions::list(&opened_file);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "With the spare take");
        let restored = crate::versions::load(&listed[0]).unwrap();
        assert_eq!(restored.media.len(), 3);
        for media in restored.media.values() {
            assert!(media.path.exists(), "{} is missing", media.path.display());
        }
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Trip: day 1"), "Trip- day 1");
        assert_eq!(folder_name("  ... "), "Untitled");
        let mut taken = HashSet::new();
        assert_eq!(free_name(&mut taken, "a.mkv"), "a.mkv");
        assert_eq!(free_name(&mut taken, "A.mkv"), "A 2.mkv");
        assert_eq!(free_name(&mut taken, "a.mkv"), "a 3.mkv");
        assert_eq!(free_name(&mut taken, "notes"), "notes");
        assert_eq!(free_name(&mut taken, "notes"), "notes 2");
    }
}
