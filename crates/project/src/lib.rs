//! The project model, edit commands, undo/redo and autosave.
//!
//! - [`time`]: exact time and frame rates (fractions, never floats).
//! - [`ids`]: one id type per kind of thing.
//! - [`look`]: how a clip's picture is placed, cut and coloured.
//! - [`keyframes`]: how a clip's look changes over time.
//! - [`ramp`]: how a clip's speed changes over time (speed ramps).
//! - [`sound`]: how loud a clip is, its pan and its fades.
//! - [`library`]: labels, custom lists and notes on the project's media.
//! - [`global_labels`]: label names and colours offered in every project.
//! - [`model`]: projects, timelines, tracks, clips, media, labels, the region.
//! - [`file`]: the versioned project file, atomic saves and migrations.
//! - [`animate`]: setting a clip's look at a moment, keyframes added, removed and reset.
//! - [`command`]: every change is a `Command` applied to a snapshot.
//! - [`tracks`]: adding, removing, renaming and hiding tracks.
//! - `layers`: laying a file over a track and moving a clip between tracks, with sound that finds a free place.
//! - [`history`]: unlimited undo and redo over snapshots.
//! - [`document`]: an open project on disk, with the autosave journal and crash recovery.
//! - [`versions`]: named copies of a project to go back to.
//! - [`side_files`]: the files that belong to a project besides the project file.
//! - [`templates`]: stretches of a timeline saved to drop into any project.

pub mod animate;
pub mod clipboard;
pub mod command;
pub mod document;
mod edits;
pub mod file;
pub mod global_labels;
pub mod history;
pub mod ids;
pub mod keyframes;
mod layers;
pub mod library;
pub mod look;
pub mod model;
pub mod ramp;
pub mod side_files;
pub mod sound;
pub mod templates;
pub mod time;
pub mod timelines;
pub mod tracks;
pub mod versions;

pub use clipboard::CopiedKeyframes;
pub use command::{Command, Edge};
pub use document::{Document, OpenCheck, Recovery, thumbnail_path, view_path};
pub use global_labels::{GlobalLabel, GlobalLabels};
pub use history::History;
pub use ids::{ClipId, LabelId, LinkId, MediaId, MediaLabelId, MediaListId, SequenceId, TrackId};
pub use keyframes::{Ease, Keyframe, Keyframes, LookField};
pub use library::{Library, MediaLabel, MediaList};
pub use look::Look;
pub use model::FrameBlend;
pub use model::{
    AudioStream, Clip, ClipKind, Fingerprint, LabelColor, MediaInfo, MediaKind, MediaRef, Project, Region,
    Sequence, SequenceSettings, TimelineLabel, Track, TrackKind, VideoStream,
};
pub use ramp::{Ramp, SpeedKey};
pub use side_files::SideFile;
pub use sound::Sound;
pub use templates::Template;
pub use time::{Rate, Speed, Time};

/// File extension for project files, without the dot.
pub const FILE_EXTENSION: &str = brand::PROJECT_FILE_EXTENSION;
