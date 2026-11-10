//! Reading and writing project files.
//!
//! A project file is RON text: a `format_version`, a small `summary` the
//! launcher can read without loading everything, then the project itself.
//! Files are written atomically (to a temporary file, then renamed over the
//! old one), so a crash mid-save never leaves a half-written project.
//!
//! When the format changes, [`FORMAT_VERSION`] goes up by one and a step is
//! added to [`MIGRATIONS`] that turns file text of the previous version into
//! the new one. A step reads the old text into a frozen copy of the old
//! types (kept in a `formats/vN.rs` module), converts it, and writes the new
//! text, so enum names and every other detail survive. Old files are migrated
//! step by step when opened; a sample of every released version lives in
//! `tests/samples/` and must keep opening.

use crate::model::{Project, SequenceSettings};
use crate::time::Time;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 1;

/// Turns the text of a version `n` file into the text of a version `n + 1` file.
pub type Migration = fn(&str) -> diag::Result<String>;

/// `MIGRATIONS[i]` upgrades version `i + 1` to `i + 2`. Empty while only version 1 exists.
pub const MIGRATIONS: &[Migration] = &[];

/// The part of a project file the launcher reads to show a project card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub name: String,
    pub settings: SequenceSettings,
    /// Length of the open timeline.
    pub length: Time,
}

impl Summary {
    pub fn of(project: &Project) -> Option<Summary> {
        let sequence = project.active()?;
        Some(Summary {
            name: project.name.clone(),
            settings: sequence.settings,
            length: sequence.length(),
        })
    }
}

#[derive(Serialize)]
struct FileOut<'a> {
    format_version: u32,
    summary: Option<Summary>,
    project: &'a Project,
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    #[allow(dead_code)]
    summary: Option<Summary>,
    project: Project,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
    #[serde(default)]
    summary: Option<Summary>,
}

/// Saves `project` to `path`, replacing any existing file only once the new one is complete.
pub fn save(project: &Project, path: &Path) -> diag::Result<()> {
    project.validate()?;
    let text = to_string(project)?;
    let temp = temp_path(path);
    let explain_write = || {
        AppError::new("The project can't be saved")
            .why(format!("Writing to {} failed.", path.display()))
            .fix("Check that the disk is not full and that you can write to this folder.")
    };
    let mut file = File::create(&temp).or_explain(explain_write)?;
    file.write_all(text.as_bytes()).or_explain(explain_write)?;
    file.sync_all().or_explain(explain_write)?;
    drop(file);
    fs::rename(&temp, path).or_explain(explain_write)?;
    Ok(())
}

/// The file text for `project` in the current format.
pub fn to_string(project: &Project) -> diag::Result<String> {
    let file = FileOut {
        format_version: FORMAT_VERSION,
        summary: Summary::of(project),
        project,
    };
    let config = ron::ser::PrettyConfig::new().indentor("  ".to_owned());
    ron::ser::to_string_pretty(&file, config).or_explain(|| {
        AppError::new("The project can't be saved").why("It could not be turned into file text.")
    })
}

/// Opens a project file, migrating it from an older format if needed.
pub fn load(path: &Path) -> diag::Result<Project> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be opened")
            .why(format!("{} could not be read.", path.display()))
            .fix("Check that the file still exists and that you can open it.")
    })?;
    from_str(&text)
}

/// Reads only the summary block, for project lists. Cheap even for big projects.
pub fn read_summary(path: &Path) -> diag::Result<Option<Summary>> {
    let text = fs::read_to_string(path).or_explain(|| {
        AppError::new("The project can't be read").why(format!("{} could not be read.", path.display()))
    })?;
    Ok(parse_header(&text)?.summary)
}

/// Parses project file text, migrating it from an older format if needed.
pub fn from_str(text: &str) -> diag::Result<Project> {
    from_str_with(text, FORMAT_VERSION, MIGRATIONS)
}

/// Like [`from_str`] with an explicit target version and migration list (used by tests).
pub fn from_str_with(text: &str, current: u32, migrations: &[Migration]) -> diag::Result<Project> {
    let version = parse_header(text)?.format_version;
    if version > current {
        return Err(AppError::new("This project was saved by a newer version")
            .why(format!(
                "It uses file format {version}; this version reads up to format {current}."
            ))
            .fix("Update the app, then open the project again."));
    }
    let file: FileIn = if version == current {
        ron::from_str(text).or_explain(damaged)?
    } else {
        let upgraded = migrate(text, version, current, migrations)?;
        ron::from_str(&upgraded).or_explain(damaged)?
    };
    file.project.validate()?;
    Ok(file.project)
}

/// Runs migrations from `from` up to `to`, one version at a time.
pub fn migrate(text: &str, from: u32, to: u32, migrations: &[Migration]) -> diag::Result<String> {
    let mut text = text.to_owned();
    for version in from..to {
        let step = version
            .checked_sub(1)
            .and_then(|i| migrations.get(i as usize))
            .ok_or_else(|| {
                AppError::new("This project can't be upgraded")
                    .why(format!(
                        "There is no upgrade from file format {version} to {}.",
                        version + 1
                    ))
                    .fix("This is a bug in the app; please report it with the project file.")
            })?;
        text = step(&text)?;
    }
    Ok(text)
}

fn parse_header(text: &str) -> diag::Result<Header> {
    ron::from_str(text).or_explain(damaged)
}

fn damaged() -> AppError {
    AppError::new("This file is not a project, or it is damaged")
        .why("Its contents could not be read as a project.")
        .fix("Open the latest autosave or the backup copy next to the file.")
}

/// `name.ext` → `.name.ext.saving` in the same folder, so the rename stays on one disk.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.saving"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> Project {
        let mut p = Project::new("Test", SequenceSettings::GAME_CLIP);
        p.name = "Round trip".into();
        p
    }

    #[test]
    fn save_then_load_gives_the_same_project() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let project = sample_project();
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap(), project);
        assert!(!temp_path(&path).exists(), "the temporary file is renamed away");
    }

    #[test]
    fn saving_replaces_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        let mut project = sample_project();
        save(&project, &path).unwrap();
        project.name = "Second save".into();
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap().name, "Second save");
    }

    #[test]
    fn the_summary_is_readable_on_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.project");
        save(&sample_project(), &path).unwrap();
        let summary = read_summary(&path).unwrap().unwrap();
        assert_eq!(summary.name, "Round trip");
        assert_eq!(summary.settings, SequenceSettings::GAME_CLIP);
    }

    #[test]
    fn newer_files_are_refused_with_an_explanation() {
        let text = to_string(&sample_project()).unwrap().replace(
            &format!("format_version: {FORMAT_VERSION}"),
            &format!("format_version: {}", FORMAT_VERSION + 1),
        );
        let err = from_str(&text).unwrap_err();
        assert!(err.what().contains("newer version"), "{}", err.report());
    }

    #[test]
    fn garbage_is_reported_as_damaged() {
        let err = from_str("this is not a project").unwrap_err();
        assert!(err.what().contains("damaged"));
    }

    #[test]
    fn migrations_run_in_order_from_the_file_version() {
        // Pretend the current format is 3 and this file is version 1. Each
        // step bumps the version and renames the project, so the result shows
        // both steps ran, in order.
        fn v1_to_v2(text: &str) -> diag::Result<String> {
            Ok(text
                .replace("format_version: 1", "format_version: 2")
                .replace("\"Round trip\"", "\"after step 1\""))
        }
        fn v2_to_v3(text: &str) -> diag::Result<String> {
            Ok(text
                .replace("format_version: 2", "format_version: 3")
                .replace("\"after step 1\"", "\"after step 2\""))
        }
        let text = to_string(&sample_project()).unwrap();
        let project = from_str_with(&text, 3, &[v1_to_v2, v2_to_v3]).unwrap();
        assert_eq!(project.name, "after step 2");

        let missing = from_str_with(&text, 3, &[v1_to_v2]).unwrap_err();
        assert!(missing.what().contains("can't be upgraded"));
    }
}
