//! Approaches C and D need files built from the original ahead of time: a
//! proxy (a small copy that is cheap to jump around in) or a picture index
//! (a small picture every so often). The spike builds them with the `ffmpeg`
//! program and times it; the editor would do the same work through the
//! library, on the background pool.

use std::process::Command;
use std::time::Instant;

/// One kind of file built from a clip.
pub struct Recipe {
    pub name: &'static str,
    pub file: &'static str,
    /// Every frame in the file is a key frame: a fetch is one frame.
    pub all_keys: bool,
    /// Options for reading the original.
    input: &'static [&'static str],
    /// Filters and encoder for the output.
    output: &'static [&'static str],
}

const INTRA: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-bf",
    "0",
    "-crf",
    "26",
    "-pix_fmt",
    "yuv420p",
];
const JPEG: &[&str] = &["-c:v", "mjpeg", "-q:v", "6", "-pix_fmt", "yuvj420p"];

pub const RECIPES: [Recipe; 5] = [
    Recipe {
        name: "C1 proxy: 540p H.264, every frame a key frame",
        file: "proxy-intra.mp4",
        all_keys: true,
        input: &[],
        output: &["-vf", "scale=-2:540", "-g", "1"],
    },
    Recipe {
        name: "C2 proxy: 540p JPEG frames",
        file: "proxy-jpeg.mkv",
        all_keys: true,
        input: &[],
        output: &["-vf", "scale=-2:540"],
    },
    Recipe {
        name: "C3 proxy: 540p H.264, a key frame every 8 frames",
        file: "proxy-short.mp4",
        all_keys: false,
        input: &[],
        output: &[
            "-vf",
            "scale=-2:540",
            "-g",
            "8",
            "-keyint_min",
            "8",
            "-sc_threshold",
            "0",
        ],
    },
    Recipe {
        name: "D1 index: a 360p JPEG for every key frame",
        file: "index-keys.mkv",
        all_keys: true,
        input: &["-skip_frame", "nokey"],
        output: &["-vf", "scale=-2:360"],
    },
    Recipe {
        name: "D2 index: a 360p JPEG every 15 frames",
        file: "index-15.mkv",
        all_keys: true,
        input: &[],
        output: &["-vf", "select=not(mod(n\\,15)),scale=-2:360"],
    },
];

impl Recipe {
    fn encoder(&self) -> &'static [&'static str] {
        if self.file.ends_with(".mp4") { INTRA } else { JPEG }
    }
}

pub struct Built {
    pub path: String,
    pub seconds: f64,
    pub bytes: u64,
}

/// Builds `recipe` from the first `span` seconds of `clip` into `folder`.
/// `hardware` decodes the original on the GPU, as the editor would; when
/// that fails the build runs again on the CPU.
pub fn build(
    ffmpeg: &str,
    recipe: &Recipe,
    clip: &str,
    span: f64,
    hardware: &Option<String>,
    folder: &std::path::Path,
) -> Result<Built, String> {
    let path = folder.join(recipe.file).to_string_lossy().into_owned();
    let run = |hardware: &Option<String>| -> Result<f64, String> {
        let mut command = Command::new(ffmpeg);
        command.args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"]);
        if let Some(kind) = hardware {
            command.args(["-hwaccel", kind]);
        }
        command.args(recipe.input);
        command.args(["-t", &format!("{span:.3}"), "-i", clip]);
        command.args(["-map", "0:v:0", "-an", "-sn", "-fps_mode", "passthrough"]);
        command.args(recipe.output);
        command.args(recipe.encoder());
        command.arg(&path);
        let started = Instant::now();
        let out = command
            .output()
            .map_err(|e| format!("the ffmpeg program could not be started ({e})"))?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr);
            return Err(text.lines().last().unwrap_or("ffmpeg failed").to_owned());
        }
        Ok(started.elapsed().as_secs_f64())
    };
    let seconds = match run(hardware) {
        Ok(s) => s,
        Err(_) if hardware.is_some() => run(&None)?,
        Err(e) => return Err(e),
    };
    let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    Ok(Built { path, seconds, bytes })
}
