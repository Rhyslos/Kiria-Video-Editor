//! Fetching a whole update: the signed manifest first, then the package,
//! each checked before the next step.

use crate::download::{Progress, download_with_progress};
use crate::manifest::{Manifest, RELEASE_PUBLIC_KEY, tampered};
use crate::release::Release;
use diag::{AppError, Explain};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The signed file list's name in a release and in a download folder.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Its signature's name.
pub const SIGNATURE_NAME: &str = "manifest.json.sig";

/// The most a manifest may hold: a short list of files, far below this.
const MANIFEST_LIMIT: u64 = 256 * 1024;
/// The most a signature may hold: 88 characters of base64 and a line end.
const SIGNATURE_LIMIT: u64 = 1024;

/// A downloaded, verified update, ready to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyUpdate {
    pub version: String,
    pub package: PathBuf,
    pub manifest: Manifest,
}

/// Downloads `release` into `<downloads>/<version>/` and verifies everything.
pub fn fetch(release: &Release, downloads: &Path) -> diag::Result<ReadyUpdate> {
    fetch_with_progress(release, downloads, &Progress::new())
}

/// [`fetch`], reporting the package download to `progress`.
pub fn fetch_with_progress(
    release: &Release,
    downloads: &Path,
    progress: &Progress,
) -> diag::Result<ReadyUpdate> {
    let version = release.version.to_string();
    let dir = downloads.join(&version);
    let manifest_bytes = small_asset(release, MANIFEST_NAME, MANIFEST_LIMIT)?;
    let signature =
        String::from_utf8(small_asset(release, SIGNATURE_NAME, SIGNATURE_LIMIT)?).or_explain(tampered)?;
    let manifest = Manifest::verify_and_parse(&manifest_bytes, &signature, &RELEASE_PUBLIC_KEY)?;
    if manifest.version != version {
        return Err(tampered());
    }
    // Kept next to the package so the launcher can check everything again
    // right before installing it on a later start.
    let save = || {
        AppError::new("The update could not be saved")
            .why(format!("Writing to {} failed.", dir.display()))
            .fix("Check that the disk is not full.")
    };
    std::fs::create_dir_all(&dir).or_explain(save)?;
    std::fs::write(dir.join(MANIFEST_NAME), &manifest_bytes).or_explain(save)?;
    std::fs::write(dir.join(SIGNATURE_NAME), signature.as_bytes()).or_explain(save)?;
    let name = crate::package_name(&version);
    let entry = manifest.file(&name).ok_or_else(|| missing(&name))?;
    let asset = release.asset(&name).ok_or_else(|| missing(&name))?;
    let package = download_with_progress(&asset.url, &dir, entry, progress)?;
    Ok(ReadyUpdate {
        version,
        package,
        manifest,
    })
}

/// Downloads a small file of the release, refusing one larger than `limit`
/// bytes (before it is signature-checked, so a server cannot fill memory).
fn small_asset(release: &Release, name: &str, limit: u64) -> diag::Result<Vec<u8>> {
    let asset = release.asset(name).ok_or_else(|| missing(name))?;
    small_file(&asset.url, name, limit)
}

fn small_file(url: &str, name: &str, limit: u64) -> diag::Result<Vec<u8>> {
    let agent = crate::agent(Duration::from_secs(20));
    agent
        .get(url)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().with_config().limit(limit).read_to_vec().ok())
        .ok_or_else(|| {
            AppError::new("The update could not be downloaded")
                .why(format!("{name} could not be fetched from GitHub."))
                .fix("Nothing was installed; the app tries again next time it opens.")
        })
}

fn missing(name: &str) -> AppError {
    AppError::new("This update is incomplete")
        .why(format!("The release has no {name}."))
        .fix("Nothing was installed. The next release will be offered when it appears.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{TestServer, response};

    #[test]
    fn a_small_file_within_its_limit_is_read() {
        let server = TestServer::start(1, |_| response("200 OK", &["content-length: 5".into()], b"hello"));
        assert_eq!(small_file(&server.url, "x", 16).unwrap(), b"hello");
    }

    #[test]
    fn a_small_file_past_its_limit_is_refused() {
        let server = TestServer::start(1, |_| response("200 OK", &[], &[b'x'; 4096]));
        let err = small_file(&server.url, SIGNATURE_NAME, SIGNATURE_LIMIT).unwrap_err();
        assert!(err.what().contains("could not be downloaded"));
    }
}
