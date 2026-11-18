//! A file's loudness readings kept on disk (the Library's detail panel):
//! every sound stream's gated loudness and true peak, a few bytes a file,
//! in the cache folder's `loudness/` by the recording's fingerprint (as
//! proxies and waveforms are), so a file is measured once, not once a
//! session. The measuring itself is the Library page's.

use crate::naming;
use project::Fingerprint;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What was measured of a sound stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Gated loudness of the whole stream, LUFS (minus infinity for silence).
    pub integrated: f32,
    /// Its highest true peak, dBTP.
    pub true_peak: f32,
}

/// Readings kept on disk, one small text file per recording: a header,
/// then a line per sound stream (its number, loudness and true peak).
#[derive(Debug, Clone)]
pub struct Store {
    folder: PathBuf,
}

/// The first line of a kept file: what it is and its format's version.
const HEADER: &str = "loudness 1";

/// The ending of kept files.
const EXTENSION: &str = "txt";

impl Store {
    /// Readings kept in `folder` (made when the first is written).
    pub fn new(folder: PathBuf) -> Store {
        Store { folder }
    }

    /// Where this recording's readings are kept; `None` for a fingerprint
    /// the app does not make (nothing is kept for it).
    fn file_for(&self, fingerprint: &Fingerprint) -> Option<PathBuf> {
        naming::stem(fingerprint).map(|stem| self.folder.join(format!("{stem}.{EXTENSION}")))
    }

    /// The readings kept for `streams` of this recording, when there is
    /// one for every one of them.
    pub fn load(&self, fingerprint: &Fingerprint, streams: &[u32]) -> Option<Vec<(u32, Reading)>> {
        let text = std::fs::read_to_string(self.file_for(fingerprint)?).ok()?;
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut kept = HashMap::new();
        for line in lines {
            let mut parts = line.split_whitespace();
            let stream: u32 = parts.next()?.parse().ok()?;
            let integrated: f32 = parts.next()?.parse().ok()?;
            let true_peak: f32 = parts.next()?.parse().ok()?;
            kept.insert(
                stream,
                Reading {
                    integrated,
                    true_peak,
                },
            );
        }
        streams.iter().map(|s| Some((*s, *kept.get(s)?))).collect()
    }

    /// Keeps `readings` (by stream) of this recording, whole or not at
    /// all. A failure, or a fingerprint the app does not make, only means
    /// measuring again next session.
    pub fn save(&self, fingerprint: &Fingerprint, readings: &[(u32, Reading)]) {
        let Some(file) = self.file_for(fingerprint) else {
            return;
        };
        let mut text = format!("{HEADER}\n");
        for (stream, r) in readings {
            text.push_str(&format!("{stream} {} {}\n", r.integrated, r.true_peak));
        }
        let written = std::fs::create_dir_all(&self.folder)
            .and_then(|()| project::disk::write_atomic(&file, text.as_bytes()));
        if let Err(err) = written {
            tracing::info!("a file's loudness was not kept: {err}");
        }
    }

    /// Removes every reading kept (Settings › Cache); how many went. Only
    /// the files written here are removed, never the folder's other
    /// contents.
    pub fn clear(&self) -> usize {
        clear_folder(&self.folder)
    }
}

fn clear_folder(folder: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION) && path.is_file())
        .filter(|path| std::fs::remove_file(path).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{hostile, print};

    fn reading(integrated: f32, true_peak: f32) -> Reading {
        Reading {
            integrated,
            true_peak,
        }
    }

    #[test]
    fn readings_round_trip_by_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("loudness"));
        assert_eq!(store.load(&print(1), &[1]), None, "nothing kept yet");
        let readings = vec![
            (1, reading(-23.25, -3.5)),
            (2, reading(f32::NEG_INFINITY, f32::NEG_INFINITY)),
        ];
        store.save(&print(1), &readings);
        assert_eq!(store.load(&print(1), &[1, 2]), Some(readings.clone()));
        assert_eq!(
            store.load(&print(1), &[2]),
            Some(vec![readings[1]]),
            "some of the streams"
        );
        assert_eq!(store.load(&print(1), &[1, 3]), None, "a stream not measured");
        assert_eq!(store.load(&print(2), &[1]), None, "another recording");
        // The format and name earlier versions wrote are read the same.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("loudness/1-00000000000000000000000000000001.txt"))
                .unwrap(),
            "loudness 1\n1 -23.25 -3.5\n2 -inf -inf\n"
        );
    }

    #[test]
    fn clearing_removes_the_kept_readings_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("loudness");
        let store = Store::new(folder.clone());
        store.save(&print(1), &[(0, reading(-14.0, -1.0))]);
        let other = folder.join("not ours.dat");
        std::fs::write(&other, "x").unwrap();
        assert_eq!(store.clear(), 1);
        assert_eq!(store.load(&print(1), &[0]), None);
        assert!(other.exists(), "only readings are removed");
    }

    #[test]
    fn a_fingerprint_the_app_does_not_make_is_never_read_or_written() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("cache/loudness");
        let store = Store::new(folder.clone());
        // A file a crafted name would reach.
        std::fs::write(dir.path().join("escaped.txt"), "loudness 1\n0 -14 -1\n").unwrap();
        for fingerprint in hostile() {
            store.save(&fingerprint, &[(0, reading(-14.0, -1.0))]);
            assert_eq!(store.load(&fingerprint, &[0]), None, "{fingerprint:?}");
        }
        assert!(!folder.exists(), "nothing written");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "nothing beside the file that was there"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("escaped.txt")).unwrap(),
            "loudness 1\n0 -14 -1\n",
            "left as it was"
        );
    }
}
