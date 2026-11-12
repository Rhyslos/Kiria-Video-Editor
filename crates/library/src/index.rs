//! The SQLite index of one project's media: tables, keeping them in step
//! with the project, and answering queries.
//!
//! The index is derived: everything in it comes from the project
//! ([`Index::sync`]), so it lives in memory and is built again whenever a
//! project opens. Nothing is lost when it goes, and it can never disagree
//! with the project file, its undo history or a recovered journal.

use crate::query::{Query, Sort, Type, Usage};
use diag::{AppError, Explain};
use project::{MediaId, MediaLabelId, MediaListId, MediaRef, Project, SequenceId};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The tables. `search` is the full-text index (trigram: any three letters
/// in a row match, so part of a word finds it, as the old filter did).
const SCHEMA: &str = "
PRAGMA foreign_keys = OFF;
CREATE TABLE types (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE media (
    id INTEGER PRIMARY KEY,         -- the project's MediaId
    size INTEGER NOT NULL,          -- fingerprint: file size
    hash TEXT NOT NULL,             -- fingerprint: hash of the start and end
    type INTEGER NOT NULL REFERENCES types(id),
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    duration REAL NOT NULL,         -- seconds, for sorting only
    note TEXT NOT NULL,
    words TEXT NOT NULL,            -- name, labels and note in lower case, for words under three letters
    clips INTEGER NOT NULL          -- clips on every timeline
);
CREATE INDEX media_type ON media(type);
CREATE INDEX media_fingerprint ON media(size, hash);
CREATE TABLE labels (id INTEGER PRIMARY KEY, name TEXT NOT NULL, colour TEXT NOT NULL, position INTEGER NOT NULL);
CREATE TABLE media_labels (media INTEGER NOT NULL, label INTEGER NOT NULL, PRIMARY KEY (label, media)) WITHOUT ROWID;
CREATE INDEX media_labels_media ON media_labels(media);
CREATE TABLE lists (id INTEGER PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL);
CREATE TABLE list_members (list INTEGER NOT NULL, media INTEGER NOT NULL, position INTEGER NOT NULL, PRIMARY KEY (list, media)) WITHOUT ROWID;
CREATE TABLE usage (media INTEGER NOT NULL, sequence INTEGER NOT NULL, clips INTEGER NOT NULL, PRIMARY KEY (media, sequence)) WITHOUT ROWID;
CREATE VIRTUAL TABLE search USING fts5(name, labels, note, tokenize = 'trigram');
";

/// One file as the index holds it; compared with the last sync to write
/// only what changed.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    size: u64,
    hash: String,
    kind: Type,
    name: String,
    path: String,
    duration: f64,
    note: String,
    labels: Vec<MediaLabelId>,
    label_names: String,
    usage: BTreeMap<SequenceId, u32>,
}

impl Row {
    fn clips(&self) -> u32 {
        self.usage.values().sum()
    }

    fn words(&self) -> String {
        format!("{}\n{}\n{}", self.name, self.label_names, self.note).to_lowercase()
    }
}

/// What a sync changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Synced {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

/// One project's media index.
#[derive(Debug)]
pub struct Index {
    conn: Connection,
    rows: HashMap<MediaId, Row>,
    /// Labels and lists as last written, to rewrite them only on change.
    labels: Vec<(MediaLabelId, String, String)>,
    lists: Vec<(MediaListId, String, Vec<MediaId>)>,
}

fn broken() -> AppError {
    AppError::new("The media search stopped working")
        .why("Its index, kept in memory, could not be read or written.")
        .fix("Close and open the project again. If this keeps happening, report it with the Diagnostics details.")
}

/// A file's name as the library shows it.
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl Index {
    /// An empty index in memory.
    pub fn new() -> diag::Result<Index> {
        let conn = Connection::open_in_memory().or_explain(broken)?;
        conn.execute_batch(SCHEMA).or_explain(broken)?;
        for t in Type::ALL {
            conn.execute(
                "INSERT INTO types (id, name) VALUES (?1, ?2)",
                params![t.code(), t.name()],
            )
            .or_explain(broken)?;
        }
        Ok(Index {
            conn,
            rows: HashMap::new(),
            labels: Vec::new(),
            lists: Vec::new(),
        })
    }

    /// How many files it holds.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Brings the index in step with `project`, writing only what changed,
    /// in one transaction.
    pub fn sync(&mut self, project: &Project) -> diag::Result<Synced> {
        let usage = usage(project);
        let library = &project.library;
        let label_names: HashMap<MediaLabelId, &str> =
            library.labels.iter().map(|l| (l.id, l.name.as_str())).collect();
        let mut wanted: HashMap<MediaId, Row> = HashMap::with_capacity(project.media.len());
        for (id, media) in &project.media {
            let labels: Vec<MediaLabelId> = library
                .tagged
                .get(id)
                .map(|on| on.iter().copied().collect())
                .unwrap_or_default();
            let names: Vec<&str> = labels
                .iter()
                .filter_map(|l| label_names.get(l).copied())
                .collect();
            wanted.insert(
                *id,
                row(media, library.note(*id), labels, names.join(" "), usage.get(id)),
            );
        }
        let labels: Vec<(MediaLabelId, String, String)> = library
            .labels
            .iter()
            .map(|l| (l.id, l.name.clone(), format!("{:?}", l.color)))
            .collect();
        let lists: Vec<(MediaListId, String, Vec<MediaId>)> = library
            .lists
            .iter()
            .map(|l| (l.id, l.name.clone(), l.media.iter().copied().collect()))
            .collect();

        let tx = self.conn.transaction().or_explain(broken)?;
        let mut synced = Synced::default();
        for id in self.rows.keys().filter(|id| !wanted.contains_key(id)) {
            delete(&tx, *id)?;
            synced.removed += 1;
        }
        for (id, row) in &wanted {
            match self.rows.get(id) {
                Some(old) if old == row => continue,
                Some(_) => {
                    delete(&tx, *id)?;
                    synced.changed += 1;
                }
                None => synced.added += 1,
            }
            insert(&tx, *id, row)?;
        }
        if labels != self.labels {
            tx.execute("DELETE FROM labels", []).or_explain(broken)?;
            let mut add = tx
                .prepare_cached("INSERT INTO labels (id, name, colour, position) VALUES (?1, ?2, ?3, ?4)")
                .or_explain(broken)?;
            for (position, (id, name, colour)) in labels.iter().enumerate() {
                add.execute(params![id.0 as i64, name, colour, position as i64])
                    .or_explain(broken)?;
            }
        }
        if lists != self.lists {
            tx.execute("DELETE FROM lists", []).or_explain(broken)?;
            tx.execute("DELETE FROM list_members", []).or_explain(broken)?;
            let mut add = tx
                .prepare_cached("INSERT INTO lists (id, name, position) VALUES (?1, ?2, ?3)")
                .or_explain(broken)?;
            let mut member = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO list_members (list, media, position) VALUES (?1, ?2, ?3)",
                )
                .or_explain(broken)?;
            for (position, (id, name, media)) in lists.iter().enumerate() {
                add.execute(params![id.0 as i64, name, position as i64])
                    .or_explain(broken)?;
                for (at, m) in media.iter().enumerate() {
                    member
                        .execute(params![id.0 as i64, m.0 as i64, at as i64])
                        .or_explain(broken)?;
                }
            }
        }
        tx.commit().or_explain(broken)?;
        self.rows = wanted;
        self.labels = labels;
        self.lists = lists;
        Ok(synced)
    }

    /// The files matching `query`, in its order.
    pub fn search(&self, query: &Query) -> diag::Result<Vec<MediaId>> {
        let mut sql = String::from("SELECT m.id FROM media m");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let in_list = query.list.filter(|_| query.sort == Sort::ListOrder);
        if let Some(list) = in_list {
            sql.push_str(" JOIN list_members lm ON lm.media = m.id AND lm.list = ?");
            args.push((list.0 as i64).into());
        }
        sql.push_str(" WHERE 1");
        let (long, short): (Vec<String>, Vec<String>) =
            query.words().into_iter().partition(|w| w.chars().count() >= 3);
        if !long.is_empty() {
            // Each word quoted, so nothing typed is read as search syntax.
            let phrase: Vec<String> = long
                .iter()
                .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
                .collect();
            sql.push_str(" AND m.id IN (SELECT rowid FROM search WHERE search MATCH ?)");
            args.push(phrase.join(" AND ").into());
        }
        for word in short {
            sql.push_str(" AND instr(m.words, ?) > 0");
            args.push(word.into());
        }
        if !query.types.is_empty() {
            let marks = vec!["?"; query.types.len()].join(", ");
            sql.push_str(&format!(" AND m.type IN ({marks})"));
            args.extend(query.types.iter().map(|t| t.code().into()));
        }
        for label in &query.labels {
            sql.push_str(" AND m.id IN (SELECT media FROM media_labels WHERE label = ?)");
            args.push((label.0 as i64).into());
        }
        if let Some(list) = query.list.filter(|_| in_list.is_none()) {
            sql.push_str(" AND m.id IN (SELECT media FROM list_members WHERE list = ?)");
            args.push((list.0 as i64).into());
        }
        match query.usage {
            Usage::Any => {}
            Usage::Used => sql.push_str(" AND m.clips > 0"),
            Usage::Unused => sql.push_str(" AND m.clips = 0"),
        }
        sql.push_str(match query.sort {
            Sort::Name => " ORDER BY m.name COLLATE NOCASE, m.id",
            Sort::Newest => " ORDER BY m.id DESC",
            Sort::Oldest => " ORDER BY m.id",
            Sort::Longest => " ORDER BY m.duration DESC, m.name COLLATE NOCASE, m.id",
            Sort::Shortest => " ORDER BY m.duration, m.name COLLATE NOCASE, m.id",
            Sort::Type => " ORDER BY m.type, m.name COLLATE NOCASE, m.id",
            Sort::MostUsed => " ORDER BY m.clips DESC, m.name COLLATE NOCASE, m.id",
            Sort::ListOrder if in_list.is_some() => " ORDER BY lm.position",
            Sort::ListOrder => " ORDER BY m.name COLLATE NOCASE, m.id",
        });
        let mut statement = self.conn.prepare_cached(&sql).or_explain(broken)?;
        let ids = statement
            .query_map(params_from_iter(args), |r| r.get::<_, i64>(0))
            .or_explain(broken)?
            .map(|id| id.map(|id| MediaId(id as u64)))
            .collect::<Result<Vec<_>, _>>()
            .or_explain(broken)?;
        Ok(ids)
    }

    /// How many clips use `media` on each timeline (timelines without any
    /// left out).
    pub fn usage(&self, media: MediaId) -> Vec<(SequenceId, u32)> {
        self.rows
            .get(&media)
            .map(|r| r.usage.iter().map(|(s, n)| (*s, *n)).collect())
            .unwrap_or_default()
    }

    /// The files in the project with this fingerprint (the same recording,
    /// imported more than once or under another name).
    pub fn with_fingerprint(&self, fingerprint: &project::Fingerprint) -> diag::Result<Vec<MediaId>> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT id FROM media WHERE size = ?1 AND hash = ?2 ORDER BY id")
            .or_explain(broken)?;
        let ids = statement
            .query_map(params![fingerprint.size as i64, fingerprint.sample_hash], |r| {
                r.get::<_, i64>(0)
            })
            .or_explain(broken)?
            .map(|id| id.map(|id| MediaId(id as u64)))
            .collect::<Result<Vec<_>, _>>()
            .or_explain(broken)?;
        Ok(ids)
    }
}

/// Clips per file per timeline: file clips only (nested timelines and
/// adjustment layers play no file).
fn usage(project: &Project) -> HashMap<MediaId, BTreeMap<SequenceId, u32>> {
    let mut usage: HashMap<MediaId, BTreeMap<SequenceId, u32>> = HashMap::new();
    for seq in &project.sequences {
        for clip in seq.tracks().flat_map(|t| t.clips.iter()).filter(|c| c.is_file()) {
            *usage.entry(clip.media).or_default().entry(seq.id).or_default() += 1;
        }
    }
    usage
}

fn row(
    media: &MediaRef,
    note: &str,
    labels: Vec<MediaLabelId>,
    label_names: String,
    usage: Option<&BTreeMap<SequenceId, u32>>,
) -> Row {
    Row {
        size: media.fingerprint.size,
        hash: media.fingerprint.sample_hash.clone(),
        kind: media.kind.into(),
        name: file_name(&media.path),
        path: media.path.display().to_string(),
        duration: media.duration.as_secs_f64(),
        note: note.to_owned(),
        labels,
        label_names,
        usage: usage.cloned().unwrap_or_default(),
    }
}

fn delete(tx: &rusqlite::Transaction<'_>, id: MediaId) -> diag::Result<()> {
    let id = id.0 as i64;
    for sql in [
        "DELETE FROM media WHERE id = ?1",
        "DELETE FROM media_labels WHERE media = ?1",
        "DELETE FROM usage WHERE media = ?1",
        "DELETE FROM search WHERE rowid = ?1",
    ] {
        tx.prepare_cached(sql)
            .and_then(|mut s| s.execute([id]))
            .or_explain(broken)?;
    }
    Ok(())
}

fn insert(tx: &rusqlite::Transaction<'_>, id: MediaId, row: &Row) -> diag::Result<()> {
    let key = id.0 as i64;
    tx.prepare_cached(
        "INSERT INTO media (id, size, hash, type, name, path, duration, note, words, clips)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .and_then(|mut s| {
        s.execute(params![
            key,
            row.size as i64,
            row.hash,
            row.kind.code(),
            row.name,
            row.path,
            row.duration,
            row.note,
            row.words(),
            row.clips()
        ])
    })
    .or_explain(broken)?;
    tx.prepare_cached("INSERT INTO search (rowid, name, labels, note) VALUES (?1, ?2, ?3, ?4)")
        .and_then(|mut s| s.execute(params![key, row.name, row.label_names, row.note]))
        .or_explain(broken)?;
    let mut label = tx
        .prepare_cached("INSERT INTO media_labels (media, label) VALUES (?1, ?2)")
        .or_explain(broken)?;
    for l in &row.labels {
        label.execute(params![key, l.0 as i64]).or_explain(broken)?;
    }
    let mut used = tx
        .prepare_cached("INSERT INTO usage (media, sequence, clips) VALUES (?1, ?2, ?3)")
        .or_explain(broken)?;
    for (seq, clips) in &row.usage {
        used.execute(params![key, seq.0 as i64, clips])
            .or_explain(broken)?;
    }
    Ok(())
}
