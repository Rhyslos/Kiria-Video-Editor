//! Fetching a whole update: the signed manifest first, then the package,
//! each checked before the next step.

use crate::download::download;
use crate::manifest::{Manifest, RELEASE_PUBLIC_KEY, tampered};
use crate::release::Release;
use diag::{AppError, Explain};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A downloaded, verified update, ready to install (Phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyUpdate {
    pub version: String,
    pub package: PathBuf,
    pub manifest: Manifest,
}

/// Downloads `release` into `<downloads>/<version>/` and verifies everything.
pub fn fetch(release: &Release, downloads: &Path) -> diag::Result<ReadyUpdate> {
    let version = release.version.to_string();
    let dir = downloads.join(&version);
    let manifest_bytes = small_asset(release, "manifest.json")?;
    let signature = String::from_utf8(small_asset(release, "manifest.json.sig")?).or_explain(tampered)?;
    let manifest = Manifest::verify_and_parse(&manifest_bytes, &signature, &RELEASE_PUBLIC_KEY)?;
    if manifest.version != version {
        return Err(tampered());
    }
    let name = crate::package_name(&version);
    let entry = manifest.file(&name).ok_or_else(|| missing(&name))?;
    let asset = release.asset(&name).ok_or_else(|| missing(&name))?;
    let package = download(&asset.url, &dir, entry)?;
    Ok(ReadyUpdate {
        version,
        package,
        manifest,
    })
}

fn small_asset(release: &Release, name: &str) -> diag::Result<Vec<u8>> {
    let asset = release.asset(name).ok_or_else(|| missing(name))?;
    let agent = crate::agent(Duration::from_secs(20));
    agent
        .get(&asset.url)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_vec().ok())
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
