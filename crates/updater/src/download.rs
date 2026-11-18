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
        receive(url, &partial, have, expected, progress, dir)?;
    }

    if let Err(err) = verify_file(&partial, expected) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    fs::rename(&partial, &done).or_explain(|| disk_problem(dir))?;
    Ok(done)
}

/// Fetches the rest of `partial` (from `have` bytes on) and writes it to disk,
/// never past the size the signed manifest gives the file.
fn receive(
    url: &str,
    partial: &Path,
    have: u64,
    expected: &ManifestFile,
    progress: &Progress,
    dir: &Path,
) -> diag::Result<()> {
    let agent = crate::agent(DOWNLOAD_TIMEOUT);
    let mut response = request(&agent, url, have)?;
    // A server that ignores the range sends the whole file: start over. One
    // that answers from another place than asked would join two different
    // parts, and the result would fail the checksum on every try: start over
    // from the beginning too.
    let mut resumed = have > 0 && response.status() == 206;
    if resumed && content_range_start(&response) != Some(have) {
        tracing::warn!("The server resumed the update download from another place; starting over");
        drop(response);
        let _ = fs::remove_file(partial);
        response = request(&agent, url, 0)?;
        resumed = false;
    }
    let start = if resumed { have } else { 0 };
    progress.set(start, expected.size);
    let length = response.body().content_length();
    if length.is_some_and(|length| start.saturating_add(length) > expected.size) {
        let _ = fs::remove_file(partial);
        return Err(too_large(expected));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(partial)
        .or_explain(|| disk_problem(dir))?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0; 1 << 16];
    let mut size = start;
    loop {
        let n = reader.read(&mut buffer).or_explain(|| {
            AppError::new("The update download was interrupted")
                .fix("It continues where it stopped the next time the app opens.")
        })?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > expected.size {
            drop(file);
            let _ = fs::remove_file(partial);
            return Err(too_large(expected));
        }
        file.write_all(&buffer[..n]).or_explain(|| disk_problem(dir))?;
        progress.add(n as u64);
    }
    file.sync_all().or_explain(|| disk_problem(dir))
}

/// Asks for `url`, from byte `from` on when that is not the start.
fn request(agent: &ureq::Agent, url: &str, from: u64) -> diag::Result<ureq::http::Response<ureq::Body>> {
    let mut request = agent.get(url);
    if from > 0 {
        request = request.header("Range", &format!("bytes={from}-"));
    }
    request.call().map_err(|_| {
        AppError::new("The update could not be downloaded")
            .why("The connection to GitHub failed.")
            .fix("It continues where it stopped the next time the app opens.")
    })
}

/// Where a partial answer starts, from its `Content-Range: bytes <start>-<end>/<size>`.
fn content_range_start<B>(response: &ureq::http::Response<B>) -> Option<u64> {
    let value = response.headers().get("content-range")?.to_str().ok()?;
    let range = value.trim().strip_prefix("bytes")?.trim_start();
    range.split('-').next()?.trim().parse().ok()
}

fn too_large(expected: &ManifestFile) -> AppError {
    AppError::new("The update was refused")
        .why(format!(
            "The server sent more than the {} bytes the signed file list gives {}.",
            expected.size, expected.name
        ))
        .fix("Nothing was installed. Try again later; if it keeps happening, report it.")
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

#[cfg(test)]
mod server_tests {
    use super::*;
    use crate::manifest::sha256_hex;
    use crate::test_server::{TestServer, response};

    const PACKAGE: &[u8] = b"the whole genuine update package";

    fn expected() -> ManifestFile {
        ManifestFile {
            name: "pkg.zip".into(),
            size: PACKAGE.len() as u64,
            sha256: sha256_hex(PACKAGE),
        }
    }

    fn range_start(head: &str) -> Option<usize> {
        let line = head.lines().find(|l| l.starts_with("range:"))?;
        line.split('=').nth(1)?.trim_end_matches('-').trim().parse().ok()
    }

    /// Answers a range with a 206 from `offset(asked)`, anything else with the whole file.
    fn ranged(offset: fn(usize) -> usize) -> impl Fn(&str) -> Vec<u8> + Send + 'static {
        move |head| match range_start(head) {
            Some(asked) => {
                let from = offset(asked);
                let body = &PACKAGE[from..];
                response(
                    "206 Partial Content",
                    &[
                        format!("content-length: {}", body.len()),
                        format!(
                            "content-range: bytes {from}-{}/{}",
                            PACKAGE.len() - 1,
                            PACKAGE.len()
                        ),
                    ],
                    body,
                )
            }
            None => response("200 OK", &[format!("content-length: {}", PACKAGE.len())], PACKAGE),
        }
    }

    #[test]
    fn a_partial_download_continues_where_it_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg.zip.part"), &PACKAGE[..10]).unwrap();
        let server = TestServer::start(1, ranged(|asked| asked));
        let progress = Progress::new();
        let path = download_with_progress(&server.url, tmp.path(), &expected(), &progress).unwrap();
        assert_eq!(fs::read(path).unwrap(), PACKAGE);
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("range: bytes=10-"));
        assert_eq!(progress.bytes(), (PACKAGE.len() as u64, PACKAGE.len() as u64));
    }

    #[test]
    fn a_resume_from_the_wrong_place_starts_over() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg.zip.part"), &PACKAGE[..10]).unwrap();
        // The server answers the range from 4 bytes earlier than asked.
        let server = TestServer::start(2, ranged(|asked| asked - 4));
        let path = download(&server.url, tmp.path(), &expected()).unwrap();
        assert_eq!(fs::read(path).unwrap(), PACKAGE);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("range: bytes=10-"));
        assert!(!requests[1].contains("range:"));
    }

    #[test]
    fn a_body_longer_than_the_manifest_says_is_refused_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        // No length given: the body runs on past the package's size.
        let server = TestServer::start(1, |_| {
            let endless = vec![b'x'; 4 << 20];
            response("200 OK", &[], &endless)
        });
        let progress = Progress::new();
        let err = download_with_progress(&server.url, tmp.path(), &expected(), &progress).unwrap_err();
        assert!(err.what().contains("refused"), "{err:?}");
        assert!(err.why_text().unwrap_or("").contains("more than"), "{err:?}");
        // Nothing past the stated size was ever written.
        assert!(progress.bytes().0 <= PACKAGE.len() as u64);
        assert!(!tmp.path().join("pkg.zip.part").exists());
        assert!(!tmp.path().join("pkg.zip").exists());
    }

    #[test]
    fn a_stated_length_past_the_manifest_size_is_refused_before_reading() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg.zip.part"), &PACKAGE[..10]).unwrap();
        let server = TestServer::start(1, |_| {
            response("200 OK", &["content-length: 1000000".into()], b"")
        });
        let err = download(&server.url, tmp.path(), &expected()).unwrap_err();
        assert!(err.what().contains("refused"), "{err:?}");
        assert!(!tmp.path().join("pkg.zip.part").exists());
    }

    #[test]
    fn a_content_range_start_is_read() {
        let answer = |value: &str| {
            ureq::http::Response::builder()
                .header("Content-Range", value)
                .body(())
                .unwrap()
        };
        assert_eq!(content_range_start(&answer("bytes 100-199/200")), Some(100));
        assert_eq!(content_range_start(&answer("bytes 0-9/*")), Some(0));
        assert_eq!(content_range_start(&answer("items 1-2/3")), None);
        let none = ureq::http::Response::builder().body(()).unwrap();
        assert_eq!(content_range_start(&none), None);
    }
}
