//! A sound clip's own EQ and clean-up (format 47): the same effects as a
//! track's, applied to that clip alone before its volume, pan, lines and
//! fades, so one noisy take can be cleaned without touching the rest of
//! its track. Set by [`crate::Command::SetClipFx`], on sound clips of
//! recordings only; a split keeps them on both parts. Also whether a clip
//! played at another speed keeps its pitch (format 48).

use crate::track_sound::{Cleanup, Eq, TrackSound};
use serde::{Deserialize, Serialize};

/// A clip's own sound effects. The default has both off.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipFx {
    pub eq: Eq,
    pub cleanup: Cleanup,
    /// Played faster or slower, the sound keeps its pitch (format 48)
    /// instead of going higher or lower like tape. Not with a speed ramp,
    /// whose pitch follows the speed.
    #[serde(skip_serializing_if = "crate::defaults::is_default")]
    pub keep_pitch: bool,
}

impl ClipFx {
    /// Whether either changes the sound.
    pub fn is_on(&self) -> bool {
        self.eq.on || self.cleanup.on
    }

    /// Whether `clip`'s sound is shifted back to its own pitch: kept, and
    /// played at one speed other than normal.
    pub fn keeps_pitch(clip: &crate::Clip) -> bool {
        clip.fx.keep_pitch && clip.ramp.is_empty() && clip.speed != crate::Speed::NORMAL
    }

    /// Refuses settings a track's EQ or clean-up would refuse.
    pub fn check(&self) -> diag::Result<()> {
        TrackSound {
            eq: self.eq,
            cleanup: self.cleanup.clone(),
            ..TrackSound::default()
        }
        .check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_at_first_and_checked_as_a_tracks() {
        let mut fx = ClipFx::default();
        assert!(!fx.is_on());
        assert!(fx.check().is_ok());
        fx.eq.on = true;
        assert!(fx.is_on());
        fx.eq.bands[0].gain_mdb = 40_000;
        assert!(fx.check().is_err(), "over +24 dB");
    }
}
