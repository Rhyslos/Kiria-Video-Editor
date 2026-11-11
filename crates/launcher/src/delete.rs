//! Deleting a project: the project file and the files that belong to it go
//! to the recycle bin, so a mistake can be undone from there.
//!
//! What belongs to a project is decided in one place, `project::side_files`.
//! Recordings a project uses are never on that list, and anything a project
//! uses as media is left alone even if it carries a side file's name.
//!
//! Both steps touch the disk, so the browser runs them on a worker:
//! [`plan`] finds what would go (the confirmation shows it), [`run`] moves
//! it to the [`Bin`].

use diag::{AppError, Explain};
use std::path::{Path, PathBuf};

/// Where deleted files go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bin {
    /// The system's recycle bin.
    System,
    /// A plain folder standing in for it. Sample projects (`--demo`) and
    /// tests use one, so nothing of theirs ever lands in a person's own
    /// recycle bin.
    Folder(PathBuf),
}

impl Bin {
    /// What the system calls it, for interface text.
    pub const NAME: &'static str = if cfg!(windows) { "recycle bin" } else { "trash" };

    /// Moves one file or folder into the bin.
    fn take(&self, path: &Path) -> diag::Result<()> {
        match self {
            Bin::System => trash::delete(path).or_explain(|| not_taken(path)),
            Bin::Folder(folder) => {
                std::fs::create_dir_all(folder).or_explain(|| not_taken(path))?;
                let name = path.file_name().unwrap_or(path.as_os_str());
                // Like a real bin, it takes two files of one name.
                let free = (1..)
                    .map(|n| match n {
                        1 => folder.join(name),
                        n => folder.join(format!("{} ({n})", name.to_string_lossy())),
                    })
                    .find(|p| p.symlink_metadata().is_err())
                    .unwrap_or_else(|| folder.join(name));
                std::fs::rename(path, free).or_explain(|| not_taken(path))
            }
        }
    }
}

fn not_taken(path: &Path) -> AppError {
    AppError::new(format!("The project can't be moved to the {}", Bin::NAME))
        .why(format!("{} could not be moved there.", path.display()))
        .fix(format!(
            "Close anything that has the file open, then try again. Some network and removable drives have no {}: delete the files in the file manager instead.",
            Bin::NAME
        ))
}

/// One file or folder that deleting the project would move to the bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What it is, in a few words.
    pub what: &'static str,
    pub path: PathBuf,
    /// More to say about it ("3 versions").
    pub detail: Option<String>,
}

/// What deleting a project would move to the bin. Only [`plan`] makes one,
/// so [`run`] can only ever be handed files that belong to a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    project: PathBuf,
    items: Vec<Item>,
}

impl Plan {
    /// The project file this is for (it may be gone already).
    pub fn project(&self) -> &Path {
        &self.project
    }

    /// The files that exist now, the project file first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

/// Finds what deleting the project at `project` would move to the bin: the
/// project file and its side files, as far as they exist. Reads the names in
/// the project's folder and the top of the project file.
pub fn plan(project: &Path) -> diag::Result<Plan> {
    let is_project_file = project
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(project::FILE_EXTENSION));
    if !is_project_file {
        // A card can stand for any file that was once opened as a project.
        return Err(AppError::new("This file is not a project")
            .why(format!(
                "{} does not end in .{}, so it is left where it is.",
                project.display(),
                project::FILE_EXTENSION
            ))
            .fix("Right-click the card and choose Remove from list to take it off the list."));
    }
    // What the project uses as media stays, whatever it is called.
    let media = match project::file::read_summary(project) {
        Ok(Some(summary)) => summary.media,
        _ => Vec::new(),
    };
    let is_media = |path: &Path| media.iter().any(|m| m.starts_with(path));

    let mut items = Vec::new();
    if project.symlink_metadata().is_ok() {
        items.push(Item {
            what: "Project file",
            path: project.to_path_buf(),
            detail: None,
        });
    }
    for found in project::side_files::existing(project) {
        if is_media(&found.path) {
            continue;
        }
        let detail = (found.kind == project::SideFile::Versions).then(|| {
            match project::versions::list(project).len() {
                1 => "1 version".to_owned(),
                n => format!("{n} versions"),
            }
        });
        items.push(Item {
            what: found.kind.label(),
            path: found.path,
            detail,
        });
    }
    Ok(Plan {
        project: project.to_path_buf(),
        items,
    })
}

/// Moves everything in `plan` to `bin` and returns how many files and
/// folders went. A file that has gone missing since the plan was made is
/// fine. On a failure nothing more is moved, and the error says what is in
/// the bin already.
pub fn run(plan: &Plan, bin: &Bin) -> diag::Result<usize> {
    let mut moved = 0;
    // The project file goes last: if something fails on the way, the
    // project is still there and listed, and deleting can be tried again.
    for item in plan.items.iter().rev() {
        if item.path.symlink_metadata().is_err() {
            continue;
        }
        bin.take(&item.path).map_err(|err| partly(err, moved))?;
        moved += 1;
    }
    Ok(moved)
}

/// Adds what already went to the explanation of a failure.
fn partly(err: AppError, moved: usize) -> AppError {
    if moved == 0 {
        return err;
    }
    let files = if moved == 1 {
        "1 of the project's files is".to_owned()
    } else {
        format!("{moved} of the project's files are")
    };
    let why = format!(
        "{} {files} already in the {} and can be restored from there.",
        err.why_text().unwrap_or_default(),
        Bin::NAME
    );
    err.why(why.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Project, SideFile};
    use std::fs;

    fn ext() -> &'static str {
        project::FILE_EXTENSION
    }

    /// A project with every kind of side file, and a recording beside it.
    fn full_project(folder: &Path, name: &str) -> PathBuf {
        let path = crate::files::create_project(folder, name, None).unwrap();
        for kind in [
            SideFile::Journal,
            SideFile::DiscardedJournal,
            SideFile::UnfinishedSave,
            SideFile::Thumbnail,
            SideFile::View,
        ] {
            fs::write(kind.path(&path), kind.label()).unwrap();
        }
        project::file::back_up_before_upgrade(&path, 3).unwrap();
        let project = project::file::load(&path).unwrap();
        project::versions::save(&project, &path, "First cut").unwrap();
        project::versions::save(&project, &path, "Second cut").unwrap();
        path
    }

    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn whats(plan: &Plan) -> Vec<&'static str> {
        plan.items().iter().map(|i| i.what).collect()
    }

    #[test]
    fn the_plan_lists_the_project_file_and_everything_that_belongs_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = full_project(tmp.path(), "Clip");
        fs::write(tmp.path().join("recording.mkv"), "a recording").unwrap();
        let plan = plan(&path).unwrap();
        assert_eq!(plan.project(), path);
        assert_eq!(
            whats(&plan),
            [
                "Project file",
                "Autosave file",
                "Unsaved work set aside after a crash",
                "Unfinished save",
                "Backup from before an upgrade",
                "Picture for the project list",
                "Timeline layout",
                "Saved versions",
            ]
        );
        assert_eq!(plan.items()[0].path, path);
        assert_eq!(plan.items()[7].detail.as_deref(), Some("2 versions"));
        assert!(plan.items().iter().all(|i| i.path.exists()));
    }

    #[test]
    fn everything_goes_to_the_bin_and_recordings_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let (folder, bin) = (tmp.path().join("projects"), tmp.path().join("bin"));
        let path = full_project(&folder, "Clip");
        let neighbour = full_project(&folder, "Clip 2");
        fs::write(folder.join("recording.mkv"), "a recording").unwrap();
        fs::write(folder.join(format!("Clip.{}.mkv", ext())), "also a recording").unwrap();
        let neighbours_files = names_in(&folder)
            .into_iter()
            .filter(|n| n.contains("Clip 2"))
            .count();

        let plan = plan(&path).unwrap();
        let moved = run(&plan, &Bin::Folder(bin.clone())).unwrap();
        assert_eq!(moved, 8);
        let left = names_in(&folder);
        assert_eq!(
            left.iter().filter(|n| n.contains("Clip 2")).count(),
            neighbours_files,
            "a project with a similar name keeps all its files"
        );
        let others: Vec<String> = left.into_iter().filter(|n| !n.contains("Clip 2")).collect();
        assert_eq!(
            others,
            [format!("Clip.{}.mkv", ext()), "recording.mkv".to_owned()],
            "only the recordings are left"
        );
        assert!(neighbour.exists());
        assert_eq!(
            fs::read_to_string(folder.join("recording.mkv")).unwrap(),
            "a recording"
        );

        // Everything can be taken back out, the saved versions whole.
        let binned = names_in(&bin);
        assert_eq!(binned.len(), 8);
        assert!(binned.contains(&format!("Clip.{}", ext())));
        assert_eq!(names_in(&bin.join(format!("Clip.{}.versions", ext()))).len(), 2);
        assert_eq!(
            project::file::load(&bin.join(format!("Clip.{}", ext())))
                .unwrap()
                .name,
            "Clip"
        );
    }

    #[test]
    fn files_that_are_already_gone_are_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let (folder, bin) = (tmp.path().join("projects"), Bin::Folder(tmp.path().join("bin")));
        let path = full_project(&folder, "Clip");
        let plan_before = plan(&path).unwrap();
        // Between the question and the answer, some files went away.
        fs::remove_file(&path).unwrap();
        fs::remove_file(project::view_path(&path)).unwrap();
        assert_eq!(run(&plan_before, &bin).unwrap(), 6);
        assert!(names_in(&folder).is_empty());

        // A project whose file is missing can still be cleared away...
        let gone = full_project(&folder, "Gone");
        fs::remove_file(&gone).unwrap();
        let plan_gone = plan(&gone).unwrap();
        assert!(!whats(&plan_gone).contains(&"Project file"));
        assert_eq!(run(&plan_gone, &bin).unwrap(), 7);
        assert!(names_in(&folder).is_empty());

        // ...and one with nothing left on disk at all.
        let nothing = plan(&gone).unwrap();
        assert!(nothing.items().is_empty());
        assert_eq!(run(&nothing, &bin).unwrap(), 0);
    }

    #[test]
    fn a_bin_that_fails_is_explained_and_the_project_file_is_still_there() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("projects");
        let path = full_project(&folder, "Clip");
        let plan = plan(&path).unwrap();
        // A bin that can't be made: a file is where its folder should be.
        let blocked = tmp.path().join("bin");
        fs::write(&blocked, "in the way").unwrap();
        let err = run(&plan, &Bin::Folder(blocked)).unwrap_err();
        assert!(err.what().contains("can't be moved to the"), "{}", err.report());
        assert!(err.why_text().is_some_and(|w| w.contains(".versions")));
        assert!(!err.fix_text().is_empty());
        assert!(path.exists(), "nothing was lost");
        assert_eq!(names_in(&folder).len(), 8);
    }

    #[test]
    fn a_failure_part_way_says_what_is_already_in_the_bin() {
        let err = partly(not_taken(Path::new("p.view")), 2);
        let why = err.why_text().unwrap();
        assert!(why.starts_with("p.view could not be moved there."), "{why}");
        assert!(
            why.contains("2 of the project's files are already in the"),
            "{why}"
        );
        assert_eq!(
            partly(not_taken(Path::new("p.view")), 0).why_text(),
            not_taken(Path::new("p.view")).why_text()
        );
    }

    #[test]
    fn a_file_the_project_uses_as_media_is_never_deleted() {
        use project::{Command, Fingerprint, MediaId, MediaInfo, MediaKind, MediaRef, Time};
        let tmp = tempfile::tempdir().unwrap();
        let (folder, bin) = (tmp.path().join("projects"), tmp.path().join("bin"));
        let path = full_project(&folder, "Clip");
        // The project's own picture was imported into it as media.
        let picture = project::thumbnail_path(&path);
        let with_media = Command::AddMedia {
            media: MediaRef {
                id: MediaId(0),
                path: picture.clone(),
                fingerprint: Fingerprint {
                    size: 1,
                    sample_hash: "x".into(),
                },
                kind: MediaKind::Video,
                duration: Time::seconds(5),
                info: MediaInfo::default(),
            },
        }
        .apply(&project::file::load(&path).unwrap())
        .unwrap();
        project::file::save(&with_media, &path).unwrap();
        assert!(
            project::file::read_summary(&path)
                .unwrap()
                .unwrap()
                .media
                .contains(&picture)
        );

        let plan = plan(&path).unwrap();
        assert!(plan.items().iter().all(|i| i.path != picture));
        run(&plan, &Bin::Folder(bin)).unwrap();
        assert_eq!(
            names_in(&folder),
            [picture.file_name().unwrap().to_string_lossy().into_owned()],
            "the file it uses is still there"
        );
    }

    #[test]
    fn only_project_files_can_be_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let recording = tmp.path().join("holiday.mkv");
        fs::write(&recording, "a recording").unwrap();
        // Opened as a project once, so it has a (damaged) card.
        let err = plan(&recording).unwrap_err();
        assert!(err.what().contains("not a project"), "{}", err.report());
        assert!(err.fix_text().contains("Remove from list"));

        let upper = tmp.path().join(format!("LOUD.{}", ext().to_uppercase()));
        project::file::save(&Project::new_automatic("Loud"), &upper).unwrap();
        assert_eq!(whats(&plan(&upper).unwrap()), ["Project file"]);
    }

    #[test]
    fn the_stand_in_bin_takes_two_files_of_one_name() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = Bin::Folder(tmp.path().join("bin"));
        for folder in ["a", "b"] {
            let file = tmp.path().join(folder).join("same.txt");
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, folder).unwrap();
            bin.take(&file).unwrap();
        }
        assert_eq!(names_in(&tmp.path().join("bin")), ["same.txt", "same.txt (2)"]);
    }
}
