//! Release checks, signature checks and downloads.
//!
//! - [`release::check`] asks GitHub for the newest release on the chosen
//!   channel, giving up after 3 seconds so being offline never delays opening.
//! - [`manifest`] verifies a release's signed file list against the public
//!   key built into the app.
//! - [`download::download`] fetches a file in the background, resumes a
//!   partial download, and refuses anything whose checksum does not match.
//!
//! Installing side by side and rolling back arrive in Phase 3.

pub mod download;
pub mod fetch;
pub mod install;
pub mod manifest;
pub mod prefs;
pub mod release;

pub use fetch::{ReadyUpdate, fetch};
pub use install::Layout;
pub use manifest::{Channel, Manifest, ManifestFile, RELEASE_PUBLIC_KEY};
pub use prefs::UpdatePrefs;
pub use release::{CheckResult, Release};

use std::time::Duration;

/// The release asset holding the app for this platform.
pub fn package_name(version: &str) -> String {
    let platform = if cfg!(windows) {
        "windows-x64.zip"
    } else {
        "linux-x86_64.AppImage"
    };
    format!("{}-{version}-{platform}", brand::APP_NAME)
}

/// An HTTP client with a time limit, identifying itself as the app.
fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!(
            "{}-updater/{}",
            brand::APP_NAME,
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}
