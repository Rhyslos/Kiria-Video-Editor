//! Labels for every project: label names and colours the person wants
//! offered in every project's Labels menu ("everywhere").
//!
//! They belong to the person, not to a project, like templates: one small
//! file, `labels.ron` in the app's data folder, with its own format version.
//! Only names and colours are kept here. Which files carry a label is
//! always the project's: putting a label from this set on a file copies it
//! into the project's own labels first (an ordinary, undoable
//! [`crate::Command::AddMediaLabel`]), so a project file stays complete on
//! its own and opens the same on another machine. Taking a label out of
//! this set leaves every project that used it with its own copy.
//!
//! ```ron
//! (
//!   format_version: 1,
//!   labels: [
//!     (name: "Drone", color: Green),
//!   ],
//! )
//! ```

use crate::library::NAME_LIMIT;
use crate::model::LabelColor;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format this build writes.
pub const FORMAT_VERSION: u32 = 1;

/// The file's name in the app's data folder.
pub const FILE_NAME: &str = "labels.ron";

/// One label offered in every project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalLabel {
    pub name: String,
    pub color: LabelColor,
}

/// The labels offered in every project, in the order they were added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalLabels {
    pub labels: Vec<GlobalLabel>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    format_version: u32,
    labels: &'a [GlobalLabel],
}

#[derive(Deserialize)]
struct FileIn {
    #[allow(dead_code)]
    format_version: u32,
    labels: Vec<GlobalLabel>,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
}

/// Where the file is.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

impl GlobalLabels {
    /// Reads the set from the app's data folder: empty when there is no
    /// file yet; refused when it was written by a newer version or is
    /// damaged (the file is then left as it is).
    pub fn load(data_dir: &Path) -> diag::Result<GlobalLabels> {
        let file = path(data_dir);
        if !file.exists() {
            return Ok(GlobalLabels::default());
        }
        let text = std::fs::read_to_string(&file).or_explain(|| {
            AppError::new("Labels for every project can't be read")
                .why(format!("{} could not be read.", file.display()))
                .fix("Check that the app's data folder can be read. Each project's own labels still work.")
        })?;
        GlobalLabels::parse(&text)
    }

    /// Parses the file's text.
    pub fn parse(text: &str) -> diag::Result<GlobalLabels> {
        let version = ron::from_str::<Header>(text).or_explain(damaged)?.format_version;
        if version > FORMAT_VERSION {
            return Err(AppError::new("Labels for every project were saved by a newer version")
                .why(format!(
                    "They use format {version}; this version reads up to format {FORMAT_VERSION}."
                ))
                .fix("Update the app. Until then each project's own labels work, and the file is left as it is."));
        }
        let file: FileIn = ron::from_str(text).or_explain(damaged)?;
        Ok(GlobalLabels { labels: file.labels })
    }

    /// The file text in the current format.
    pub fn to_string(&self) -> diag::Result<String> {
        let config = ron::ser::PrettyConfig::new().indentor("  ".to_owned());
        ron::ser::to_string_pretty(
            &FileOut {
                format_version: FORMAT_VERSION,
                labels: &self.labels,
            },
            config,
        )
        .or_explain(|| {
            AppError::new("Labels for every project can't be saved")
                .why("They could not be turned into file text.")
        })
    }

    /// Writes the set to the app's data folder (a temporary file, then
    /// renamed over the old one). Touches the disk: call it from a worker.
    pub fn save(&self, data_dir: &Path) -> diag::Result<()> {
        let explain = || {
            AppError::new("Labels for every project can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full and that the app's data folder can be written to.")
        };
        std::fs::create_dir_all(data_dir).or_explain(explain)?;
        crate::file::write_atomic(&path(data_dir), &self.to_string()?).or_explain(explain)
    }

    /// The label with this name, whatever its capitals.
    pub fn find(&self, name: &str) -> Option<&GlobalLabel> {
        let name = name.trim().to_lowercase();
        self.labels.iter().find(|l| l.name.to_lowercase() == name)
    }

    /// Offers a label in every project. A name already in the set (in any
    /// capitals) is left as it is.
    pub fn add(&mut self, name: &str, color: LabelColor) -> diag::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new("A label needs a name").fix("Type at least one letter."));
        }
        if name.chars().count() > NAME_LIMIT {
            return Err(AppError::new("That label name is too long")
                .why(format!(
                    "Names are up to {NAME_LIMIT} characters, so they fit on a tab."
                ))
                .fix("Use a shorter name."));
        }
        if self.find(name).is_none() {
            self.labels.push(GlobalLabel {
                name: name.to_owned(),
                color,
            });
        }
        Ok(())
    }

    /// Stops offering a label in every project. Projects that used it keep
    /// their own copy.
    pub fn remove(&mut self, name: &str) {
        let name = name.trim().to_lowercase();
        self.labels.retain(|l| l.name.to_lowercase() != name);
    }
}

fn damaged() -> AppError {
    AppError::new("Labels for every project can't be read")
        .why("The file holding them is damaged.")
        .fix("Delete labels.ron in the app's data folder and set them up again. Each project's own labels are not affected.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            GlobalLabels::load(dir.path()).unwrap(),
            GlobalLabels::default(),
            "no file: none"
        );
        let mut set = GlobalLabels::default();
        set.add(" Drone ", LabelColor::Green).unwrap();
        set.add("drone", LabelColor::Red).unwrap();
        set.add("Interview", LabelColor::Orange).unwrap();
        assert_eq!(set.labels.len(), 2, "one name once, whatever its capitals");
        assert_eq!(set.find("DRONE").map(|l| l.color), Some(LabelColor::Green));
        set.save(dir.path()).unwrap();
        assert_eq!(GlobalLabels::load(dir.path()).unwrap(), set);
        set.remove("drone");
        assert_eq!(set.labels.len(), 1);
        assert!(set.add("", LabelColor::Blue).is_err());
        assert!(set.add(&"x".repeat(NAME_LIMIT + 1), LabelColor::Blue).is_err());
    }

    #[test]
    fn newer_and_damaged_files_are_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = path(dir.path());
        std::fs::write(&file, "(format_version: 99, labels: [])").unwrap();
        let err = GlobalLabels::load(dir.path()).unwrap_err();
        assert!(err.what().contains("newer"), "{}", err.report());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "(format_version: 99, labels: [])"
        );
        std::fs::write(&file, "not ron").unwrap();
        assert!(GlobalLabels::load(dir.path()).is_err());
    }
}
