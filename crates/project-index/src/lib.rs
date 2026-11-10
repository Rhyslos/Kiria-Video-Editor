//! The launcher's list of known projects and folders.
//!
//! A small SQLite file in the app's data folder remembers every project the
//! person has created, opened, or that sits in one of their project folders,
//! with what a project card shows: name, length, settings, when it was last
//! opened, and any issue. Building the list never opens a whole project: only
//! the summary at the top of each file is read ([`project::file::read_info`]).
//!
//! The interface reads the list with [`Index::projects`], which is a single
//! quick query. Looking through folders and files is done by [`Index::refresh`]
//! on a worker thread with its own connection.

mod entry;
mod scan;
mod store;

pub use entry::{Folder, FolderKind, Issue, ProjectEntry};
pub use scan::find_project_files;
pub use store::{Index, now};

/// The index file's name inside the app's data folder.
pub const INDEX_FILE_NAME: &str = "projects.sqlite";
