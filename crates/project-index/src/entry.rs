//! What the index stores about a project and a folder.

use project::{SequenceSettings, Time};
use std::path::PathBuf;

/// The three groups of the launcher's folder tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FolderKind {
    /// Where projects live. New projects are saved into one of these, and they are searched for project files.
    Projects,
    /// Folders of recordings, remembered so importing can start there (Phase 5).
    Media,
    /// Any other folder; searched for project files like project folders.
    Other,
}

impl FolderKind {
    pub const ALL: [FolderKind; 3] = [FolderKind::Projects, FolderKind::Media, FolderKind::Other];

    pub fn label(self) -> &'static str {
        match self {
            FolderKind::Projects => "Projects",
            FolderKind::Media => "Media folders",
            FolderKind::Other => "Other folders",
        }
    }

    /// Whether project files in folders of this kind are listed.
    pub fn holds_projects(self) -> bool {
        matches!(self, FolderKind::Projects | FolderKind::Other)
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            FolderKind::Projects => "projects",
            FolderKind::Media => "media",
            FolderKind::Other => "other",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<FolderKind> {
        FolderKind::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// A folder in the launcher's folder tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub kind: FolderKind,
}

impl Folder {
    /// The folder's own name, for the tree.
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

/// Something wrong with a project that its card should say before it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issue {
    /// The file is no longer where it was.
    Missing,
    /// A newer version of the app saved it; this version can't open it.
    NewerVersion,
    /// The file could not be read as a project.
    Damaged,
}

impl Issue {
    /// The line shown on the project card.
    pub fn message(self) -> &'static str {
        match self {
            Issue::Missing => "File not found. It was moved, renamed or deleted.",
            Issue::NewerVersion => "Saved by a newer version. Opens read-only.",
            Issue::Damaged => "The file is damaged and can't be read.",
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Issue::Missing => "missing",
            Issue::NewerVersion => "newer",
            Issue::Damaged => "damaged",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Issue> {
        [Issue::Missing, Issue::NewerVersion, Issue::Damaged]
            .into_iter()
            .find(|i| i.key() == key)
    }
}

/// One project card.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    /// Missing when the file can't be read.
    pub settings: Option<SequenceSettings>,
    pub length: Option<Time>,
    /// Seconds since 1970 when it was last opened from the launcher, if ever.
    pub last_opened: Option<i64>,
    /// Seconds since 1970 when the file last changed.
    pub modified: Option<i64>,
    pub issue: Option<Issue>,
    /// The timeline's settings wait for the first video imported.
    pub automatic: bool,
}

impl ProjectEntry {
    /// The folder the project file is in.
    pub fn folder(&self) -> Option<&std::path::Path> {
        self.path.parent()
    }

    /// The settings in a few words: "1440p 60", or "Automatic" while they
    /// wait for the first video.
    pub fn settings_label(&self) -> Option<String> {
        if self.automatic {
            Some("Automatic".into())
        } else {
            self.settings.map(|s| s.short_label())
        }
    }

    /// When it was last used: opened from here, or else changed on disk.
    pub fn last_used(&self) -> Option<i64> {
        self.last_opened.or(self.modified)
    }
}
