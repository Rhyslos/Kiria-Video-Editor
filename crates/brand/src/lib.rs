//! The one place the app's name and identity live.
//!
//! Nothing else in the code base may type the app's name. Window titles,
//! installers, update checks and folder paths all read these constants, so a
//! rename is a change to this file only. The test at the bottom enforces it.

/// The name people see: window titles, the launcher, installers.
pub const APP_NAME: &str = "Kiria";

/// A shorter form for tight spaces such as task bars. Same as the name for now.
pub const APP_SHORT_NAME: &str = APP_NAME;

/// Reverse-domain identifier used by the operating system (file associations,
/// desktop entries, app data folders on some platforms).
pub const APP_ID: &str = "io.github.rhyslos.kiria";

/// File extension for project files, without the dot.
pub const PROJECT_FILE_EXTENSION: &str = "kiria";

/// Folder name under the user's data and cache directories.
pub const DATA_DIR_NAME: &str = APP_NAME;

/// GitHub owner and repository the updater asks for new releases.
pub const RELEASE_REPO_OWNER: &str = "Rhyslos";
pub const RELEASE_REPO_NAME: &str = "kiria-video-editor";

/// The version of this build. Normally the workspace version from
/// `Cargo.toml`; automatic test builds set `BUILD_VERSION` (for example
/// `0.1.1-beta.12`) so they sort after the release they build on.
pub const VERSION: &str = match option_env!("BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Title for a window, for example `window_title("Lofoten trip")`
/// gives "Lofoten trip – Kiria".
pub fn window_title(document: &str) -> String {
    format!("{document} – {APP_NAME}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn window_title_puts_the_document_first() {
        assert_eq!(window_title("Clip"), format!("Clip – {APP_NAME}"));
    }

    /// Fails if the app's name is typed in any Rust file outside this crate.
    #[test]
    fn name_is_only_typed_in_the_brand_crate() {
        let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let this_crate = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .expect("brand crate path");
        let mut offenders = Vec::new();
        for file in rust_files(&crates_dir) {
            if file.canonicalize().is_ok_and(|f| f.starts_with(&this_crate)) {
                continue;
            }
            let text = fs::read_to_string(&file).unwrap_or_default();
            if text.contains(APP_NAME) {
                offenders.push(file);
            }
        }
        assert!(
            offenders.is_empty(),
            "the app name is typed outside crates/brand; read it from `brand::APP_NAME` instead: {offenders:?}"
        );
    }

    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return found;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                found.extend(rust_files(&path));
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path);
            }
        }
        found
    }
}
