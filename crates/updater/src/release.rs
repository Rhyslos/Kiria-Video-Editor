//! Asking GitHub whether a newer version exists.

use crate::manifest::Channel;
use crate::prefs::UpdatePrefs;
use diag::{AppError, Explain};
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

/// How long the check may take before it gives up. Being offline must never delay opening.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// A release on GitHub that can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub prerelease: bool,
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The answer to "is there an update?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    UpToDate,
    /// A newer version the person chose to skip.
    Skipped(Version),
    Available(Release),
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Asks GitHub for the newest release for this channel.
pub fn check(current: &str, prefs: &UpdatePrefs) -> diag::Result<CheckResult> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases?per_page=20",
        brand::RELEASE_REPO_OWNER,
        brand::RELEASE_REPO_NAME
    );
    let agent = crate::agent(CHECK_TIMEOUT);
    let body = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|err| explain_http(&err))?
        .body_mut()
        .read_to_string()
        .or_explain(|| unreachable_server("The answer from GitHub was cut off."))?;
    decide(current, prefs, &body)
}

/// Picks the newest suitable release from GitHub's answer. Separate from
/// [`check`] so it can be tested without a network.
pub fn decide(current: &str, prefs: &UpdatePrefs, github_json: &str) -> diag::Result<CheckResult> {
    let current = Version::parse(current).or_explain(|| {
        AppError::new("The update check could not run").why(format!("\"{current}\" is not a version number."))
    })?;
    let releases: Vec<GithubRelease> = serde_json::from_str(github_json)
        .or_explain(|| unreachable_server("GitHub's answer could not be read."))?;
    // A beta build keeps following betas, whatever the channel setting says.
    let betas = prefs.channel == Channel::Beta || !current.pre.is_empty();
    let newest = releases
        .into_iter()
        .filter(|r| !r.draft && (betas || !r.prerelease))
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            Some(Release {
                version,
                prerelease: r.prerelease,
                notes: r.body.unwrap_or_default(),
                assets: r
                    .assets
                    .into_iter()
                    .map(|a| Asset {
                        name: a.name,
                        url: a.browser_download_url,
                        size: a.size,
                    })
                    .collect(),
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version));
    Ok(match newest {
        Some(release) if release.version > current => {
            let skipped = prefs
                .skipped_version
                .as_deref()
                .and_then(|v| Version::parse(v).ok())
                .is_some_and(|v| v == release.version);
            if skipped {
                CheckResult::Skipped(release.version)
            } else {
                CheckResult::Available(release)
            }
        }
        _ => CheckResult::UpToDate,
    })
}

fn explain_http(err: &ureq::Error) -> AppError {
    match err {
        ureq::Error::StatusCode(403 | 429) => AppError::new("Updates can't be checked right now")
            .why("GitHub is limiting how often it can be asked.")
            .fix("Nothing to do; the app checks again next time it opens."),
        ureq::Error::StatusCode(code) => unreachable_server(&format!("GitHub answered with error {code}.")),
        ureq::Error::Timeout(_) => unreachable_server("GitHub did not answer within 3 seconds."),
        _ => unreachable_server("There seems to be no internet connection."),
    }
}

fn unreachable_server(why: &str) -> AppError {
    AppError::new("Updates can't be checked right now")
        .why(why.to_owned())
        .fix("Nothing to do; the app works offline and checks again next time it opens.")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASES: &str = r#"[
        {"tag_name": "v0.3.0-beta.1", "draft": false, "prerelease": true, "body": "beta", "assets": []},
        {"tag_name": "v0.2.0", "draft": false, "prerelease": false, "body": "notes",
         "assets": [{"name": "manifest.json", "browser_download_url": "https://x/m", "size": 10}]},
        {"tag_name": "v0.9.0", "draft": true, "prerelease": false, "assets": []},
        {"tag_name": "deps-ffmpeg-8.1", "draft": false, "prerelease": true, "assets": []}
    ]"#;

    #[test]
    fn stable_ignores_betas_drafts_and_non_versions() {
        let r = decide("0.1.0", &UpdatePrefs::default(), RELEASES).unwrap();
        let CheckResult::Available(release) = r else {
            panic!("{r:?}")
        };
        assert_eq!(release.version, Version::new(0, 2, 0));
        assert_eq!(release.asset("manifest.json").unwrap().size, 10);
    }

    #[test]
    fn beta_sees_prereleases() {
        let prefs = UpdatePrefs {
            channel: Channel::Beta,
            ..UpdatePrefs::default()
        };
        let CheckResult::Available(release) = decide("0.1.0", &prefs, RELEASES).unwrap() else {
            panic!()
        };
        assert_eq!(release.version.to_string(), "0.3.0-beta.1");
    }

    #[test]
    fn a_beta_build_follows_betas_on_the_stable_channel() {
        let CheckResult::Available(release) =
            decide("0.1.1-beta.3", &UpdatePrefs::default(), RELEASES).unwrap()
        else {
            panic!()
        };
        assert_eq!(release.version.to_string(), "0.3.0-beta.1");
    }

    #[test]
    fn up_to_date_and_skipped() {
        assert_eq!(
            decide("0.2.0", &UpdatePrefs::default(), RELEASES).unwrap(),
            CheckResult::UpToDate
        );
        let prefs = UpdatePrefs {
            skipped_version: Some("0.2.0".into()),
            ..UpdatePrefs::default()
        };
        assert_eq!(
            decide("0.1.0", &prefs, RELEASES).unwrap(),
            CheckResult::Skipped(Version::new(0, 2, 0))
        );
    }

    #[test]
    fn a_garbled_answer_is_explained() {
        let err = decide("0.1.0", &UpdatePrefs::default(), "<html>").unwrap_err();
        assert!(err.what().contains("can't be checked"));
    }
}
