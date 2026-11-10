//! Downloading a release file: in the background, resumable, and verified
//! before it is used.

use crate::manifest::{ManifestFile, hex, tampered};
use diag::{AppError, Explain};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Downloads can be slow; this only limits how long a stalled connection waits.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// Downloads `url` into `dir`, continuing a partial download if one exists,
/// and returns the finished file only if it matches `expected` exactly.
pub fn download(url: &str, dir: &Path, expected: &ManifestFile) -> diag::Result<PathBuf> {
    fs::create_dir_all(dir).or_explain(|| disk_problem(dir))?;
    let done = dir.join(&expected.name);
    if done.exists() && verify_file(&done, expected).is_ok() {
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
