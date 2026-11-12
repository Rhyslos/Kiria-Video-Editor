//! The media index and search.
//!
//! - [`index`]: one project's media in SQLite (in memory): types, labels,
//!   custom lists, notes, use on timelines and full-text search, kept in
//!   step with the project and answering [`Query`]s.
//! - [`query`]: what a search asks for: words, types, labels, a list, use, order.
//! - [`live`]: the index on a thread of its own, so the interface never waits.
//! - [`watch`]: media folders watched for new recordings, which count once
//!   they stop growing.
//!
//! The project is the source of truth: labels, lists and notes are project
//! data (`project::Library`), changed by undoable commands. The index is
//! built from it and can always be built again.

pub mod index;
pub mod live;
pub mod query;
pub mod watch;

pub use index::{Index, Synced, file_name};
pub use live::{Answer, Asker, Live};
pub use query::{Query, Sort, Type, Usage};
pub use watch::{Found, Watcher};
