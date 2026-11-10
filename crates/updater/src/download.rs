//! Downloading a release file: in the background, resumable, and verified
//! before it is used.

use crate::manifest::{ManifestFile, hex, tampered};
use diag::{AppError, Explain};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Downloads can be slow; this only limits how long a stalled connection waits.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// How far a download has got, readable from another thread while it runs.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl Progress {
    pub fn new() -> Progress {
        Progress::default()
    }

    /// Bytes downloaded so far (including a resumed part) and the full size; 0 total until known.
    pub fn bytes(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    /// From 0 to 1, or `None` while the size is not known yet.
    pub fn fraction(&self) -> Option<f32> {
        let (done, total) = self.bytes();
        (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    /// Sets both numbers (also used to show a sample bar in screenshots).
    pub fn set(&self, done: u64, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
    }

    fn add(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Downloads `url` into `dir`, continuing a partial download if one exists,
/// and returns the finished file only if it matches `expected` exactly.
pub fn download(url: &str, dir: &Path, expected: &ManifestFile) -> diag::Result<PathBuf> {
    download_with_progress(url, dir, expected, &Progress::new())
}

/// [`download`], reporting bytes received to `progress` as they arrive.
pub fn download_with_progress(
    url: &str,
    dir: &Path,
    expected: &ManifestFile,
    progress: &Progress,
) -> diag::Result<PathBuf> {
    fs::create_dir_all(dir).or_explain(|| disk_problem(dir))?;
    let done = dir.join(&expected.name);
    if done.exists() && verify_file(&done, expected).is_ok() {
        progress.set(expected.size, expected.size);
        return Ok(done);
    }
    let partial = dir.join(format!("{}.part", expected.name));
    let have = fs::metadata(&partial).map_or(0, |m| m.len());
    if have > expected.size {
        let _ = fs::remove_file(&partial);
    }
    let have = fs::metadata(&partial).map_or(0, |m| m.len());

    if have < expected.size {
        let agent = crate::agent(DOWNLOAD_TIMEOUT);
        let mut request = agent.get(url);
        if have > 0 {
            request = request.header("Range", &format!("bytes={have}-"));
        }
        let mut response = request.call().map_err(|_| {
            AppError::new("The update could not be downloaded")
                .why("The connection to GitHub failed.")
                .fix("It continues where it stopped the next time the app opens.")
        })?;
        // A server that ignores the range sends the whole file: start over.
        let resumed = response.status() == 206;
        progress.set(if resumed { have } else { 0 }, expected.size);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&partial)
            .or_explain(|| disk_problem(dir))?;
        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0; 1 << 16];
        loop {
            let n = reader.read(&mut buffer).or_explain(|| {
                AppError::new("The update download was interrupted")
                    .fix("It continues where it stopped the next time the app opens.")
            })?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).or_explain(|| disk_problem(dir))?;
            progress.add(n as u64);
        }
        file.sync_all().or_explain(|| disk_problem(dir))?;
    }

    if let Err(err) = verify_file(&partial, expected) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    fs::rename(&partial, &done).or_explain(|| disk_problem(dir))?;
    Ok(done)
}

/// Checks size and SHA-256 of a file against the manifest.
pub fn verify_file(path: &Path, expected: &ManifestFile) -> diag::Result<()> {
    let mut file = File::open(path).or_explain(tampered)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buffer).or_explain(tampered)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buffer[..n]);
    }
    if size != expected.size || hex(&hasher.finalize()) != expected.sha256 {
        return Err(tampered());
    }
    Ok(())
}

fn disk_problem(dir: &Path) -> AppError {
    AppError::new("The update could not be saved")
        .why(format!("Writing to {} failed.", dir.display()))
        .fix("Check that the disk is not full.")
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn progress_reports_a_fraction_once_the_size_is_known() {
        let progress = Progress::new();
        assert_eq!(progress.fraction(), None);
        let seen_elsewhere = progress.clone();
        progress.set(25, 100);
        progress.add(25);
        assert_eq!(seen_elsewhere.bytes(), (50, 100));
        assert_eq!(seen_elsewhere.fraction(), Some(0.5));
    }

    #[test]
    fn an_already_finished_download_reports_full() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"finished package";
        let expected = ManifestFile {
            name: "pkg.zip".into(),
            size: bytes.len() as u64,
            sha256: crate::manifest::sha256_hex(bytes),
        };
        fs::write(tmp.path().join("pkg.zip"), bytes).unwrap();
        let progress = Progress::new();
        // No network needed: the finished file is found and checked first.
        download_with_progress("https://unused.invalid", tmp.path(), &expected, &progress).unwrap();
        assert_eq!(progress.fraction(), Some(1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::sha256_hex;

    fn entry(name: &str, bytes: &[u8]) -> ManifestFile {
        ManifestFile {
            name: name.into(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        }
    }

    #[test]
    fn a_matching_file_passes_and_a_changed_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pkg.zip");
        fs::write(&path, b"genuine").unwrap();
        assert!(verify_file(&path, &entry("pkg.zip", b"genuine")).is_ok());
        fs::write(&path, b"genuinf").unwrap();
        let err = verify_file(&path, &entry("pkg.zip", b"genuine")).unwrap_err();
        assert!(err.what().contains("refused"));
    }

    #[test]
    fn an_already_downloaded_file_is_not_fetched_again() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg.zip"), b"genuine").unwrap();
        // The URL is never contacted because the file is already complete and correct.
        let path = download(
            "http://invalid.invalid/pkg.zip",
            tmp.path(),
            &entry("pkg.zip", b"genuine"),
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"genuine");
    }
}
