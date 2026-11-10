//! Recognising a file by its contents.
//!
//! Hashing a whole multi-gigabyte recording takes too long, so the
//! fingerprint is the file size plus a hash of its first and last megabyte.
//! That is enough to tell recordings apart and to recognise one that was
//! moved or renamed.

use diag::{AppError, Explain};
use project::Fingerprint;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of each end of the file is hashed.
const SAMPLE: u64 = 1024 * 1024;

pub fn fingerprint(path: &Path) -> diag::Result<Fingerprint> {
    let cant_read = || {
        AppError::new("The file can't be read")
            .why(format!("Reading {} failed.", path.display()))
            .fix("Check that the file still exists and that the drive is connected.")
    };
    let mut file = File::open(path).or_explain(cant_read)?;
    let size = file.metadata().or_explain(cant_read)?.len();
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    hash_range(&mut file, 0, SAMPLE.min(size), &mut hasher).or_explain(cant_read)?;
    if size > SAMPLE {
        let start = size.saturating_sub(SAMPLE).max(SAMPLE);
        hash_range(&mut file, start, size - start, &mut hasher).or_explain(cant_read)?;
    }
    let digest = hasher.finalize();
    let sample_hash = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    Ok(Fingerprint { size, sample_hash })
}

fn hash_range(file: &mut File, start: u64, len: u64, hasher: &mut Sha256) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0; len as usize];
    file.read_exact(&mut buf)?;
    hasher.update(&buf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_contents_same_fingerprint_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..3 * SAMPLE as usize).map(|i| (i % 251) as u8).collect();
        let a = dir.path().join("a.mkv");
        let b = dir.path().join("moved and renamed.mkv");
        std::fs::write(&a, &data).unwrap();
        std::fs::write(&b, &data).unwrap();
        assert_eq!(fingerprint(&a).unwrap(), fingerprint(&b).unwrap());
        assert_eq!(fingerprint(&a).unwrap().size, 3 * SAMPLE);
    }

    #[test]
    fn a_change_at_either_end_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let data = vec![7u8; 3 * SAMPLE as usize];
        let path = dir.path().join("x");
        std::fs::write(&path, &data).unwrap();
        let original = fingerprint(&path).unwrap();
        let mut end = data.clone();
        *end.last_mut().unwrap() = 8;
        std::fs::write(&path, &end).unwrap();
        assert_ne!(fingerprint(&path).unwrap(), original);
    }

    #[test]
    fn small_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(fingerprint(&path).unwrap().size, 3);
        let err = fingerprint(&dir.path().join("gone")).unwrap_err();
        assert!(err.what().contains("can't be read"));
    }
}
