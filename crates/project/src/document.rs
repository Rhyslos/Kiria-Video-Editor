//! An open project on disk: history, autosave journal and crash recovery.
//!
//! Every change, undo and redo is appended to a journal file next to the
//! project (`<file>.journal`) the moment it happens, numbered in order. A
//! full save writes the project file together with the number of the last
//! journal entry it includes. Full saves run on a worker thread: the
//! interface hands out a [`SaveJob`] (a cheap snapshot), the job writes the
//! file, and [`Document::finish_save`] records the result. After a crash,
//! [`Document::open`] replays only the journal entries newer than the file,
//! so nothing is lost and nothing is applied twice.

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

/// One change as recorded in the journal.
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

/// A full save, ready to run on a worker thread. Holds its own snapshot.
#[derive(Debug, Clone)]
pub struct SaveJob {
    project: Project,
    path: PathBuf,
    journal_seq: u64,
}

impl SaveJob {
    /// Writes the project file. Safe to run on any thread.
    pub fn run(&self) -> diag::Result<()> {
        file::save_with_journal_seq(&self.project, &self.path, self.journal_seq)
    }

    /// The journal entry this save includes; pass it to [`Document::finish_save`].
    pub fn journal_seq(&self) -> u64 {
        self.journal_seq
    }
}

/// An open project.
#[derive(Debug)]
pub struct Document {
    path: PathBuf,
    history: History,
    /// `None` for a read-only document: nothing is written.
    journal: Option<File>,
    /// Number of the last journal entry written.
    seq: u64,
    unsaved_since: Option<Instant>,
    save_in_flight: bool,
}

impl Document {
    /// Saves a new project at `path` and opens it.
    pub fn create(path: &Path, project: Project) -> diag::Result<Document> {
        file::save(&project, path)?;
        let journal = open_journal(&journal_path(path), true)?;
        Ok(Document {
            path: path.to_owned(),
            history: History::new(project),
            journal: Some(journal),
            seq: 0,
            unsaved_since: None,
            save_in_flight: false,
        })
    }

    /// Looks for unsaved changes left by a crash, without opening anything.
    pub fn check(path: &Path) -> diag::Result<OpenCheck> {
        let saved = file::read_journal_seq(path)?;
        let entries = read_journal(&journal_path(path))?;
        Ok(OpenCheck {
            unsaved_changes: entries.iter().filter(|(seq, _)| *seq > saved).count(),
        })
    }

    /// Opens the project at `path`. `recovery` only matters when [`Document::check`]
    /// found unsaved changes.
    ///
    /// A file from an older format is copied to `<file>.backup-v<old>` before
    /// anything is written to it. A file saved by a newer version opens
    /// read-only ([`Document::is_read_only`]) when this version can show it.
    pub fn open(path: &Path, recovery: Recovery) -> diag::Result<Document> {
        let format = file::read_info(path)?.format_version;
        if format > file::FORMAT_VERSION {
            return Ok(Document {
                path: path.to_owned(),
                history: History::new(file::load_newer(path)?),
                journal: None,
                seq: 0,
                unsaved_since: None,
                save_in_flight: false,
            });
        }
        if format < file::FORMAT_VERSION {
            file::back_up_before_upgrade(path, format)?;
        }
        let (project, saved_seq) = file::load_with_journal_seq(path)?;
        let journal_file = journal_path(path);
        let entries: Vec<(u64, JournalEntry)> = read_journal(&journal_file)?
            .into_iter()
            .filter(|(seq, _)| *seq > saved_seq)
            .collect();
        let mut history = History::new(project);
        let mut seq = saved_seq;
        let recovering = !entries.is_empty() && recovery == Recovery::Recover;
        if recovering {
            replay(&mut history, entries.iter().map(|(_, entry)| entry));
            seq = entries.last().map_or(seq, |(last, _)| *last);
        } else if !entries.is_empty() {
            let kept = journal_file.with_extension("journal.discarded");
            fs::rename(&journal_file, &kept).or_explain(|| {
                AppError::new("The unsaved changes can't be set aside")
                    .why(format!("Renaming {} failed.", journal_file.display()))
                    .fix("Check that you can write to the project's folder.")
            })?;
        }
        let mut document = Document {
            path: path.to_owned(),
            history,
            journal: Some(open_journal(&journal_file, false)?),
            seq,
            unsaved_since: None,
            save_in_flight: false,
        };
        if recovering {
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

    /// True for a project saved by a newer version: it can be looked at, not changed.
    pub fn is_read_only(&self) -> bool {
        self.journal.is_none()
    }

    fn refuse_if_read_only(&self) -> diag::Result<()> {
        if self.is_read_only() {
            return Err(read_only());
        }
        Ok(())
    }

    /// Applies a change and records it in the journal.
    pub fn apply(&mut self, command: Command) -> diag::Result<()> {
        self.refuse_if_read_only()?;
        self.history.apply(&command)?;
        self.record(JournalEntry::Do(command))
    }

    /// Undoes the last change. Returns false when there was nothing to undo.
    pub fn undo(&mut self) -> diag::Result<bool> {
        self.refuse_if_read_only()?;
        if !self.history.undo() {
            return Ok(false);
        }
        self.record(JournalEntry::Undo)?;
        Ok(true)
    }

    /// Redoes the last undone change. Returns false when there was nothing to redo.
    pub fn redo(&mut self) -> diag::Result<bool> {
        self.refuse_if_read_only()?;
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

    /// Call regularly, with `idle = true` when nobody is using the app. Returns
    /// a save to run on a worker when changes are 30 seconds old or the app is
    /// idle, and no other save is running.
    pub fn tick(&mut self, now: Instant, idle: bool) -> Option<SaveJob> {
        let due = self
            .unsaved_since
            .is_some_and(|since| idle || now.duration_since(since) >= FULL_SAVE_INTERVAL);
        (due && !self.save_in_flight && !self.is_read_only()).then(|| self.prepare_save())
    }

    /// Takes a snapshot for a full save. Run the job, then call [`Document::finish_save`].
    pub fn prepare_save(&mut self) -> SaveJob {
        self.save_in_flight = true;
        SaveJob {
            project: self.history.current().clone(),
            path: self.path.clone(),
            journal_seq: self.seq,
        }
    }

    /// Records how a [`SaveJob`] went. On success the journal is emptied if
    /// nothing changed meanwhile; on failure the changes stay in the journal.
    pub fn finish_save(&mut self, journal_seq: u64, result: diag::Result<()>) -> diag::Result<()> {
        self.save_in_flight = false;
        result?;
        if self.seq == journal_seq {
            if self.journal.is_some() {
                self.journal = Some(open_journal(&journal_path(&self.path), true)?);
            }
            self.unsaved_since = None;
        } else {
            // Newer changes arrived during the save; they wait for the next one.
            self.unsaved_since = Some(Instant::now());
        }
        Ok(())
    }

    /// Writes the project file now, on this thread.
    pub fn save(&mut self) -> diag::Result<()> {
        self.refuse_if_read_only()?;
        let job = self.prepare_save();
        let result = job.run();
        self.finish_save(job.journal_seq, result)
    }

    fn record(&mut self, entry: JournalEntry) -> diag::Result<()> {
        let seq = self.seq + 1;
        let line = ron::to_string(&(seq, entry)).or_explain(|| {
            AppError::new("A change could not be written to the autosave journal")
                .why("It could not be turned into text.")
                .fix("Save the project now; this is a bug worth reporting.")
        })?;
        let explain = || {
            AppError::new("Autosave can't keep up")
                .why("Writing to the autosave journal failed.")
                .fix("Check that the disk is not full, then save the project.")
        };
        let journal = self.journal.as_mut().ok_or_else(read_only)?;
        writeln!(journal, "{line}").or_explain(explain)?;
        journal.sync_data().or_explain(explain)?;
        self.seq = seq;
        self.unsaved_since.get_or_insert_with(Instant::now);
        Ok(())
    }
}

fn read_only() -> AppError {
    AppError::new("This project is open read-only")
        .why("It was saved by a newer version of the app, so this version can show it but not change it.")
        .fix("Update the app to edit it.")
}

/// `<project file>.journal`.
/// The project's picture for the launcher: `<file>.thumbnail.png`.
pub fn thumbnail_path(project: &Path) -> PathBuf {
    let mut name = project.as_os_str().to_owned();
    name.push(".thumbnail.png");
    PathBuf::from(name)
}

pub fn journal_path(project: &Path) -> PathBuf {
    let mut name = project.as_os_str().to_owned();
    name.push(".journal");
    PathBuf::from(name)
}

/// Replays journal entries. Entries that no longer apply are skipped, so a
/// damaged journal recovers as much as it can.
fn replay<'a>(history: &mut History, entries: impl Iterator<Item = &'a JournalEntry>) {
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

/// Reads every complete, numbered entry. A crash can leave a half-written
/// last line; it is ignored.
fn read_journal(path: &Path) -> diag::Result<Vec<(u64, JournalEntry)>> {
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
    #[test]
    fn a_newer_project_opens_read_only_and_is_never_written() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let text = file::to_string(&Project::new("Future", crate::SequenceSettings::GAME_CLIP))
            .unwrap()
            .replacen(
                &format!("format_version: {}", file::FORMAT_VERSION),
                &format!("format_version: {}", file::FORMAT_VERSION + 1),
                1,
            );
        fs::write(&path, &text).unwrap();
        let mut doc = Document::open(&path, Recovery::Recover).unwrap();
        assert!(doc.is_read_only());
        assert_eq!(doc.project().name, "Future");
        let sequence = doc.project().active_sequence;
        let err = doc
            .apply(Command::SetEditNeedle {
                sequence,
                at: crate::Time::ZERO,
            })
            .unwrap_err();
        assert!(err.what().contains("read-only"));
        assert!(doc.save().is_err());
        assert!(doc.tick(Instant::now() + FULL_SAVE_INTERVAL * 2, true).is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "the file is untouched");
        assert!(!journal_path(&path).exists(), "no journal is made");
    }

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
        write!(journal, "(2, Do(RenamePro").unwrap();

        let doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project().name, "Kept");
    }

    fn run(doc: &mut Document, job: SaveJob) {
        let result = job.run();
        doc.finish_save(job.journal_seq(), result).unwrap();
    }

    #[test]
    fn full_saves_happen_after_the_interval_or_when_idle() {
        let tmp = tempfile::tempdir().unwrap();
        let mut doc = new_doc(tmp.path());
        assert!(doc.tick(Instant::now(), true).is_none(), "nothing to save yet");

        doc.apply(rename("Changed")).unwrap();
        assert!(doc.has_unsaved_changes());
        assert!(doc.tick(Instant::now(), false).is_none(), "too soon");
        let job = doc.tick(Instant::now() + FULL_SAVE_INTERVAL, false).unwrap();
        assert!(doc.tick(Instant::now(), true).is_none(), "one save at a time");
        run(&mut doc, job);
        assert!(!doc.has_unsaved_changes());

        doc.apply(rename("Again")).unwrap();
        let job = doc.tick(Instant::now(), true).expect("idle saves straight away");
        run(&mut doc, job);
        assert_eq!(file::load(doc.path()).unwrap().name, "Again");
    }

    fn audio_tracks(doc: &Document) -> usize {
        doc.project().active().unwrap().audio_tracks.len()
    }

    fn add_track(doc: &Document) -> Command {
        Command::AddTrack {
            sequence: doc.project().active_sequence,
            kind: crate::model::TrackKind::Audio,
            name: "A".into(),
        }
    }

    #[test]
    fn changes_during_a_background_save_are_kept_and_never_applied_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(add_track(&doc)).unwrap(); // 2 audio tracks
        let job = doc.prepare_save();
        doc.apply(add_track(&doc)).unwrap(); // 3, made while the save runs
        run(&mut doc, job);
        assert!(doc.has_unsaved_changes(), "the newer change still needs saving");
        drop(doc); // crash

        let doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(audio_tracks(&doc), 3);
    }

    #[test]
    fn a_crash_right_after_a_save_does_not_replay_saved_work() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(add_track(&doc)).unwrap();
        doc.apply(add_track(&doc)).unwrap(); // 3 audio tracks
        let job = doc.prepare_save();
        job.run().unwrap();
        drop(doc); // crash before the journal was emptied

        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 0);
        let doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(audio_tracks(&doc), 3);
    }
}
