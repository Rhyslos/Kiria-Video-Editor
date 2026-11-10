//! Installing a downloaded update.
//!
//! The launcher calls these on start, before any window opens:
//!
//! 1. [`pending`] finds the newest download newer than the running version
//!    and checks its signature and checksum again.
//! 2. [`install`] puts it next to the current version (never over it) and
//!    switches the `current` file to it in one rename. On Linux the AppImage
//!    file is replaced in one rename instead.
//! 3. [`replace_launcher`] swaps in the new version's launcher when it is
//!    newer than the running one, so launcher changes arrive too.
//!
//! Earlier versions are kept for rolling back; [`crate::health`] removes all
//! but the two newest once the new version has started properly.

use crate::download::verify_file;
use crate::fetch::{MANIFEST_NAME, ReadyUpdate, SIGNATURE_NAME};
use crate::install::{CURRENT_FILE, Layout, VERSIONS_DIR, editor_file_name};
use crate::manifest::{Manifest, RELEASE_PUBLIC_KEY};
use diag::{AppError, Explain};
use semver::Version;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// How the running copy was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// The Windows layout: `current` plus `versions/<version>/`.
    Folders(Layout),
    /// A single AppImage file (Linux); the path is the file itself.
    AppImage(PathBuf),
    /// A development build: nothing is installed or replaced.
    Development,
}

impl InstallKind {
    /// Works out how the launcher in `root` was installed.
    pub fn detect(root: &Path) -> InstallKind {
        if let Some(path) = std::env::var_os("APPIMAGE") {
            return InstallKind::AppImage(PathBuf::from(path));
        }
        let layout = Layout::at(root);
        if layout.current_version().is_some() {
            InstallKind::Folders(layout)
        } else {
            InstallKind::Development
        }
    }
}

/// The launcher's file name on this platform.
pub fn launcher_file_name() -> &'static str {
    if cfg!(windows) { "launcher.exe" } else { "launcher" }
}

/// Where the replaced launcher is parked until the next start can delete it.
/// (A running program can be renamed on Windows, but not deleted.)
fn old_launcher_name() -> &'static str {
    if cfg!(windows) {
        "launcher.old.exe"
    } else {
        "launcher.old"
    }
}

/// The newest verified download in `downloads/<version>/` that is newer than
/// `current` and not one of the `bad` versions. Downloads that fail the checks are deleted.
pub fn pending(downloads: &Path, current: &str, bad: &[String]) -> Option<ReadyUpdate> {
    let current = Version::parse(current).ok()?;
    let mut candidates: Vec<(Version, PathBuf)> = fs::read_dir(downloads)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version = Version::parse(&entry.file_name().to_string_lossy()).ok()?;
            (version > current && !bad.contains(&version.to_string())).then(|| (version, entry.path()))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (version, dir) in candidates {
        match verify_download(&dir, &version.to_string()) {
            Ok(ready) => return Some(ready),
            Err(err) => {
                // Half-finished downloads have no manifest yet: leave those to resume.
                if dir.join(MANIFEST_NAME).exists() {
                    tracing::warn!(dir = %dir.display(), "{}", err.report());
                    let _ = fs::remove_dir_all(&dir);
                }
            }
        }
    }
    None
}

/// Checks a download folder: the manifest's signature, its version, and the package's checksum.
fn verify_download(dir: &Path, version: &str) -> diag::Result<ReadyUpdate> {
    let unreadable = || {
        AppError::new("The downloaded update is incomplete")
            .why(format!("{} is missing files.", dir.display()))
            .fix("Nothing was installed; the app downloads it again.")
    };
    let bytes = fs::read(dir.join(MANIFEST_NAME)).or_explain(unreadable)?;
    let signature = fs::read_to_string(dir.join(SIGNATURE_NAME)).or_explain(unreadable)?;
    verify_parts(dir, version, &bytes, &signature, &RELEASE_PUBLIC_KEY)
}

fn verify_parts(
    dir: &Path,
    version: &str,
    manifest_bytes: &[u8],
    signature: &str,
    public_key: &[u8; 32],
) -> diag::Result<ReadyUpdate> {
    let manifest = Manifest::verify_and_parse(manifest_bytes, signature, public_key)?;
    if manifest.version != version {
        return Err(crate::manifest::tampered());
    }
    let name = crate::package_name(version);
    let entry = manifest.file(&name).ok_or_else(crate::manifest::tampered)?;
    let package = dir.join(&name);
    verify_file(&package, entry)?;
    Ok(ReadyUpdate {
        version: version.to_owned(),
        package,
        manifest,
    })
}

/// Installs a verified update. Nothing the running copy uses is changed until
/// the new version is complete; then one rename switches over.
pub fn install(kind: &InstallKind, ready: &ReadyUpdate) -> diag::Result<()> {
    match kind {
        InstallKind::Folders(layout) => {
            unpack(&ready.package, layout, &ready.version)?;
            set_current(layout, &ready.version)
        }
        InstallKind::AppImage(file) => replace_file(&ready.package, file),
        InstallKind::Development => Ok(()),
    }
}

fn install_failed(why: String) -> AppError {
    AppError::new("The update could not be installed")
        .why(why)
        .fix("Nothing changed: the current version still opens. The app tries again next time.")
}

/// Unpacks the update package into `versions/<version>/`.
fn unpack(package: &Path, layout: &Layout, version: &str) -> diag::Result<()> {
    let versions = layout.root().join(VERSIONS_DIR);
    let target = layout.version_dir(version);
    let partial = versions.join(format!(".{version}.partial"));
    let failed = || install_failed(format!("Unpacking into {} failed.", versions.display()));
    if partial.exists() {
        fs::remove_dir_all(&partial).or_explain(failed)?;
    }
    fs::create_dir_all(&partial).or_explain(failed)?;

    let mut archive = zip::ZipArchive::new(File::open(package).or_explain(failed)?).or_explain(failed)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).or_explain(failed)?;
        // Names that would land outside the folder are refused outright.
        let Some(relative) = entry.enclosed_name() else {
            return Err(install_failed(
                "The package contains a file name that points outside its folder.".into(),
            ));
        };
        let out = partial.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&out).or_explain(failed)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).or_explain(failed)?;
        }
        let mut file = File::create(&out).or_explain(failed)?;
        std::io::copy(&mut entry, &mut file).or_explain(failed)?;
        file.sync_all().or_explain(failed)?;
    }
    if !partial.join(editor_file_name()).is_file() {
        let _ = fs::remove_dir_all(&partial);
        return Err(install_failed("The package has no editor in it.".into()));
    }
    if target.exists() {
        if layout.current_version().as_deref() == Some(version) {
            return Err(install_failed(format!(
                "{version} is already the current version."
            )));
        }
        fs::remove_dir_all(&target).or_explain(failed)?;
    }
    fs::rename(&partial, &target).or_explain(failed)
}

/// Points the `current` file at `version` in one rename.
pub fn set_current(layout: &Layout, version: &str) -> diag::Result<()> {
    let file = layout.root().join(CURRENT_FILE);
    let temp = layout.root().join(format!("{CURRENT_FILE}.new"));
    let failed = || install_failed(format!("{} could not be written.", file.display()));
    fs::write(&temp, version).or_explain(failed)?;
    fs::rename(&temp, &file).or_explain(failed)
}

/// Replaces `target` with `source`: a copy beside it first, then one rename.
fn replace_file(source: &Path, target: &Path) -> diag::Result<()> {
    let temp = target.with_extension("new");
    let failed = || install_failed(format!("{} could not be replaced.", target.display()));
    fs::copy(source, &temp).or_explain(failed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o755)).or_explain(failed)?;
    }
    fs::rename(&temp, target).or_explain(failed)
}

/// Removes a download folder once it is installed.
pub fn forget_download(downloads: &Path, version: &str) {
    let _ = fs::remove_dir_all(downloads.join(version));
}

/// Removes installed versions other than `keep` (the current and previous ones).
pub fn prune(layout: &Layout, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(layout.root().join(VERSIONS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !keep.contains(&name.as_str()) && Version::parse(&name).is_ok() {
            tracing::info!(version = %name, "removing an old version");
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Swaps in the current version's launcher if it is newer than the running one
/// (`own_version`). Returns true when it did; the caller should then start the
/// new launcher and exit.
pub fn replace_launcher(layout: &Layout, own_version: &str) -> diag::Result<bool> {
    let Some(current) = layout.current_version() else {
        return Ok(false);
    };
    let newer = match (Version::parse(&current), Version::parse(own_version)) {
        (Ok(current), Ok(own)) => current > own,
        _ => false,
    };
    let new = layout.version_dir(&current).join(launcher_file_name());
    if !newer || !new.is_file() {
        return Ok(false);
    }
    let exe = layout.root().join(launcher_file_name());
    let old = layout.root().join(old_launcher_name());
    let failed = || {
        install_failed(format!(
            "The launcher in {} could not be replaced.",
            layout.root().display()
        ))
    };
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).or_explain(failed)?;
    if let Err(err) = fs::copy(&new, &exe) {
        let _ = fs::rename(&old, &exe);
        return Err(failed().source(err));
    }
    Ok(true)
}

/// Deletes the launcher parked by [`replace_launcher`] on an earlier start.
pub fn remove_old_launcher(layout: &Layout) {
    let old = layout.root().join(old_launcher_name());
    if old.exists() {
        let _ = fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Channel, ManifestFile, sha256_hex, sign};
    use std::io::Write;

    const SECRET: [u8; 32] = [7; 32];

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        out.into_inner()
    }

    /// A download folder for `version` holding `package`, signed with the test key.
    fn download_dir(root: &Path, version: &str, package: &[u8]) -> (PathBuf, Vec<u8>, String) {
        let dir = root.join(version);
        fs::create_dir_all(&dir).unwrap();
        let name = crate::package_name(version);
        fs::write(dir.join(&name), package).unwrap();
        let manifest = Manifest {
            version: version.into(),
            channel: Channel::Stable,
            min_launcher_version: "0.1.0".into(),
            files: vec![ManifestFile {
                name,
                size: package.len() as u64,
                sha256: sha256_hex(package),
            }],
        };
        let bytes = manifest.to_bytes();
        let signature = sign(&bytes, &SECRET);
        fs::write(dir.join(MANIFEST_NAME), &bytes).unwrap();
        fs::write(dir.join(SIGNATURE_NAME), &signature).unwrap();
        (dir, bytes, signature)
    }

    fn test_key() -> [u8; 32] {
        crate::manifest::public_key_of(&SECRET)
    }

    #[test]
    fn a_signed_download_verifies_and_a_changed_package_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, bytes, signature) = download_dir(tmp.path(), "0.2.0", b"package");
        assert!(verify_parts(&dir, "0.2.0", &bytes, &signature, &test_key()).is_ok());
        assert!(
            verify_parts(&dir, "0.3.0", &bytes, &signature, &test_key()).is_err(),
            "wrong version"
        );
        fs::write(dir.join(crate::package_name("0.2.0")), b"packagf").unwrap();
        assert!(verify_parts(&dir, "0.2.0", &bytes, &signature, &test_key()).is_err());
    }

    #[test]
    fn downloads_signed_with_another_key_are_never_pending() {
        // The test key is not the release key, so this download must be refused and removed.
        let tmp = tempfile::tempdir().unwrap();
        download_dir(tmp.path(), "9.0.0", b"package");
        assert!(pending(tmp.path(), "0.1.0", &[]).is_none());
        assert!(!tmp.path().join("9.0.0").exists());
    }

    #[test]
    fn unpacking_installs_beside_the_current_version_and_switches() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        let old = layout.version_dir("0.1.0");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join(editor_file_name()), "old").unwrap();
        set_current(&layout, "0.1.0").unwrap();

        let package = tmp.path().join("pkg.zip");
        let launcher = launcher_file_name();
        fs::write(
            &package,
            zip_with(&[
                (editor_file_name(), b"new editor"),
                (launcher, b"new launcher"),
                ("licenses/A.txt", b"a"),
            ]),
        )
        .unwrap();
        let ready = ReadyUpdate {
            version: "0.2.0".into(),
            package,
            manifest: Manifest {
                version: "0.2.0".into(),
                channel: Channel::Stable,
                min_launcher_version: "0.1.0".into(),
                files: vec![],
            },
        };
        install(&InstallKind::Folders(layout.clone()), &ready).unwrap();
        assert_eq!(layout.current_version().as_deref(), Some("0.2.0"));
        assert_eq!(fs::read(layout.editor().unwrap()).unwrap(), b"new editor");
        assert!(layout.version_dir("0.2.0").join("licenses/A.txt").is_file());
        assert!(
            old.join(editor_file_name()).is_file(),
            "the old version stays for rolling back"
        );

        prune(&layout, &["0.2.0"]);
        assert!(!old.exists());
    }

    #[test]
    fn a_package_without_an_editor_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        set_current(&layout, "0.1.0").unwrap();
        let package = tmp.path().join("pkg.zip");
        fs::write(&package, zip_with(&[("readme.txt", b"no editor")])).unwrap();
        let err = unpack(&package, &layout, "0.2.0").unwrap_err();
        assert!(err.what().contains("could not be installed"));
        assert_eq!(layout.current_version().as_deref(), Some("0.1.0"));
        assert!(!layout.version_dir("0.2.0").exists());
    }

    #[test]
    fn names_escaping_the_folder_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        let package = tmp.path().join("pkg.zip");
        fs::write(
            &package,
            zip_with(&[("../evil.txt", b"x"), (editor_file_name(), b"e")]),
        )
        .unwrap();
        assert!(unpack(&package, &layout, "0.2.0").is_err());
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn a_newer_launcher_is_swapped_in_and_the_old_one_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path());
        fs::write(tmp.path().join(launcher_file_name()), "running launcher").unwrap();
        let dir = layout.version_dir("0.2.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(launcher_file_name()), "new launcher").unwrap();
        set_current(&layout, "0.2.0").unwrap();

        assert!(
            !replace_launcher(&layout, "0.2.0").unwrap(),
            "same version: nothing to do"
        );
        assert!(replace_launcher(&layout, "0.1.0").unwrap());
        assert_eq!(
            fs::read(tmp.path().join(launcher_file_name())).unwrap(),
            b"new launcher"
        );
        assert!(tmp.path().join(old_launcher_name()).exists());
        remove_old_launcher(&layout);
        assert!(!tmp.path().join(old_launcher_name()).exists());
    }

    #[test]
    fn an_appimage_is_replaced_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("app.AppImage");
        let source = tmp.path().join("new.AppImage");
        fs::write(&target, "old").unwrap();
        fs::write(&source, "new").unwrap();
        replace_file(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }
}
