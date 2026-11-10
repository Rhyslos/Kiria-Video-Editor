//! `cargo xtask ffmpeg`: fetches the FFmpeg build pinned in
//! `packaging/ffmpeg.env` into `.ffmpeg/sdk` (headers, libraries and the
//! shared libraries), which `.cargo/config.toml` points the build at.
//!
//! On Windows it also copies the DLLs next to where Cargo puts programs and
//! tests, because Windows finds DLLs beside the program.
//!
//! Uses the `curl` and `tar` commands that come with Windows 10+ and Linux.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn fetch(args: &[String]) -> Result<(), String> {
    let root = crate::media::workspace_root();
    let env = std::fs::read_to_string(root.join("packaging/ffmpeg.env"))
        .map_err(|e| format!("Can't read packaging/ffmpeg.env: {e}"))?;
    let key = if cfg!(windows) {
        "FFMPEG_WINDOWS_URL="
    } else {
        "FFMPEG_LINUX_URL="
    };
    let url = env
        .lines()
        .find_map(|l| l.strip_prefix(key))
        .ok_or_else(|| format!("packaging/ffmpeg.env has no {key} line"))?
        .trim()
        .to_owned();

    let base = root.join(".ffmpeg");
    let sdk = base.join("sdk");
    let stamp = base.join("source-url.txt");
    let force = args.iter().any(|a| a == "--force");
    if !force
        && std::fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == url)
        && sdk.join("include").is_dir()
    {
        println!("FFmpeg is already in {}", sdk.display());
    } else {
        download(&url, &base, &sdk)?;
        std::fs::write(&stamp, &url).map_err(|e| format!("Can't write {}: {e}", stamp.display()))?;
        println!("FFmpeg is in {}", sdk.display());
    }
    if cfg!(windows) {
        copy_dlls(&sdk, &target_dir(&root))?;
    }
    Ok(())
}

fn download(url: &str, base: &Path, sdk: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(base);
    let unpack = base.join("unpack");
    std::fs::create_dir_all(&unpack).map_err(|e| format!("Can't create {}: {e}", unpack.display()))?;
    let archive = base.join(if url.ends_with(".zip") {
        "ffmpeg.zip"
    } else {
        "ffmpeg.tar.xz"
    });
    println!("Downloading {url}");
    run(Command::new("curl").args(["-fsSL", "-o"]).arg(&archive).arg(url))?;
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
