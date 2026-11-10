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
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        UpdatePrefs {
            automatic: true,
            channel: Channel::Stable,
            skipped_version: None,
        }
    }
}
