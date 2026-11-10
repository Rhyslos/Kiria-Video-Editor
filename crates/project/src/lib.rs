//! The project model, edit commands, undo/redo and autosave.
//!
//! - [`time`]: exact time and frame rates (fractions, never floats).
//! - [`ids`]: one id type per kind of thing.
//! - [`model`]: projects, timelines, tracks, clips, media, labels, the region.
//! - [`file`]: the versioned project file, atomic saves and migrations.
//!
//! Edit commands, undo and autosave arrive in Phase 0.4.

pub mod file;
pub mod ids;
pub mod model;
pub mod time;

pub use ids::{ClipId, LabelId, LinkId, MediaId, SequenceId, TrackId};
pub use model::{
    Clip, Fingerprint, LabelColor, MediaKind, MediaRef, Project, Region, Sequence, SequenceSettings,
    TimelineLabel, Track, TrackKind,
};
pub use time::{Rate, Time};

/// File extension for project files, without the dot.
pub const FILE_EXTENSION: &str = brand::PROJECT_FILE_EXTENSION;
