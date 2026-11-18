//! `cargo xtask ffmpeg`: fetches the FFmpeg build pinned in
//! `packaging/ffmpeg.env` into `.ffmpeg/sdk` (headers, libraries and the
//! shared libraries), which `.cargo/config.toml` points the build at.
//!
//! The archive must have the SHA-256 written beside its URL in that file; one
//! that differs is refused before anything is unpacked, so a build never uses
//! an FFmpeg nobody chose.
//!
//! On Windows it also copies the DLLs next to where Cargo puts programs and
//! tests, because Windows finds DLLs beside the program.
//!
//! Uses the `curl` and `tar` commands that come with Windows 10+ and Linux.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn fetch(args: &[String]) -> Result<(), String> {
    let root = crate::media::workspace_root();
    let env = std::fs::read_to_string(root.join("packaging/ffmpeg.env"))
        .map_err(|e| format!("Can't read packaging/ffmpeg.env: {e}"))?;
    let (url_key, hash_key) = if cfg!(windows) {
        ("FFMPEG_WINDOWS_URL", "FFMPEG_WINDOWS_SHA256")
    } else {
        ("FFMPEG_LINUX_URL", "FFMPEG_LINUX_SHA256")
    };
    let url = env_value(&env, url_key)?;
    let sha256 = env_value(&env, hash_key)?.to_ascii_lowercase();

    let base = root.join(".ffmpeg");
    let sdk = base.join("sdk");
    let stamp = base.join("source-url.txt");
    let pinned = format!("{url}\n{sha256}");
    let force = args.iter().any(|a| a == "--force");
    if !force
        && std::fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == pinned)
        && sdk.join("include").is_dir()
    {
        println!("FFmpeg is already in {}", sdk.display());
    } else {
        download(&url, &sha256, &base, &sdk)?;
        std::fs::write(&stamp, &pinned).map_err(|e| format!("Can't write {}: {e}", stamp.display()))?;
        println!("FFmpeg is in {}", sdk.display());
    }
    if cfg!(windows) {
        copy_dlls(&sdk, &target_dir(&root))?;
    }
    Ok(())
}

/// The value of `key=` in `packaging/ffmpeg.env`.
fn env_value(env: &str, key: &str) -> Result<String, String> {
    env.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            format!("packaging/ffmpeg.env has no {key} line. Put it back from git (see packaging/README.md).")
        })
}

/// Refuses an archive whose SHA-256 is not the pinned one.
fn check_archive(bytes: &[u8], expected: &str, url: &str) -> Result<(), String> {
    let actual: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "The FFmpeg archive from {url} is not the pinned build: its SHA-256 is {actual}, \
             packaging/ffmpeg.env expects {expected}. Nothing was unpacked. The build host may have \
             replaced the file, or the download was damaged: try again, and if it still differs, \
             bump the pin on purpose (a dated release and its hashes, see packaging/README.md)."
        ))
    }
}

fn download(url: &str, sha256: &str, base: &Path, sdk: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(base);
    let unpack = base.join("unpack");
    std::fs::create_dir_all(&unpack).map_err(|e| format!("Can't create {}: {e}", unpack.display()))?;
    let archive = base.join(if url.ends_with(".zip") {
        "ffmpeg.zip"
    } else {
        "ffmpeg.tar.xz"
    });
    println!("Downloading {url}");
    run(Command::new("curl").args(["-fsSL", "-o"]).arg(&archive).arg(url)).map_err(|e| {
        format!(
            "{e}. If the build host says the file is not found, it has removed the pinned dated \
             release: bump the pin in packaging/ffmpeg.env (see packaging/README.md)."
        )
    })?;
    let bytes = std::fs::read(&archive).map_err(|e| format!("Can't read {}: {e}", archive.display()))?;
    if let Err(e) = check_archive(&bytes, sha256, url) {
        let _ = std::fs::remove_dir_all(base);
        return Err(e);
    }
    drop(bytes);
    run(Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpack))?;
    let top = std::fs::read_dir(&unpack)
        .map_err(|e| format!("Can't list {}: {e}", unpack.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .ok_or("The FFmpeg archive had no folder in it")?;
    std::fs::rename(&top, sdk).map_err(|e| format!("Can't move FFmpeg into {}: {e}", sdk.display()))?;
    let _ = std::fs::remove_dir_all(&unpack);
    let _ = std::fs::remove_file(&archive);
    Ok(())
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("Can't run {:?}: {e}", cmd.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", cmd.get_program()))
    }
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

/// Windows: copies the FFmpeg DLLs beside the programs and test programs of
/// both build profiles.
fn copy_dlls(sdk: &Path, target: &Path) -> Result<(), String> {
    let bin = sdk.join("bin");
    let dlls: Vec<PathBuf> = std::fs::read_dir(&bin)
        .map_err(|e| format!("Can't list {}: {e}", bin.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    for profile in ["debug", "release"] {
        for dir in [target.join(profile), target.join(profile).join("deps")] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
            for dll in &dlls {
                if let Some(name) = dll.file_name() {
                    std::fs::copy(dll, dir.join(name))
                        .map_err(|e| format!("Can't copy {}: {e}", dll.display()))?;
                }
            }
        }
    }
    println!("Copied {} FFmpeg DLLs into {}", dlls.len(), target.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // SHA-256 of "abc" (FIPS 180-2 example).
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn the_pinned_hash_passes() {
        assert!(check_archive(b"abc", ABC, "url").is_ok());
        assert!(check_archive(b"abc", &ABC.to_ascii_uppercase(), "url").is_ok());
    }

    #[test]
    fn another_hash_is_refused() {
        let error = check_archive(b"abd", ABC, "https://example/ffmpeg.zip").unwrap_err();
        assert!(error.contains("not the pinned build"));
        assert!(error.contains("https://example/ffmpeg.zip"));
        assert!(check_archive(b"abc", "", "url").is_err());
    }

    #[test]
    fn the_env_file_pins_both_archives() {
        let env =
            std::fs::read_to_string(crate::media::workspace_root().join("packaging/ffmpeg.env")).unwrap();
        for key in ["FFMPEG_WINDOWS_SHA256", "FFMPEG_LINUX_SHA256"] {
            let hash = env_value(&env, key).unwrap();
            assert!(
                hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{key}"
            );
        }
        for key in ["FFMPEG_WINDOWS_URL", "FFMPEG_LINUX_URL"] {
            assert!(
                !env_value(&env, key).unwrap().contains("/latest/"),
                "{key} must name a dated build"
            );
        }
        assert!(env_value(&env, "FFMPEG_MISSING").is_err());
    }
}
