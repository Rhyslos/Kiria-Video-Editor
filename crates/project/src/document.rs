//! An open project on disk: history, autosave journal and crash recovery.
//!
//! **The journal.** Every change, undo and redo is appended to a journal
//! file next to the project (`<file>.journal`) the moment it happens,
//! numbered in order, so a crash of the app loses nothing; a thread of its
//! own then asks the disk to keep it through a power cut too, so the
//! interface never waits for the disk (`journal_sync`). The journal holds
//! the whole session: it starts when the project is opened, from a copy of
//! the project file as it was then (`<file>.journal.start`), and full saves
//! never empty it. So the journal replayed onto its start rebuilds not only
//! the project but its undo and redo history too: an undo after a save
//! steps back over the save exactly as it did on screen.
//!
//! **Full saves** write the project file together with the number of the
//! last journal entry it includes. They run on a worker thread: the
//! interface hands out a [`SaveJob`] (a cheap snapshot), the job writes the
//! file, and [`Document::finish_save`] records the result. Several jobs may
//! be under way (an autosave and a save asked for); they write one at a
//! time, and a job whose snapshot is older than the file already written
//! leaves the file alone, so an older project never replaces a newer one.
//!
//! **Recovery.** [`Document::check`] counts the journal entries newer than
//! the project file. [`Document::open`] with [`Recovery::Recover`] replays
//! the whole journal onto its start, so the state at the crash comes back
//! with its history, and saves it; the session then goes on with the same
//! journal. With [`Recovery::OpenLastSave`] the journal is set aside under a
//! name of its own (never deleted, never replaced) and a new session starts
//! from the file.

use crate::command::Command;
use crate::disk;
use crate::file;
use crate::history::History;
use crate::journal_sync::JournalSync;
use crate::model::Project;
use diag::{AppError, Explain};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Changes older than this are written into the project file at the next [`Document::tick`].
pub const FULL_SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// One change as recorded in the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JournalEntry {
    /// Boxed: a command can be large (one carries a whole clip), and undo
    /// and redo are not. Written the same as the command itself.
    Do(Box<Command>),
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
    /// beside the project as `<file>.journal.discarded-<when>`, one file each
    /// time, never deleted.
    OpenLastSave,
}

/// What happened to unsaved work from a crash when the document was opened.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    /// Nothing was waiting: the project opened as saved.
    AsSaved,
    /// `changes` unsaved changes were put back ([`Recovery::Recover`]).
    /// `last_save` is the project as it was last saved before them, so the
    /// person can still go back to it (`Command::Restore`, one undo step);
    /// the recovered changes can be undone one by one too.
    Recovered { changes: usize, last_save: Box<Project> },
    /// `changes` unsaved changes were set aside ([`Recovery::OpenLastSave`])
    /// in the file `kept`, beside the project.
    SetAside { changes: usize, kept: PathBuf },
}

/// "1 change" or "3 changes".
pub fn changes_text(changes: usize) -> String {
    if changes == 1 {
        "1 change".to_owned()
    } else {
        format!("{changes} changes")
    }
}

/// Lets one full save of a document write at a time, and remembers the
/// newest journal entry written, so an older snapshot never replaces it.
#[derive(Debug)]
struct SaveGate {
    written: Mutex<u64>,
}

/// A full save, ready to run on a worker thread. Holds its own snapshot.
#[derive(Debug, Clone)]
pub struct SaveJob {
    project: Project,
    path: PathBuf,
    journal_seq: u64,
    gate: Arc<SaveGate>,
}

impl SaveJob {
    /// Writes the project file. Safe to run on any thread; saves of one
    /// document wait for each other here. A job whose snapshot is older
    /// than the one already written leaves the file alone (and succeeds:
    /// what it holds is on disk).
    pub fn run(&self) -> diag::Result<()> {
        let mut written = self.gate.written.lock().unwrap_or_else(|p| p.into_inner());
        if self.journal_seq < *written {
            return Ok(());
        }
        file::save_with_journal_seq(&self.project, &self.path, self.journal_seq)?;
        *written = self.journal_seq;
        Ok(())
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
    /// Puts what is written to the journal on the disk, off this thread.
    sync: JournalSync,
    /// Number of the last journal entry written.
    seq: u64,
    /// Number of the newest journal entry in the project file.
    saved_seq: u64,
    unsaved_since: Option<Instant>,
    /// Save jobs handed out and not yet finished.
    saves_in_flight: usize,
    gate: Arc<SaveGate>,
    opened: Opened,
}

impl Document {
    fn new(path: &Path, history: History, journal: Option<File>, seq: u64, saved_seq: u64) -> Document {
        Document {
            path: path.to_owned(),
            history,
            journal,
            sync: JournalSync::start(),
            seq,
            saved_seq,
            unsaved_since: None,
            saves_in_flight: 0,
            gate: Arc::new(SaveGate {
                written: Mutex::new(saved_seq),
            }),
            opened: Opened::AsSaved,
        }
    }

    /// Saves a new project at `path` and opens it.
    pub fn create(path: &Path, project: Project) -> diag::Result<Document> {
        file::save(&project, path)?;
        let journal = start_session(path)?;
        Ok(Document::new(path, History::new(project), Some(journal), 0, 0))
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
            let mut project = file::load_newer(path)?;
            file::resolve_media_paths(&mut project, file::folder_of(path));
            return Ok(Document::new(path, History::new(project), None, 0, 0));
        }
        if format < file::FORMAT_VERSION {
            file::back_up_before_upgrade(path, format)?;
        }
        let (project, saved_seq) = load_resolved(path)?;
        let journal_file = journal_path(path);
        let entries = read_journal(&journal_file)?;
        let changes = entries.iter().filter(|(seq, _)| *seq > saved_seq).count();
        if changes > 0 && recovery == Recovery::Recover {
            let last_save = Box::new(project.clone());
            let (history, seq) = recover(path, project, saved_seq, &entries)?;
            let journal = open_journal(&journal_file, false)?;
            let mut document = Document::new(path, history, Some(journal), seq, saved_seq);
            // Recovered work goes straight into the project file.
            document.save()?;
            document.opened = Opened::Recovered { changes, last_save };
            return Ok(document);
        }
        let kept = if changes > 0 { Some(set_aside(path)?) } else { None };
        let journal = start_session(path)?;
        let mut document = Document::new(path, History::new(project), Some(journal), saved_seq, saved_seq);
        if let Some(kept) = kept {
            document.opened = Opened::SetAside { changes, kept };
        }
        Ok(document)
    }

    pub fn project(&self) -> &Project {
        self.history.current()
    }

    /// What happened to unsaved work from a crash when this was opened.
    pub fn opened(&self) -> &Opened {
        &self.opened
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
        self.record(JournalEntry::Do(Box::new(command)))
    }

    /// Opens another timeline of the project: not an edit, so it works on a
    /// read-only project too, and nothing is written (see [`History::open`]).
    pub fn open_sequence(&mut self, sequence: crate::ids::SequenceId) -> bool {
        self.history.open(sequence)
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

    /// True while a full save handed out is not finished.
    pub fn is_saving(&self) -> bool {
        self.saves_in_flight > 0
    }

    /// Call regularly, with `idle = true` when nobody is using the app. Returns
    /// a save to run on a worker when changes are 30 seconds old or the app is
    /// idle, and no other save is running.
    pub fn tick(&mut self, now: Instant, idle: bool) -> Option<SaveJob> {
        let due = self
            .unsaved_since
            .is_some_and(|since| idle || now.duration_since(since) >= FULL_SAVE_INTERVAL);
        (due && !self.is_saving() && !self.is_read_only()).then(|| self.prepare_save())
    }

    /// Takes a snapshot for a full save, even while another save runs (they
    /// write one at a time, newest last). Run the job, then call
    /// [`Document::finish_save`].
    pub fn prepare_save(&mut self) -> SaveJob {
        self.saves_in_flight += 1;
        SaveJob {
            project: self.history.current().clone(),
            path: self.path.clone(),
            journal_seq: self.seq,
            gate: Arc::clone(&self.gate),
        }
    }

    /// Records how a [`SaveJob`] went. On success the project counts as
    /// saved if nothing changed after the newest save on disk; on failure
    /// the changes stay unsaved (they are in the journal).
    pub fn finish_save(&mut self, journal_seq: u64, result: diag::Result<()>) -> diag::Result<()> {
        self.saves_in_flight = self.saves_in_flight.saturating_sub(1);
        result?;
        self.saved_seq = self.saved_seq.max(journal_seq);
        if self.saved_seq >= self.seq {
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
        self.sync.request(journal).or_explain(explain)?;
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

/// A project file read, with its media paths made full, and the journal
/// entry it includes.
fn load_resolved(path: &Path) -> diag::Result<(Project, u64)> {
    let (mut project, seq) = file::load_with_journal_seq(path)?;
    // Relative media paths are from the project's folder (a package).
    file::resolve_media_paths(&mut project, file::folder_of(path));
    Ok((project, seq))
}

/// Starts a new journal: the project file as it is now becomes the
/// journal's start, then the journal is emptied (in that order, so a crash
/// between the two leaves an old journal whose entries the file already
/// holds, which nothing replays).
fn start_session(path: &Path) -> diag::Result<File> {
    let start = journal_start_path(path);
    disk::copy_atomic(path, &start).or_explain(|| {
        AppError::new("Autosave can't start")
            .why(format!("{} could not be written.", start.display()))
            .fix("Check that the disk is not full and that you can write to the project's folder.")
    })?;
    open_journal(&journal_path(path), true)
}

/// Rebuilds the project and its history at the crash: the whole journal
/// replayed onto its start. A journal from before the start was kept (or
/// one whose start is missing or doesn't match it) is replayed onto the
/// project file instead, as far as it goes, and the file becomes the start.
/// Either way the journal is written again with just the entries replayed,
/// so a half-written last line is gone before the session goes on adding
/// to it.
fn recover(
    path: &Path,
    saved: Project,
    saved_seq: u64,
    entries: &[(u64, JournalEntry)],
) -> diag::Result<(History, u64)> {
    let last = entries.last().map_or(saved_seq, |(seq, _)| *seq);
    let start = journal_start_path(path);
    let from_start = match load_resolved(&start) {
        Ok((project, start_seq)) if start_seq <= saved_seq && follows(entries, start_seq) => Some(project),
        Ok(_) => {
            tracing::warn!(
                "the autosave journal doesn't follow on from its start; recovering from the last save"
            );
            None
        }
        Err(err) => {
            if start.exists() {
                tracing::warn!("the autosave journal's start can't be read: {}", err.report());
            }
            None
        }
    };
    let explain = || {
        AppError::new("The recovered work can't be kept")
            .why(format!("Writing beside {} failed.", path.display()))
            .fix("Check that the disk is not full and that you can write to the project's folder.")
    };
    let (mut history, kept): (History, Vec<&(u64, JournalEntry)>) = match from_start {
        Some(project) => (History::new(project), entries.iter().collect()),
        None => {
            disk::copy_atomic(path, &start).or_explain(explain)?;
            (
                History::new(saved),
                entries.iter().filter(|(seq, _)| *seq > saved_seq).collect(),
            )
        }
    };
    replay(&mut history, kept.iter().map(|(_, entry)| entry));
    let mut text = String::new();
    for entry in kept {
        text.push_str(&ron::to_string(entry).or_explain(explain)?);
        text.push('\n');
    }
    disk::write_atomic(&journal_path(path), text.as_bytes()).or_explain(explain)?;
    Ok((history, last))
}

/// True when `entries` are exactly the ones after `start_seq`, one by one.
fn follows(entries: &[(u64, JournalEntry)], start_seq: u64) -> bool {
    entries
        .iter()
        .enumerate()
        .all(|(i, (seq, _))| *seq == start_seq + 1 + i as u64)
}

/// Moves the journal aside under a name of its own: never replaces one set
/// aside before.
fn set_aside(path: &Path) -> diag::Result<PathBuf> {
    let journal_file = journal_path(path);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let explain = || {
        AppError::new("The unsaved changes can't be set aside")
            .why(format!("Renaming {} failed.", journal_file.display()))
            .fix("Check that you can write to the project's folder.")
    };
    // A second later for each name taken (two set asides within a second).
    let kept = (0..10_000)
        .map(|later| discarded_journal_path(path, Some(stamp_of(now + later))))
        .find(|candidate| candidate.symlink_metadata().is_err())
        .ok_or_else(explain)?;
    fs::rename(&journal_file, &kept).or_explain(explain)?;
    Ok(kept)
}

/// The project's picture for the launcher: `<file>.thumbnail.png`.
pub fn thumbnail_path(project: &Path) -> PathBuf {
    with_suffix(project, ".thumbnail.png")
}

/// Where the editor keeps how a project's timeline was laid out when last
/// open (track heights and such): `<file>.view`. View state only, never
/// part of the project: deleting it loses nothing but the layout.
pub fn view_path(project: &Path) -> PathBuf {
    with_suffix(project, ".view")
}

/// The autosave journal: `<project file>.journal`.
pub fn journal_path(project: &Path) -> PathBuf {
    with_suffix(project, ".journal")
}

/// The project as it was when the journal started: `<project file>.journal.start`.
pub fn journal_start_path(project: &Path) -> PathBuf {
    with_suffix(project, ".journal.start")
}

/// Where unsaved work goes when a project is opened as last saved:
/// `<project file>.journal.discarded-<YYYYMMDD-HHMMSS>` (UTC), named by the
/// digits of `stamp` (see [`stamp_of`]). `None` is the single name earlier
/// versions used, `<project file>.journal.discarded`.
pub fn discarded_journal_path(project: &Path, stamp: Option<u64>) -> PathBuf {
    match stamp {
        Some(stamp) => with_suffix(
            project,
            &format!(
                ".journal.discarded-{:08}-{:06}",
                stamp / 1_000_000,
                stamp % 1_000_000
            ),
        ),
        None => with_suffix(project, ".journal.discarded"),
    }
}

/// The stamp in `candidate` when it is a set-aside journal of the project
/// file called `project_name`, by exactly the name [`discarded_journal_path`]
/// gives it.
pub fn discarded_stamp(candidate: &str, project_name: &str) -> Option<Option<u64>> {
    let rest = disk::strip_name(candidate, project_name)?.strip_prefix(".journal.discarded")?;
    if rest.is_empty() {
        return Some(None);
    }
    let (date, time) = rest.strip_prefix('-')?.split_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(date, 8) || !digits(time, 6) {
        return None;
    }
    let stamp: u64 = format!("{date}{time}").parse().ok()?;
    Some(Some(stamp))
}

/// `seconds` since 1970 as the number YYYYMMDDHHMMSS (UTC).
pub fn stamp_of(seconds: u64) -> u64 {
    let days = seconds / 86_400;
    let in_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    ((year * 100 + month) * 100 + day) * 1_000_000
        + (in_day / 3600) * 10_000
        + (in_day % 3600 / 60) * 100
        + in_day % 60
}

/// The calendar date `days` after 1 January 1970 (the proleptic Gregorian
/// calendar, counted in 400-year eras).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

fn with_suffix(project: &Path, suffix: &str) -> PathBuf {
    let mut name = project.as_os_str().to_owned();
    name.push(suffix);
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
    fn a_burst_of_changes_reaches_the_disk_without_waiting_for_it_each_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        for i in 0..100 {
            doc.apply(rename(&format!("Name {i}"))).unwrap();
        }
        // Each line is in the journal as soon as it is applied.
        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 100);
        let syncs = doc.sync.wait();
        assert!((1..=100).contains(&syncs), "{syncs}");
        let expected = doc.project().clone();
        drop(doc);
        assert_eq!(
            Document::open(&path, Recovery::Recover).unwrap().project(),
            &expected
        );
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
        assert_eq!(set_aside_journals(tmp.path()).len(), 1);
        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 0);
    }

    /// The text of every journal set aside in `dir`, by name.
    fn set_aside_journals(dir: &Path) -> Vec<String> {
        let mut found: Vec<(String, String)> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name().to_string_lossy().contains(".journal.discarded-"))
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read_to_string(e.path()).unwrap(),
                )
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, text)| text).collect()
    }

    #[test]
    fn every_open_last_save_keeps_its_own_set_aside_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("First lost work")).unwrap();
        drop(doc);
        let mut doc = Document::open(&path, Recovery::OpenLastSave).unwrap();
        doc.apply(rename("Second lost work")).unwrap();
        drop(doc);
        drop(Document::open(&path, Recovery::OpenLastSave).unwrap());

        let kept = set_aside_journals(tmp.path());
        assert_eq!(kept.len(), 2, "the first is not replaced");
        assert!(kept.iter().any(|t| t.contains("First lost work")));
        assert!(kept.iter().any(|t| t.contains("Second lost work")));
        // Both are the project's side files, so they move and go with it.
        let listed = crate::side_files::existing(&path);
        let discarded = listed
            .iter()
            .filter(|f| matches!(f.kind, crate::SideFile::DiscardedJournal(Some(_))))
            .count();
        assert_eq!(discarded, 2);
    }

    #[test]
    fn redos_after_an_autosave_and_a_later_edit_all_come_back() {
        // A tester's steps: three edits, undo all three, the idle autosave,
        // redo all three, one more edit, then the app is killed.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(add_track(&doc)).unwrap();
        doc.apply(rename("Renamed")).unwrap();
        doc.apply(add_track(&doc)).unwrap();
        for _ in 0..3 {
            assert!(doc.undo().unwrap());
        }
        let job = doc.tick(Instant::now(), true).expect("idle: saved");
        run(&mut doc, job);
        assert!(!doc.has_unsaved_changes());
        for _ in 0..3 {
            assert!(doc.redo().unwrap());
        }
        doc.apply(rename("Short cut")).unwrap();
        let expected = doc.project().clone();
        drop(doc); // kill -9

        assert_eq!(Document::check(&path).unwrap().unsaved_changes, 4);
        let mut doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project(), &expected);
        assert_eq!(audio_tracks(&doc), 3);
        let Opened::Recovered { changes, last_save } = doc.opened().clone() else {
            panic!("recovered");
        };
        assert_eq!(changes, 4);
        assert_eq!(last_save.name, "Start", "the last save is offered");
        // The recovered changes can be undone one by one...
        assert!(doc.undo().unwrap());
        assert_eq!(doc.project().name, "Renamed");
        assert!(doc.redo().unwrap());
        // ...or the last save put back in one step, which undo takes back.
        doc.apply(Command::Restore { project: last_save }).unwrap();
        assert_eq!(audio_tracks(&doc), 1);
        assert!(doc.undo().unwrap());
        assert_eq!(doc.project(), &expected);
    }

    #[test]
    fn opening_says_what_happened_to_unsaved_work() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("Lost?")).unwrap();
        drop(doc);
        let doc = Document::open(&path, Recovery::OpenLastSave).unwrap();
        let Opened::SetAside { changes, kept } = doc.opened().clone() else {
            panic!("set aside");
        };
        assert_eq!(changes, 1);
        assert!(fs::read_to_string(&kept).unwrap().contains("Lost?"));
        drop(doc);
        assert_eq!(
            Document::open(&path, Recovery::Recover).unwrap().opened(),
            &Opened::AsSaved
        );
        assert_eq!(changes_text(1), "1 change");
        assert_eq!(changes_text(4), "4 changes");
    }

    #[test]
    fn set_aside_names_are_dates() {
        assert_eq!(stamp_of(0), 19700101000000);
        // 2026-11-13 15:30:07 UTC.
        assert_eq!(stamp_of(1_794_583_807), 20261113153007);
        // A leap day.
        assert_eq!(stamp_of(951_782_400), 20000229000000);
        let path = Path::new("p.x");
        let named = discarded_journal_path(path, Some(20261113153007));
        assert_eq!(named, Path::new("p.x.journal.discarded-20261113-153007"));
        assert_eq!(
            discarded_stamp("p.x.journal.discarded-20261113-153007", "p.x"),
            Some(Some(20261113153007))
        );
        assert_eq!(discarded_stamp("p.x.journal.discarded", "p.x"), Some(None));
        assert_eq!(discarded_stamp("p.x.journal.discarded-2026-1113", "p.x"), None);
        assert_eq!(
            discarded_stamp("p.x.journal.discarded-20261113-15300", "p.x"),
            None
        );
        assert_eq!(
            discarded_stamp("q.x.journal.discarded-20261113-153007", "p.x"),
            None
        );
    }

    #[test]
    fn an_undo_or_redo_after_a_full_save_survives_a_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("A")).unwrap();
        doc.apply(rename("B")).unwrap();
        doc.save().unwrap();
        assert!(doc.undo().unwrap());
        assert_eq!(doc.project().name, "A");
        drop(doc); // crash

        let mut doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project().name, "A", "the undo after the save came back");
        assert!(doc.history().can_redo(), "with its history");
        // Again across the recovery's own save: redo, then undo twice.
        assert!(doc.redo().unwrap());
        doc.save().unwrap();
        assert!(doc.undo().unwrap());
        assert!(doc.undo().unwrap());
        assert_eq!(doc.project().name, "Start");
        drop(doc);

        let doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project().name, "Start");
        assert!(!doc.history().can_undo());
    }

    #[test]
    fn work_after_a_damaged_last_line_is_kept_through_the_next_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("Kept")).unwrap();
        drop(doc);
        let mut journal = OpenOptions::new().append(true).open(journal_path(&path)).unwrap();
        write!(journal, "(2, Do(RenamePro").unwrap();
        drop(journal);

        let mut doc = Document::open(&path, Recovery::Recover).unwrap();
        doc.apply(rename("After")).unwrap();
        drop(doc);
        assert_eq!(
            Document::open(&path, Recovery::Recover).unwrap().project().name,
            "After"
        );
    }

    #[test]
    fn a_journal_left_by_an_older_version_still_recovers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("One")).unwrap();
        doc.save().unwrap();
        doc.apply(rename("Two")).unwrap();
        drop(doc);
        // Older versions kept no start, and emptied the journal on a save.
        fs::remove_file(journal_start_path(&path)).unwrap();
        let text = ron::to_string(&(2u64, JournalEntry::Do(Box::new(rename("Two"))))).unwrap();
        fs::write(journal_path(&path), format!("{text}\n")).unwrap();

        let mut doc = Document::open(&path, Recovery::Recover).unwrap();
        assert_eq!(doc.project().name, "Two");
        // The session is whole again: a later crash recovers too.
        doc.undo().unwrap();
        drop(doc);
        assert_eq!(
            Document::open(&path, Recovery::Recover).unwrap().project().name,
            "One"
        );
    }

    #[test]
    fn an_older_save_finishing_late_never_replaces_a_newer_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        doc.apply(rename("B")).unwrap();
        let autosave = doc.prepare_save(); // held back, say while playing
        doc.apply(rename("C")).unwrap();
        let asked = doc.prepare_save(); // Ctrl+S
        assert!(doc.is_saving());
        run(&mut doc, asked);
        assert!(doc.is_saving(), "the autosave is still out");
        run(&mut doc, autosave);
        assert!(!doc.is_saving());
        assert!(!doc.has_unsaved_changes());
        assert_eq!(file::load(&path).unwrap().name, "C");
        drop(doc); // crash
        assert_eq!(
            Document::open(&path, Recovery::Recover).unwrap().project().name,
            "C"
        );
    }

    #[test]
    fn saves_of_one_document_write_one_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.project");
        let mut doc = new_doc(tmp.path());
        let mut jobs = Vec::new();
        for i in 0..12 {
            doc.apply(rename(&format!("Name {i}"))).unwrap();
            jobs.push(doc.prepare_save());
        }
        let results: Vec<(u64, diag::Result<()>)> = std::thread::scope(|scope| {
            let running: Vec<_> = jobs
                .iter()
                .rev()
                .map(|job| scope.spawn(move || (job.journal_seq(), job.run())))
                .collect();
            running.into_iter().map(|t| t.join().unwrap()).collect()
        });
        for (seq, result) in results {
            doc.finish_save(seq, result).unwrap();
        }
        assert_eq!(
            file::load(&path).unwrap().name,
            "Name 11",
            "the newest is on disk"
        );
        assert!(!doc.has_unsaved_changes());
        let leftovers = fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".saving")
            })
            .count();
        assert_eq!(leftovers, 0);
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
