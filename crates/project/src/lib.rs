//! The project model, edit commands, undo/redo and autosave.
//!
//! - [`time`]: exact time and frame rates (fractions, never floats).
//! - [`ids`]: one id type per kind of thing.
//! - [`model`]: projects, timelines, tracks, clips, media, labels, the region.
//! - [`file`]: the versioned project file, atomic saves and migrations.
//! - [`command`]: every change is a `Command` applied to a snapshot.
//! - [`history`]: unlimited undo and redo over snapshots.
//! - [`document`]: an open project on disk, with the autosave journal and crash recovery.

pub mod command;
pub mod document;
mod edits;
pub mod file;
pub mod history;
pub mod ids;
pub mod model;
pub mod time;
pub mod versions;

pub use command::{Command, Edge};
pub use document::{Document, OpenCheck, Recovery, thumbnail_path};
pub use history::History;
pub use ids::{ClipId, LabelId, LinkId, MediaId, SequenceId, TrackId};
pub use model::{
    AudioStream, Clip, Fingerprint, LabelColor, MediaInfo, MediaKind, MediaRef, Project, Region, Sequence,
    SequenceSettings, TimelineLabel, Track, TrackKind, VideoStream,
};
pub use time::{Rate, Time};

/// File extension for project files, without the dot.
pub const FILE_EXTENSION: &str = brand::PROJECT_FILE_EXTENSION;
