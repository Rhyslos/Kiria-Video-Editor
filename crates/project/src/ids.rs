//! Identifiers for things in a project.
//!
//! Each kind of thing has its own id type, so a clip id can never be passed
//! where a track id is expected. Ids come from one counter in the project and
//! are never reused, which keeps undo and references simple.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }

        impl From<u64> for $name {
            fn from(raw: u64) -> Self {
                Self(raw)
            }
        }
    };
}

id_type!(
    /// A media file known to the project.
    MediaId, "media#"
);
id_type!(
    /// A timeline.
    SequenceId, "sequence#"
);
id_type!(
    /// A video or audio track.
    TrackId, "track#"
);
id_type!(
    /// A clip on a track.
    ClipId, "clip#"
);
id_type!(
    /// A timeline label.
    LabelId, "label#"
);
id_type!(
    /// Ties clips that move together, such as a video clip and its own audio.
    LinkId, "link#"
);
