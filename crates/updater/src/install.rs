//! Where an installed app keeps its versions.
//!
//! ```text
//! <install root>/
//!   launcher(.exe)        stays outside the versions so it can always recover them
//!   current               text file: the version to start, e.g. "0.1.0"
//!   versions/0.1.0/       one folder per installed version: editor, FFmpeg, licences
//! ```
//!
//! A development build or the Linux AppImage has no `versions` folder; the
//! editor then sits next to the launcher.

use std::path::{Path, PathBuf};

pub const CURRENT_FILE: &str = "current";
pub const VERSIONS_DIR: &str = "versions";

/// The editor's file name on this platform.
pub fn editor_file_name() -> &'static str {
    if cfg!(windows) { "editor.exe" } else { "editor" }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The layout around a launcher installed in `root`.
    pub fn at(root: impl Into<PathBuf>) -> Layout {
        Layout { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The version the `current` file names, if any.
    pub fn current_version(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join(CURRENT_FILE)).ok()?;
        let version = text.trim();
        (!version.is_empty()).then(|| version.to_owned())
    }

    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.root.join(VERSIONS_DIR).join(version)
    }

    /// The editor to start: the current version's, or one next to the launcher.
    pub fn editor(&self) -> Option<PathBuf> {
        let installed = self
            .current_version()
            .map(|v| self.version_dir(&v).join(editor_file_name()))
            .filter(|p| p.is_file());
        installed.or_else(|| Some(self.root.join(editor_file_name())).filter(|p| p.is_file()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_current_version_wins_over_a_loose_editor() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        assert_eq!(layout.editor(), None);

        fs::write(tmp.path().join(editor_file_name()), "").unwrap();
        assert_eq!(layout.editor(), Some(tmp.path().join(editor_file_name())));

        let dir = layout.version_dir("0.2.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(editor_file_name()), "").unwrap();
        fs::write(tmp.path().join(CURRENT_FILE), "0.2.0\n").unwrap();
        assert_eq!(layout.current_version().as_deref(), Some("0.2.0"));
        assert_eq!(layout.editor(), Some(dir.join(editor_file_name())));
    }

    #[test]
    fn a_current_file_pointing_nowhere_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(CURRENT_FILE), "9.9.9").unwrap();
        fs::write(tmp.path().join(editor_file_name()), "").unwrap();
        assert_eq!(
            Layout::at(tmp.path()).editor(),
            Some(tmp.path().join(editor_file_name()))
        );
    }
}
