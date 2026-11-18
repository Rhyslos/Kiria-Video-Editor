//! What the person chose about updates.

use crate::manifest::Channel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Download and install new versions without asking (installed on the next start).
    pub automatic: bool,
    pub channel: Channel,
    /// A version the person chose to skip; newer versions are still offered.
    pub skipped_version: Option<String>,
    /// Versions that failed to start and were rolled back; never offered again.
    pub bad_versions: Vec<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs {
            automatic: true,
            channel: Channel::Stable,
            skipped_version: None,
            bad_versions: Vec::new(),
        }
    }
}

/// The file the choices are kept in, inside the app's data folder.
pub const PREFS_FILE_NAME: &str = "updates.ron";

impl UpdatePrefs {
    /// Reads the saved choices; a missing file gives the defaults. An
    /// unreadable one gives them too, but is logged and kept beside it as
    /// `updates.ron.unreadable` first ([`crate::state_file::read_ron`]), so
    /// the versions that failed to start can still be found.
    pub fn load(data_dir: &std::path::Path) -> UpdatePrefs {
        crate::state_file::read_ron(&data_dir.join(PREFS_FILE_NAME)).unwrap_or_default()
    }

    /// The file's text, for a writer of its own (the launcher hands it to a
    /// thread that keeps the file, off the interface thread).
    pub fn to_text(&self) -> diag::Result<String> {
        use diag::Explain;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).or_explain(|| {
            diag::AppError::new("Your update choices can't be saved")
                .why("They could not be turned into text.".to_owned())
        })
    }

    /// Reads the choices afresh, lets `change` change them and saves them,
    /// all while holding [`crate::ChoicesLock`], so a change another
    /// launcher makes at the same moment (a version marked bad) is never
    /// written away. Returns what was saved. Blocks for a few seconds at
    /// most: never call it on an interface thread.
    pub fn change(
        data_dir: &std::path::Path,
        change: impl FnOnce(&mut UpdatePrefs),
    ) -> diag::Result<UpdatePrefs> {
        let _lock = crate::ChoicesLock::wait(data_dir);
        let mut prefs = UpdatePrefs::load(data_dir);
        change(&mut prefs);
        prefs.save(data_dir)?;
        Ok(prefs)
    }

    /// Saves the choices, whole or not at all (a crash while saving leaves
    /// the previous file). Replaces whatever the file holds: to change some
    /// of the choices, use [`UpdatePrefs::change`].
    pub fn save(&self, data_dir: &std::path::Path) -> diag::Result<()> {
        use diag::Explain;
        let explain = || {
            diag::AppError::new("Your update choices can't be saved")
                .why(format!("Writing to {} failed.", data_dir.display()))
                .fix("Check that the disk is not full. The choice still applies until the app closes.")
        };
        let text = self.to_text()?;
        diag::disk::write_atomic(&data_dir.join(PREFS_FILE_NAME), text.as_bytes()).or_explain(explain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_survive_a_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), UpdatePrefs::default());
        let prefs = UpdatePrefs {
            automatic: false,
            channel: Channel::Beta,
            skipped_version: Some("0.2.0".into()),
            bad_versions: vec!["0.1.5".into()],
        };
        prefs.save(tmp.path()).unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), prefs);
    }

    fn bad_one() -> UpdatePrefs {
        UpdatePrefs {
            bad_versions: vec!["0.2.0".into()],
            ..UpdatePrefs::default()
        }
    }

    #[test]
    fn a_save_cut_short_leaves_the_previous_choices() {
        let tmp = tempfile::tempdir().unwrap();
        bad_one().save(tmp.path()).unwrap();
        // A crash in the middle of the next save: half a temporary file is left.
        let text = UpdatePrefs::default().to_text().unwrap();
        std::fs::write(
            tmp.path().join(format!(".{PREFS_FILE_NAME}.7.saving")),
            &text[..text.len() / 2],
        )
        .unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), bad_one());
        // And the next save still goes through.
        let mut prefs = bad_one();
        prefs.automatic = false;
        prefs.save(tmp.path()).unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), prefs);
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_with_its_bad_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PREFS_FILE_NAME);
        let text = bad_one().to_text().unwrap();
        let torn = &text[..text.len() - 3];
        std::fs::write(&path, torn).unwrap();
        assert_eq!(UpdatePrefs::load(tmp.path()), UpdatePrefs::default());
        let aside = crate::state_file::unreadable_path(&path);
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), torn);
        assert!(torn.contains("0.2.0"));
        // Saving fresh choices leaves the copy alone.
        UpdatePrefs::default().save(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), torn);
    }

    #[test]
    fn changes_at_the_same_moment_never_lose_a_bad_version() {
        let tmp = tempfile::tempdir().unwrap();
        let rounds = 40;
        std::thread::scope(|scope| {
            for writer in 0..2 {
                let data_dir = tmp.path();
                scope.spawn(move || {
                    for round in 0..rounds {
                        UpdatePrefs::change(data_dir, |prefs| {
                            prefs.bad_versions.push(format!("{writer}.{round}.0"));
                            // A slow writer: the other one's change lands in between without the lock.
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        })
                        .unwrap();
                    }
                });
            }
        });
        let mut bad = UpdatePrefs::load(tmp.path()).bad_versions;
        bad.sort();
        let mut expected: Vec<String> = (0..2)
            .flat_map(|w| (0..rounds).map(move |r| format!("{w}.{r}.0")))
            .collect();
        expected.sort();
        assert_eq!(bad, expected);
    }
}
