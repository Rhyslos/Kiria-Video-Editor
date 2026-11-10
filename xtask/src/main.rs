//! Project chores, run with `cargo xtask <task>`.
//!
//! Tasks:
//! - `check-media`: checks that every file listed in `test-media/MANIFEST.md`
//!   is present and has the listed SHA-256 checksum.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("check-media") => check_media(),
        _ => {
            eprintln!(
                "Usage: cargo xtask <task>\n\nTasks:\n  check-media   verify test-media/ against its MANIFEST.md"
            );
            ExitCode::FAILURE
        }
    }
}

/// One row of the manifest table: `| file | sha256 | what it tests |`.
#[derive(Debug, PartialEq)]
struct Entry {
    file: String,
    sha256: String,
}

fn check_media() -> ExitCode {
    let media_dir = workspace_root().join("test-media");
    let manifest = media_dir.join("MANIFEST.md");
    let text = match std::fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Can't read {}: {err}", manifest.display());
            return ExitCode::FAILURE;
        }
    };

    let entries = parse_manifest(&text);
    if entries.is_empty() {
        println!(
            "The manifest lists no files yet. Add rows to test-media/MANIFEST.md as recordings are collected."
        );
        return ExitCode::SUCCESS;
    }

    let mut problems = 0;
    for entry in &entries {
        let path = media_dir.join(&entry.file);
        match sha256_of(&path) {
            Ok(sum) if sum.eq_ignore_ascii_case(&entry.sha256) => println!("ok       {}", entry.file),
            Ok(sum) => {
                problems += 1;
                println!("changed  {} (checksum is {sum})", entry.file);
            }
            Err(_) => {
                problems += 1;
                println!("missing  {}", entry.file);
            }
        }
    }

    if problems == 0 {
        println!("All {} test files are present and unchanged.", entries.len());
        ExitCode::SUCCESS
    } else {
        println!(
            "{problems} of {} test files are missing or changed. Copy them from the shared test-media folder.",
            entries.len()
        );
        ExitCode::FAILURE
    }
}

/// Reads table rows whose second column looks like a SHA-256 checksum.
fn parse_manifest(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
            let [file, sha256, ..] = cells.as_slice() else {
                return None;
            };
            let is_checksum = sha256.len() == 64 && sha256.chars().all(|c| c.is_ascii_hexdigit());
            is_checksum.then(|| Entry {
                file: file.trim_matches('`').to_string(),
                sha256: sha256.to_string(),
            })
        })
        .collect()
}

fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rows_with_a_checksum_and_skips_the_rest() {
        let sum = "a".repeat(64);
        let text = format!(
            "# Test media\n\n| File | SHA-256 | Tests |\n|---|---|---|\n| `vfr_1080p60.mkv` | {sum} | variable frame rate |\n"
        );
        assert_eq!(
            parse_manifest(&text),
            vec![Entry {
                file: "vfr_1080p60.mkv".into(),
                sha256: sum
            }]
        );
    }
}
