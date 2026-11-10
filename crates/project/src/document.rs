//! An open project on disk: history, autosave journal and crash recovery.
//!
//! Every change, undo and redo is appended to a journal file next to the
//! project (`<file>.journal`) the moment it happens. A full save (every 30
//! seconds of changes, when the app goes idle, or on request) writes the
//! project file and empties the journal. After a crash the journal holds
//! exactly the work since the last full save, and [`Document::open`] can
//! replay it.

use crate::command::Command;
use crate::file;
use crate::history::History;
use crate::model::Project;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Changes older than this are written into the project file at the next [`Document::tick`].
pub const FULL_SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// One line of the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JournalEntry {
    Do(Command),
    Undo,
    Redo,
}

/// What [`Document::check`] found before opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCheck {
    /// Changes made after the last full save that a crash left behind.
    pub unsaved_changes: usize,
}

/// How to open a project that has unsaved changes from a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Replay the unsaved changes ("Recover").
    Recover,
    /// Open the project as last saved ("Open last save"). The journal is kept
    /// beside the project as `<file>.journal.discarded`, never deleted.
    OpenLastSave,
}

/// An open project.
#[derive(Debug)]
pub struct Document {
    path: PathBuf,
    history: History,
    journal: File,
    unsaved_since: Option<Instant>,
}

impl Document {
    /// Saves a new project at `path` and opens it.
    pub fn create(path: &Path, project: Project) -> diag::Result<Document> {
        file::save(&project, path)?;
        let journal = open_journal(&journal_path(path), true)?;
        Ok(Document {
            path: path.to_owned(),
            history: History::new(project),
            journal,
            unsaved_since: None,
        })
    }

    /// Looks for unsaved changes left by a crash, without opening anything.
    pub fn check(path: &Path) -> diag::Result<OpenCheck> {
        Ok(OpenCheck {
            unsaved_changes: read_journal(&journal_path(path))?.len(),
        })
    }

    /// Opens the project at `path`. `recovery` only matters when [`Document::check`]
    /// found unsaved changes.
    pub fn open(path: &Path, recovery: Recovery) -> diag::Result<Document> {
        let project = file::load(path)?;
        let journal_file = journal_path(path);
        let entries = read_journal(&journal_file)?;
        let mut history = History::new(project);
        if !entries.is_empty() {
            match recovery {
                Recovery::Recover => replay(&mut history, &entries),
                Recovery::OpenLastSave => {
                    let kept = journal_file.with_extension("journal.discarded");
                    fs::rename(&journal_file, &kept).or_explain(|| {
                        AppError::new("The unsaved changes can't be set aside")
                            .why(format!("Renaming {} failed.", journal_file.display()))
                            .fix("Check that you can write to the project's folder.")
                    })?;
                }
            }
        }
        let mut document = Document {
            path: path.to_owned(),
            history,
            journal: open_journal(&journal_file, false)?,
            unsaved_since: None,
        };
        if !entries.is_empty() && recovery == Recovery::Recover {
            // Recovered work goes straight into the project file.
            document.save()?;
        }
        Ok(document)
    }

    pub fn project(&self) -> &Project {
        self.history.current()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Applies a change and records it in the journal.
    pub fn apply(&mut self, command: Command) -> diag::Result<()> {
        self.history.apply(&command)?;
        self.record(JournalEntry::Do(command))
    }

    /// Undoes the last change. Returns false when there was nothing to undo.
    pub fn undo(&mut self) -> diag::Result<bool> {
        if !self.history.undo() {
            return Ok(false);
        }
        self.record(JournalEntry::Undo)?;
        Ok(true)
    }

    /// Redoes the last undone change. Returns false when there was nothing to redo.
    pub fn redo(&mut self) -> diag::Result<bool> {
        if !self.history.redo() {
            return Ok(false);
        }
        self.record(JournalEntry::Redo)?;
        Ok(true)
    }

    /// True when there are changes not yet in the project file (they are in the journal).
    pub fn has_unsaved_changes(&self) -> bool {
        self.unsaved_since.is_some()
    }

    /// Call regularly (and when the app goes idle with `idle = true`):
    /// does a full save when changes are old enough. Returns true if it saved.
    pub fn tick(&mut self, now: Instant, idle: bool) -> diag::Result<bool> {
        let due = self
            .unsaved_since
            .is_some_and(|since| idle || now.duration_since(since) >= FULL_SAVE_INTERVAL);
        if due {
            self.save()?;
        }
        Ok(due)
    }

    /// Writes the project file now and empties the journal.
    pub fn save(&mut self) -> diag::Result<()> {
        file::save(self.history.current(), &self.path)?;
        self.journal = open_journal(&journal_path(&self.path), true)?;
        self.unsaved_since = None;
        Ok(())
    }

    fn record(&mut self, entry: JournalEntry) -> diag::Result<()> {
        let line = ron::to_string(&entry).or_explain(|| {
            AppError::new("A change could not be written to the autosave journal")
                .why("It could not be turned into text.")
                .fix("Save the project now; this is a bug worth reporting.")
        })?;
        let explain = || {
            AppError::new("Autosave can't keep up")
                .why("Writing to the autosave journal failed.")
                .fix("Check that the disk is not full, then save the project.")
        };
        writeln!(self.journal, "{line}").or_explain(explain)?;
        self.journal.sync_data().or_explain(explain)?;
        self.unsaved_since.get_or_insert_with(Instant::now);
        Ok(())
    }
}

/// `<project file>.journal`.
pub fn journal_path(project: &Path) -> PathBuf {
    let mut name = project.as_os_str().to_owned();
    name.push(".journal");
    PathBuf::from(name)
}

/// Replays journal entries. Entries that no longer apply are skipped, so a
/// damaged journal recovers as much as it can.
fn replay(history: &mut History, entries: &[JournalEntry]) {
    for entry in entries {
        match entry {
            JournalEntry::Do(command) => {
                if let Err(err) = history.apply(command) {
                    tracing::warn!("skipped a journal entry during recovery: {}", err.report());
                }
            }
            JournalEntry::Undo => {
                history.undo();
            }
            JournalEntry::Redo => {
                history.redo();
            }
        }
    }
}

/// Reads every complete entry. A crash can leave a half-written last line; it is ignored.
fn read_journal(path: &Path) -> diag::Result<Vec<JournalEntry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(AppError::new("The autosave journal can't be read")
                .why(format!("{} could not be opened.", path.display()))
                .fix("Check that you can read the project's folder.")
                .source(err));
        }
    };
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        match ron::from_str(&line) {
            Ok(entry) => entries.push(entry),
            Err(_) => {
                tracing::warn!("ignored an unreadable autosave journal line");
                break;
            }
        }
    }
    Ok(entries)
}

fn open_journal(path: &Path, truncate: bool) -> diag::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(!truncate)
        .write(true)
        .truncate(truncate)
        .open(path)
        .or_explain(|| {
            AppError::new("Autosave can't start")
                .why(format!("{} could not be opened for writing.", path.display()))
                .fix("Check that the disk is not full and that you can write to the project's folder.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SequenceSettings;

    fn rename(name: &str) -> Command {
        Command::RenameProject { name: name.into() }
    }

    fn new_doc(dir: &Path) -> Document {
        Document::create(
            &dir.join("p.project"),
            Project::new("Start", SequenceSettings::GAME_CLIP),
        )
        .unwrap()
    }

    #[test]
    fn work_survives_a_crash_and_is_recovered() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("One")).unwrap();
        doc.apply(rename("Two")).unwrap();
        doc.undo().unwrap();
        doc.apply(rename("Three")).unwrap();
        let expected = doc.project().clone();
        drop(doc); // the "crash": no save

        assert_eq!(
            file::load(&path).unwrap().name,
            "Start",
            "nothing reached the project file"
        );
        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 4);

        let recovered = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(recovered.project(), &expected);
        assert!(recovered.history().can_undo(), "undo still works after recovery");
        assert_eq!(
            Document::check(&path).unwrap().unsaved_changes,
            0,
            "recovered work was saved"
        );
    }

    #[test]
    fn open_last_save_keeps_the_journal_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("Lost?")).unwrap();
        drop(doc);

        let doc = Document::open(&path, Recovery::OpenLastSave).unwrap();
        assert_eq!(doc.project().name, "Start");
        assert!(tmp.path().join("p.project.journal.discarded").exists());
        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 0);
    }

    #[test]
    fn a_half_written_last_line_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("Kept")).unwrap();
        drop(doc);
        let mut journal = OpenOptions::new().append(true).open(journal_path(&path)).unwrap();
        write!(journal, "Do(RenamePro").unwrap();

        let doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project().name, "Kept");
    }

    #[test]
    fn full_saves_happen_after_the_interval_or_when_idle() {
        let tmp = tempfile::tempdir().unwrap();
        let mut doc = new_doc(tmp.path());
        let start = Instant::now();
        assert!(!doc.tick(start, true).unwrap(), "nothing to save yet");

        doc.apply(rename("Changed")).unwrap();
        assert!(doc.has_unsaved_changes());
        assert!(!doc.tick(Instant::now(), false).unwrap(), "too soon");
        assert!(doc.tick(Instant::now() + FULL_SAVE_INTERVAL, false).unwrap());
        assert!(!doc.has_unsaved_changes());

        doc.apply(rename("Again")).unwrap();
        assert!(
            doc.tick(Instant::now(), true).unwrap(),
            "idle saves straight away"
        );
        assert_eq!(file::load(doc.path()).unwrap().name, "Again");
    }
}
